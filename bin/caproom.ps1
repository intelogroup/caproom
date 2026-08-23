#Requires -Version 5.1
# caproom -- Windows backend.
#
# Enforcement uses a Job Object with JOB_OBJECT_LIMIT_PROCESS_MEMORY, which is a
# real kernel-enforced cap (no polling race) and automatically covers child
# processes. Note this limits *committed virtual memory*, not resident set -- the
# POSIX backend caps RSS, so the same --limit value can bite at a different point.
#
# park uses EmptyWorkingSet, which trims a process's working set to the pagefile
# on demand without suspending it -- unlike the POSIX backend's SIGSTOP, the
# process keeps running, so wake is a no-op here.

$ErrorActionPreference = 'Stop'

function Show-Usage {
    param([switch]$AsHelp)
    $text = @'
usage: caproom [--limit <mb>] [--interval <sec>] -- <command> [args...]
       caproom park <pid>
       caproom wake <pid>
       caproom status <pid>
       caproom guard [--threshold <pct>] [--interval <sec>] <pid...>
       caproom init <command> [--limit <mb>]
       caproom top --json [--pid <pid>] [--park-min-mb <mb>]
       caproom watch [--threshold-mb <mb>] [--auto-park] [--auto-wake-free-pct <pct>] [--json] <pid...>
       caproom setup / freemem

  --limit <mb>     memory cap in MB (default: 4096). On Windows this caps
                    committed virtual memory (Job Object ProcessMemoryLimit);
                    on macOS/Linux it caps RSS. Same flag, different quantity.
  --interval <sec> poll interval for the fallback watchdog (default: 0.2)
  --force-watchdog use the polling watchdog instead of the Job Object backend

Windows differences from macOS/Linux:
  * No SIGTERM grace period. Windows console apps have no signal equivalent,
    so a watchdog breach is a hard kill. The Job Object backend does not kill
    at all -- the allocation simply fails inside the process.
  * park <pid> uses EmptyWorkingSet: memory is trimmed to the pagefile
    immediately, on demand, and the process KEEPS RUNNING. There is no
    suspension, so it cannot hang a process that something is waiting on.
  * wake <pid> is a no-op -- nothing was suspended. Trimmed pages fault back
    in by themselves on next access.

guard watches SYSTEM-WIDE free memory (not any single process) and auto-parks
tracked pids (EmptyWorkingSet) once free mem drops below --threshold percent,
before the OS has to fail an allocation itself. Use it when unrelated heavy
processes (e.g. a GPU inference job and a TTS job in separate terminals)
share a box and neither individually breaches any --limit cap. Foreground,
blocking; exits once all watched pids have exited. There is no unpark step --
park just trims the working set, pages fault back in on next access.

env vars (override flags): CAPROOM_LIMIT_MB, CAPROOM_INTERVAL

examples:
  caproom --limit 2048 -- npm run build
  caproom park 12345
  caproom guard --threshold 10 --interval 5 -- 12345 12346
  caproom init claude --limit 6144
'@
    if ($AsHelp) { Write-Output $text; exit 0 }
    [Console]::Error.WriteLine($text)
    exit 1
}

$NativeMethods = @'
using System;
using System.Runtime.InteropServices;

public static class Caproom {
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern IntPtr CreateJobObject(IntPtr a, string lpName);

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool SetInformationJobObject(IntPtr hJob, int infoClass, IntPtr lpInfo, uint cbInfo);

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool AssignProcessToJobObject(IntPtr hJob, IntPtr hProcess);

    [DllImport("psapi.dll", SetLastError = true)]
    public static extern bool EmptyWorkingSet(IntPtr hProcess);

    [StructLayout(LayoutKind.Sequential)]
    public struct JOBOBJECT_BASIC_LIMIT_INFORMATION {
        public Int64 PerProcessUserTimeLimit;
        public Int64 PerJobUserTimeLimit;
        public UInt32 LimitFlags;
        public UIntPtr MinimumWorkingSetSize;
        public UIntPtr MaximumWorkingSetSize;
        public UInt32 ActiveProcessLimit;
        public UIntPtr Affinity;
        public UInt32 PriorityClass;
        public UInt32 SchedulingClass;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct IO_COUNTERS {
        public UInt64 ReadOperationCount;
        public UInt64 WriteOperationCount;
        public UInt64 OtherOperationCount;
        public UInt64 ReadTransferCount;
        public UInt64 WriteTransferCount;
        public UInt64 OtherTransferCount;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
        public JOBOBJECT_BASIC_LIMIT_INFORMATION BasicLimitInformation;
        public IO_COUNTERS IoInfo;
        public UIntPtr ProcessMemoryLimit;
        public UIntPtr JobMemoryLimit;
        public UIntPtr PeakProcessMemoryUsed;
        public UIntPtr PeakJobMemoryUsed;
    }

    public const int ExtendedLimitInformation = 9;
    public const uint LIMIT_PROCESS_MEMORY = 0x00000100;
    public const uint LIMIT_KILL_ON_JOB_CLOSE = 0x00002000;
}
'@

function Import-Native {
    if (-not ('Caproom' -as [type])) { Add-Type -TypeDefinition $script:NativeMethods }
}

function Invoke-Park {
    param([int]$TargetPid)
    Import-Native
    $proc = Get-Process -Id $TargetPid -ErrorAction SilentlyContinue
    if (-not $proc) { [Console]::Error.WriteLine("caproom: no such pid $TargetPid"); exit 1 }
    $before = $proc.WorkingSet64
    if (-not [Caproom]::EmptyWorkingSet($proc.Handle)) {
        [Console]::Error.WriteLine("caproom: EmptyWorkingSet failed for pid $TargetPid (error $([Runtime.InteropServices.Marshal]::GetLastWin32Error()))")
        exit 1
    }
    $after = (Get-Process -Id $TargetPid).WorkingSet64
    [Console]::Error.WriteLine("caproom: pid $TargetPid parked -- working set trimmed $([math]::Round($before/1MB))MB -> $([math]::Round($after/1MB))MB. Process is STILL RUNNING (no suspension); pages fault back in on access.")
}

function Invoke-Wake {
    param([int]$TargetPid)
    if (-not (Get-Process -Id $TargetPid -ErrorAction SilentlyContinue)) {
        [Console]::Error.WriteLine("caproom: no such pid $TargetPid"); exit 1
    }
    [Console]::Error.WriteLine("caproom: pid $TargetPid -- nothing to wake. On Windows park trims the working set without suspending, so the process never stopped running.")
}

function Invoke-Status {
    param([int]$TargetPid)
    $proc = Get-Process -Id $TargetPid -ErrorAction SilentlyContinue
    if (-not $proc) { [Console]::Error.WriteLine("caproom: no such pid $TargetPid"); exit 1 }
    [PSCustomObject]@{
        Pid           = $proc.Id
        WorkingSetMB  = [math]::Round($proc.WorkingSet64 / 1MB)
        CommittedMB   = [math]::Round($proc.PagedMemorySize64 / 1MB)
        Elapsed       = (Get-Date) - $proc.StartTime
        Command       = $proc.ProcessName
    } | Format-List
}

function Get-FreeMemPercent {
    $os = Get-CimInstance Win32_OperatingSystem
    return [math]::Floor(($os.FreePhysicalMemory * 100) / $os.TotalVisibleMemorySize)
}

function Invoke-Guard {
    param([int]$Threshold, [double]$Interval, [int[]]$TargetPids)
    Import-Native
    [Console]::Error.WriteLine("caproom: guarding $($TargetPids.Count) pid(s), park when system free mem < ${Threshold}% (poll ${Interval}s)")
    $parked = @{}
    while ($true) {
        $alive = @($TargetPids | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue })
        if ($alive.Count -eq 0) {
            [Console]::Error.WriteLine("caproom: guard: all watched pids exited")
            exit 0
        }
        $TargetPids = $alive
        $pct = Get-FreeMemPercent
        if ($pct -lt $Threshold) {
            foreach ($p in $TargetPids) {
                if (-not $parked.ContainsKey($p)) {
                    $proc = Get-Process -Id $p -ErrorAction SilentlyContinue
                    if ($proc) {
                        [Console]::Error.WriteLine("caproom: system free mem ${pct}% < ${Threshold}% threshold -- about to blow, parking pid $p (EmptyWorkingSet)")
                        [void][Caproom]::EmptyWorkingSet($proc.Handle)
                        $parked[$p] = $true
                    }
                }
            }
        }
        Start-Sleep -Seconds $Interval
    }
}

function Invoke-Init {
    param([string]$Target, [int]$LimitMb)
    @"
# caproom: auto-cap '$Target' -- added by 'caproom init $Target'
# override per-shell: `$env:CAPROOM_LIMIT_MB = 8192
function ${Target}_capped {
    `$limit = if (`$env:CAPROOM_LIMIT_MB) { `$env:CAPROOM_LIMIT_MB } else { $LimitMb }
    caproom --force-watchdog --limit `$limit -- $Target @args
}
Set-Alias -Name $Target -Value ${Target}_capped -Force
"@
}

# Start-Process -ArgumentList joins an array with spaces and does no quoting,
# so an argument containing whitespace gets re-split into several arguments by
# the callee. Build one command line with CommandLineToArgvW quoting instead.
function ConvertTo-ArgString {
    param([string[]]$Arguments)
    $quoted = foreach ($a in $Arguments) {
        if ($a -eq '') { '""' }
        elseif ($a -notmatch '[\s"]') { $a }
        else {
            # Double any backslashes preceding a quote (and at end of string),
            # then escape the quotes themselves.
            $s = $a -replace '(\\*)"', '$1$1\"'
            $s = $s -replace '(\\+)$', '$1$1'
            '"' + $s + '"'
        }
    }
    $quoted -join ' '
}

function New-CappedProcess {
    # Every pipe-based capture (Process class + ReadToEndAsync, Process class
    # + raw BaseStream, with and without stripping std-handle inheritance)
    # returned zero bytes in CI despite a clean exit 0 -- caproom is invoked
    # as powershell.exe -File caproom.ps1 from the Node shim, itself invoked
    # from a pwsh.EXE step that captures via a pipe (`| Out-String`), and
    # something in that nesting swallows anonymous-pipe output every time.
    # File-based redirection (Start-Process -RedirectStandardOutput <file>)
    # was the one capture method that survived an isolated repro under the
    # exact same nesting in the same CI job, so route through temp files
    # instead of pipes entirely.
    param([string]$Exe, [string]$ArgLine)
    $resolvedExe = $Exe
    $cmd = Get-Command $Exe -ErrorAction SilentlyContinue
    if ($cmd) { $resolvedExe = $cmd.Source }

    $outFile = [IO.Path]::GetTempFileName()
    $errFile = [IO.Path]::GetTempFileName()
    $proc = Start-Process -FilePath $resolvedExe -ArgumentList $ArgLine -NoNewWindow `
        -RedirectStandardOutput $outFile -RedirectStandardError $errFile -PassThru

    # Start-Process's PassThru object opens a limited-rights handle lazily --
    # if .Handle is never touched while the process is still alive, .ExitCode
    # silently reads back 0 for an already-exited process instead of the real
    # code. Force the full-access handle open now, before it can exit.
    $null = $proc.Handle

    $proc | Add-Member -NotePropertyName StdoutFile -NotePropertyValue $outFile
    $proc | Add-Member -NotePropertyName StderrFile -NotePropertyValue $errFile
    return $proc
}

function Read-NewOutput {
    # Tail-follow one capture file from its recorded byte offset, writing new
    # bytes to the given console stream as they land so output streams live.
    # Byte-level writes pass the child's bytes through un-re-encoded.
    param([string]$Path, $Offsets, $Stream)
    if (-not (Test-Path -LiteralPath $Path)) { return }
    $fs = [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite)
    try {
        if ($fs.Length -lt $Offsets[$Path]) { $Offsets[$Path] = 0 }   # file truncated/recreated under us
        if ($fs.Length -gt $Offsets[$Path]) {
            $fs.Position = $Offsets[$Path]
            $len = [int]($fs.Length - $fs.Position)
            $buf = New-Object byte[] $len
            $read = 0
            while ($read -lt $len) {
                $n = $fs.Read($buf, $read, $len - $read)
                if ($n -le 0) { break }
                $read += $n
            }
            if ($read -gt 0) {
                $Offsets[$Path] += $read
                $Stream.Write($buf, 0, $read)
                $Stream.Flush()
            }
        }
    } finally { $fs.Close() }
}

function Wait-CappedProcess {
    # Drains remaining output, cleans up the temp capture files, returns the
    # exit code. If the caller streamed while polling (watchdog path), pass
    # the SAME offsets table so only the unread tail is relayed here; the
    # job-object path streams internally on a 50ms cadence.
    param($Proc, $Offsets = @{ ($Proc.StdoutFile) = 0; ($Proc.StderrFile) = 0 })
    try {
        while (-not $Proc.HasExited) {
            Read-NewOutput -Path $Proc.StdoutFile -Offsets $Offsets -Stream ([Console]::Out)
            Read-NewOutput -Path $Proc.StderrFile -Offsets $Offsets -Stream ([Console]::Error)
            Start-Sleep -Milliseconds 50
        }
        Read-NewOutput -Path $Proc.StdoutFile -Offsets $Offsets -Stream ([Console]::Out)
        Read-NewOutput -Path $Proc.StderrFile -Offsets $Offsets -Stream ([Console]::Error)
        return $Proc.ExitCode
    } finally {
        Remove-Item -LiteralPath $Proc.StdoutFile, $Proc.StderrFile -ErrorAction SilentlyContinue
    }
}

# The watchdog must see the WHOLE tree, not just the top pid: coding agents
# keep their memory in children (MCP servers, bundler daemons, headless
# browsers) while the parent's own working set stays flat. Walk the
# parent->child edges of one Win32_Process snapshot and sum working sets.
function Get-TreeWorkingSetBytes {
    param([int]$RootPid)
    $ws = @{}
    $kids = @{}
    foreach ($p in Get-CimInstance -ClassName Win32_Process -Property ProcessId, ParentProcessId, WorkingSetSize) {
        $pidInt = [int]$p.ProcessId
        $ppidInt = [int]$p.ParentProcessId
        $ws[$pidInt] = [uint64]$p.WorkingSetSize
        if (-not $kids.ContainsKey($ppidInt)) { $kids[$ppidInt] = @() }
        $kids[$ppidInt] += $pidInt
    }
    if (-not $ws.ContainsKey($RootPid)) { return [uint64]0 }
    $total = [uint64]0
    $queue = New-Object System.Collections.Queue
    $visited = @{}
    $queue.Enqueue($RootPid)
    while ($queue.Count -gt 0) {
        $cur = [int]$queue.Dequeue()
        if ($visited.ContainsKey($cur)) { continue }   # pid-reuse / cycle guard
        $visited[$cur] = $true
        $total += $ws[$cur]
        if ($kids.ContainsKey($cur)) { foreach ($c in $kids[$cur]) { [void]$queue.Enqueue($c) } }
    }
    return $total
}

function Invoke-Capped {
    param([int]$LimitMb, [double]$Interval, [bool]$ForceWatchdog, [string[]]$Command)

    $exe = $Command[0]
    $rest = if ($Command.Length -gt 1) { ConvertTo-ArgString $Command[1..($Command.Length - 1)] } else { '' }

    if (-not $ForceWatchdog) {
        try {
            Import-Native
            $job = [Caproom]::CreateJobObject([IntPtr]::Zero, $null)
            if ($job -eq [IntPtr]::Zero) { throw 'CreateJobObject returned NULL' }

            $info = New-Object Caproom+JOBOBJECT_EXTENDED_LIMIT_INFORMATION
            $info.BasicLimitInformation.LimitFlags = [Caproom]::LIMIT_PROCESS_MEMORY -bor [Caproom]::LIMIT_KILL_ON_JOB_CLOSE
            $info.ProcessMemoryLimit = [UIntPtr]::new([uint64]$LimitMb * 1MB)

            $size = [Runtime.InteropServices.Marshal]::SizeOf($info)
            $ptr = [Runtime.InteropServices.Marshal]::AllocHGlobal($size)
            try {
                [Runtime.InteropServices.Marshal]::StructureToPtr($info, $ptr, $false)
                if (-not [Caproom]::SetInformationJobObject($job, [Caproom]::ExtendedLimitInformation, $ptr, $size)) {
                    throw "SetInformationJobObject failed (error $([Runtime.InteropServices.Marshal]::GetLastWin32Error()))"
                }
            } finally {
                [Runtime.InteropServices.Marshal]::FreeHGlobal($ptr)
            }

            # Assign ONLY the child to the job, immediately after spawn --
            # never caproom's own process. Putting the PowerShell runtime
            # inside the job made its ~100MB+ commit eat the user's budget,
            # and a PS spike could fail allocations inside THEIR command.
            # Policy: prefer under-counting over impeding. Cost is a
            # millisecond-scale window before assignment lands; the child's
            # own descendants are still covered automatically (they inherit
            # the association at CreateProcess).
            [Console]::Error.WriteLine("caproom: job object backend, limit=${LimitMb}m (committed memory, kernel-enforced, covers the command and its descendants)")
            $proc = New-CappedProcess -Exe $exe -ArgLine $rest
            if (-not [Caproom]::AssignProcessToJobObject($job, $proc.Handle)) {
                # Child is already running -- kill it before falling back,
                # or the watchdog path below would launch a second instance.
                & taskkill.exe /PID $proc.Id /T /F 2>$null | Out-Null
                throw "AssignProcessToJobObject failed (error $([Runtime.InteropServices.Marshal]::GetLastWin32Error()))"
            }
            exit (Wait-CappedProcess $proc)
        } catch {
            [Console]::Error.WriteLine("caproom: job object backend unavailable ($($_.Exception.Message)) -- falling back to watchdog")
        }
    }

    [Console]::Error.WriteLine("caproom: watchdog backend, limit=${LimitMb}m poll=${Interval}s (process-tree working set, hard kill on breach -- Windows has no SIGTERM equivalent)")
    $limitBytes = [uint64]$LimitMb * 1MB
    $proc = New-CappedProcess -Exe $exe -ArgLine $rest
    # Stream output WHILE the breach-poll loop runs -- polling must not sit
    # on the whole runtime and leave the tail-follow to drain everything at
    # exit. Same offsets table flows into Wait-CappedProcess for the final
    # drain so nothing is relayed twice.
    $offsets = @{ ($proc.StdoutFile) = 0; ($proc.StderrFile) = 0 }
    while (-not $proc.HasExited) {
        Read-NewOutput -Path $proc.StdoutFile -Offsets $offsets -Stream ([Console]::Out)
        Read-NewOutput -Path $proc.StderrFile -Offsets $offsets -Stream ([Console]::Error)
        Start-Sleep -Seconds $Interval
        if ($proc.HasExited) { break }
        $treeBytes = Get-TreeWorkingSetBytes -RootPid $proc.Id
        if ($treeBytes -gt $limitBytes) {
            [Console]::Error.WriteLine("caproom: process tree of pid $($proc.Id) using $([math]::Round($treeBytes/1MB))MB exceeded ${LimitMb}MB cap -- killing tree")
            & taskkill.exe /PID $proc.Id /T /F 2>$null | Out-Null
            exit 137
        }
    }
    exit (Wait-CappedProcess $proc -Offsets $offsets)
}

# ---- argument parsing ----

if ($args.Count -eq 0) { Show-Usage }

$script:CaproomNtLoaded = $false
function Ensure-NtSuspend {
    # Whole-tree park needs NtSuspendProcess/NtResumeProcess (ntdll) --
    # the Windows analogue of kill -STOP/-CONT. Loaded lazily, once.
    if ($script:CaproomNtLoaded) { return }
    try {
        Add-Type -Namespace Caproom -Name Nt -MemberDefinition @'
[DllImport("ntdll.dll")] public static extern int NtSuspendProcess(IntPtr processHandle);
[DllImport("ntdll.dll")] public static extern int NtResumeProcess(IntPtr processHandle);
[DllImport("kernel32.dll", SetLastError=true)] public static extern IntPtr OpenProcess(int desiredAccess, bool inheritHandle, int processId);
[DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr handle);
'@
        $script:CaproomNtLoaded = $true
    } catch {
        [Console]::Error.WriteLine('caproom watch: cannot load ntdll suspend/resume -- --auto-park unavailable')
        throw
    }
}

function Get-CaproomSnapshot {
    # One CIM query -> ByPid map, Children map (only live parents), Roots
    # (pids whose parent is not in the snapshot). Mirrors posix read_snapshot.
    $procs = @(Get-CimInstance Win32_Process -Property ProcessId,ParentProcessId,Name,CommandLine,WorkingSetSize)
    $byPid = @{}
    foreach ($p in $procs) { $byPid[[int]$p.ProcessId] = $p }
    $children = @{}
    foreach ($p in $procs) {
        $ppid = [int]$p.ParentProcessId
        if ($byPid.ContainsKey($ppid)) {
            if (-not $children.ContainsKey($ppid)) { $children[$ppid] = New-Object System.Collections.Generic.List[int] }
            $children[$ppid].Add([int]$p.ProcessId)
        }
    }
    $roots = @($procs | Where-Object { -not $byPid.ContainsKey([int]$_.ParentProcessId) } | ForEach-Object { [int]$_.ProcessId })
    return @{ ByPid=$byPid; Children=$children; Roots=$roots }
}

function Get-TreeStats {
    param([hashtable]$Snap, [int]$RootPid)
    $rss = [long]0
    $pids = New-Object System.Collections.Generic.List[int]
    $stack = New-Object System.Collections.Generic.Stack[int]
    $seen = @{}
    $stack.Push($RootPid)
    while ($stack.Count -gt 0) {
        $cur = $stack.Pop()
        if ($seen.ContainsKey($cur)) { continue }
        $seen[$cur] = $true
        if (-not $Snap.ByPid.ContainsKey($cur)) { continue }
        $proc = $Snap.ByPid[$cur]
        if ($proc.WorkingSetSize) { $rss += [long]$proc.WorkingSetSize }
        $pids.Add($cur)
        if ($Snap.Children.ContainsKey($cur)) { foreach ($c in $Snap.Children[$cur]) { $stack.Push($c) } }
    }
    return @{ RssKb = [long]($rss / 1KB); Pids = $pids }
}

function Invoke-Top {
    # schema:1 rows identical in shape to the POSIX build. One honest
    # divergence: Windows exposes no cheap sleep-state, so state is always
    # 'running' and park_candidate keys off tree size alone -- the reason
    # string says so instead of pretending a sleep check happened.
    param([int]$FilterPid = 0, [int]$ParkMinMb = 512)
    $snap = Get-CaproomSnapshot
    $ts = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    $limitMb = 4096; if ($env:CAPROOM_LIMIT_MB) { $limitMb = [int]$env:CAPROOM_LIMIT_MB }
    $parkMinKb = [long]$ParkMinMb * 1024
    $rows = New-Object System.Collections.Generic.List[object]
    foreach ($root in $snap.Roots) {
        if ($FilterPid -ne 0 -and $root -ne $FilterPid) { continue }
        $st = Get-TreeStats -Snap $snap -RootPid $root
        $cmd = ''
        if ($snap.ByPid.ContainsKey($root)) {
            $procRow = $snap.ByPid[$root]
            if ($procRow.CommandLine) { $cmd = [string]$procRow.CommandLine } else { $cmd = [string]$procRow.Name }
        }
        $cand = $false; $reason = ''
        if ([long]$st.RssKb -ge $parkMinKb) {
            $cand = $true
            $reason = "tree_rss $($st.RssKb)KB >= ${parkMinKb}KB park threshold (win32: no sleep-state check)"
        }
        $rows.Add([pscustomobject]@{
            pid = $root; cmd = $cmd; tree_rss_kb = $st.RssKb
            tree_pids = @($st.Pids.ToArray()); state = 'running'
            park_candidate = $cand; reason = $reason
        })
    }
    $envelope = [pscustomobject]@{ schema = 1; ts = $ts; limit_mb_default = $limitMb; processes = @($rows.ToArray()) }
    ConvertTo-Json -Compress -Depth 6 -InputObject $envelope
}

function Invoke-Watch {
    # Same NDJSON contract as the POSIX watcher (schema:1 events on stdout
    # under --json). Explicit pids only; naming the pid IS the opt-in for
    # --auto-park, same rule as POSIX.
    # NOTE: args arrive via the named -Rest parameter, NOT $args — splatting
    # into $args proved unreliable here (every element read back as null,
    # yielding pids [0,0,0,0] and a busy-spinning interval-0 loop).
    param([string[]]$Rest)
    $thresholdMb = 2048; $intervalSec = 5.0; $auto = $false; $wake = -1.0; $json = $false
    $targets = New-Object System.Collections.Generic.List[int]
    for ($i = 0; $i -lt $Rest.Count; $i++) {
        switch ($Rest[$i]) {
            '--threshold-mb'        { $thresholdMb = [int]$Rest[$i + 1]; $i++ }
            '--interval'            { $intervalSec = [double]$Rest[$i + 1]; $i++ }
            '--auto-park'           { $auto = $true }
            '--auto-wake-free-pct'  { $wake = [double]$Rest[$i + 1]; $i++ }
            '--json'                { $json = $true }
            '--'                    { }
            default {
                try { $targets.Add([int]$Rest[$i]) }
                catch { [Console]::Error.WriteLine("caproom: unknown watch arg $($Rest[$i])"); exit 1 }
            }
        }
    }
    if ($targets.Count -eq 0) {
        [Console]::Error.WriteLine('usage: caproom watch [--threshold-mb <mb>] [--interval <sec>] [--auto-park] [--auto-wake-free-pct <pct>] [--json] <pid...>')
        exit 1
    }
    if ($intervalSec -lt 0.5) { $intervalSec = 0.5 }

    function Emit([object]$Ev) {
        [Console]::Out.WriteLine((ConvertTo-Json -Compress -Depth 6 -InputObject $Ev))
    }

    $mode = 'watch'; if ($auto) { $mode = 'auto-park' }
    $ts0 = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    if ($json) {
        Emit ([pscustomobject]@{ schema = 1; event = 'started'; ts = $ts0; mode = $mode; threshold_kb = ($thresholdMb * 1024); pids = @($targets.ToArray()) })
    } else {
        $armed = ''; if ($auto) { $armed = ', AUTO-PARK ARMED' }
        [Console]::Error.WriteLine("caproom: watching $($targets.Count) pid(s), tree threshold ${thresholdMb}MB, poll ${intervalSec}s$armed")
    }

    $parkedByUs = New-Object System.Collections.Generic.List[int]
    $breaching = @{}
    while ($true) {
        $liveSet = @{}
        foreach ($q in @(Get-CimInstance Win32_Process -Property ProcessId)) { $liveSet[[int]$q.ProcessId] = $true }
        $alive = New-Object System.Collections.Generic.List[int]
        foreach ($tpid in $targets) { if ($liveSet.ContainsKey($tpid)) { $alive.Add($tpid) } }
        if ($alive.Count -eq 0) {
            if ($json) { Emit ([pscustomobject]@{ schema = 1; event = 'all-exited'; ts = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds() }) }
            [Console]::Error.WriteLine('caproom: watch: all watched pids exited')
            exit 0
        }
        $targets = $alive

        if ($wake -ge 0 -and $parkedByUs.Count -gt 0) {
            $os = Get-CimInstance Win32_OperatingSystem
            $pct = [int]($os.FreePhysicalMemory * 100 / $os.TotalVisibleMemorySize)
            if ($pct -ge $wake) {
                Ensure-NtSuspend
                foreach ($wpid in @($parkedByUs.ToArray())) {
                    if (-not $liveSet.ContainsKey($wpid)) { continue }
                    $h = [Caproom.Nt]::OpenProcess(0x0800, $false, $wpid)
                    if ($h -ne [IntPtr]::Zero) {
                        [void][Caproom.Nt]::NtResumeProcess($h); [void][Caproom.Nt]::CloseHandle($h)
                        if ($json) { Emit ([pscustomobject]@{ schema = 1; event = 'woke'; ts = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds(); pid = $wpid; free_pct = $pct }) }
                        else { [Console]::Error.WriteLine("caproom: watch: free mem ${pct}% >= ${wake}% -- resuming pid $wpid") }
                    }
                }
                $parkedByUs.Clear()
            }
        }

        $snap = Get-CaproomSnapshot
        $threshKb = [long]$thresholdMb * 1024
        foreach ($tpid in $targets) {
            if (-not $snap.ByPid.ContainsKey($tpid)) { continue }
            $st = Get-TreeStats -Snap $snap -RootPid $tpid
            if ([long]$st.RssKb -ge $threshKb) {
                if ($breaching.ContainsKey($tpid)) { continue }
                $breaching[$tpid] = $true
                $now = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
                if ($auto) {
                    Ensure-NtSuspend
                    $stopped = 0
                    foreach ($cp in $st.Pids) {
                        $h = [Caproom.Nt]::OpenProcess(0x0800, $false, $cp)
                        if ($h -ne [IntPtr]::Zero) {
                            [void][Caproom.Nt]::NtSuspendProcess($h); [void][Caproom.Nt]::CloseHandle($h)
                            $parkedByUs.Add($cp); $stopped++
                        }
                    }
                    if ($json) { Emit ([pscustomobject]@{ schema = 1; event = 'parked'; ts = $now; pid = $tpid; tree_rss_kb = $st.RssKb; tree_pids = @($st.Pids.ToArray()); stopped = $stopped }) }
                    else { [Console]::Error.WriteLine("caproom: watch: tree of pid $tpid hit $($st.RssKb)KB (>= $([int]$threshKb)KB) -- PARKED tree ($stopped pids)") }
                } else {
                    if ($json) { Emit ([pscustomobject]@{ schema = 1; event = 'breach'; ts = $now; pid = $tpid; tree_rss_kb = $st.RssKb }) }
                    else { [Console]::Error.WriteLine("caproom: watch: tree of pid $tpid hit $($st.RssKb)KB (>= $([int]$threshKb)KB) -- no --auto-park, reporting only") }
                }
            } else {
                if ($breaching.ContainsKey($tpid)) {
                    $breaching.Remove($tpid)
                    $now2 = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
                    if ($json) { Emit ([pscustomobject]@{ schema = 1; event = 'recovered'; ts = $now2; pid = $tpid; tree_rss_kb = $st.RssKb }) }
                    else { [Console]::Error.WriteLine("caproom: watch: pid $tpid back under threshold ($($st.RssKb)KB)") }
                }
            }
        }
        Start-Sleep -Seconds $intervalSec
    }
}

function Invoke-Setup {
    # Bind headroom management to PowerShell sessions in ANY terminal:
    # writes ~/.caproom/shell.ps1 (single source) and marker-patches
    # $PROFILE. Idempotent, backs up the profile, reversible via
    # `caproom setup --uninstall`. Never runs on npm install.
    $dir = Join-Path $HOME '.caproom'
    New-Item -ItemType Directory -Force -Path $dir | Out-Null

    @'
# caproom PowerShell integration -- regenerated by `caproom setup`.
function global:caproom_freemem_pct {
    $os = Get-CimInstance Win32_OperatingSystem
    [int]($os.FreePhysicalMemory * 100 / $os.TotalVisibleMemorySize)
}
$global:__caproomLastWarn = 0
function global:prompt {
    try {
        $pct = caproom_freemem_pct
        $now = [DateTimeOffset]::Now.ToUnixTimeSeconds()
        $warn = if ($env:CAPROOM_HEADROOM_WARN) { [int]$env:CAPROOM_HEADROOM_WARN } else { 20 }
        if ($pct -lt $warn -and ($now - $script:__caproomLastWarn) -ge 60) {
            $script:__caproomLastWarn = $now
            Write-Host "caproom: headroom low ($pct% free) - check 'caproom top' before launching heavy work" -ForegroundColor Yellow
        }
    } catch {}
    "PS $($executionContext.SessionState.Path.CurrentLocation)> "
}
'@ | Set-Content -Encoding UTF8 (Join-Path $dir 'shell.ps1')

    $profilePath = $PROFILE.CurrentUserAllHosts
    if (-not (Test-Path $profilePath)) { New-Item -ItemType File -Force -Path $profilePath | Out-Null }
    $content = Get-Content $profilePath -Raw -ErrorAction SilentlyContinue
    if ($content -notmatch '# >>> caproom >>>') {
        Copy-Item $profilePath "$profilePath.caproom.bak.$(Get-Date -Format yyyyMMddHHmmss)"
        Add-Content $profilePath @'

# >>> caproom >>>
. "$HOME\.caproom\shell.ps1"
# <<< caproom <<<
'@
        [Console]::Error.WriteLine("caproom setup: patched $profilePath (backup alongside)")
    } else {
        [Console]::Error.WriteLine('caproom setup: profile already bound')
    }
    [Console]::Error.WriteLine('caproom setup: shell.ps1 written to ' + $dir + ' -- new terminals pick it up automatically')
}

switch ($args[0]) {
    'help'   { Show-Usage -AsHelp }
    '-h'     { Show-Usage -AsHelp }
    '--help' { Show-Usage -AsHelp }
    'setup'  {
        Invoke-Setup; exit 0
    }
    'bind'   {
        Invoke-Setup; exit 0
    }
    'freemem' {
        $os = Get-CimInstance Win32_OperatingSystem
        Write-Output ([int]($os.FreePhysicalMemory * 100 / $os.TotalVisibleMemorySize))
        exit 0
    }
    'top' {
        $fpid = 0; $parkMin = 512
        for ($i = 1; $i -lt $args.Count; $i++) {
            switch ($args[$i]) {
                '--json'         { }
                '--pid'          { $fpid = [int]$args[$i + 1]; $i++ }
                '--park-min-mb'  { $parkMin = [int]$args[$i + 1]; $i++ }
                default { [Console]::Error.WriteLine("caproom: unknown top flag $($args[$i])"); exit 1 }
            }
        }
        Invoke-Top -FilterPid $fpid -ParkMinMb $parkMin
        exit 0
    }
    'watch' {
        Invoke-Watch -Rest @($args | Select-Object -Skip 1)
        exit 0
    }
    'park'   {
        if ($args.Count -lt 2) { [Console]::Error.WriteLine('usage: caproom park <pid>'); exit 1 }
        Invoke-Park -TargetPid ([int]$args[1]); exit 0
    }
    'wake'   {
        if ($args.Count -lt 2) { [Console]::Error.WriteLine('usage: caproom wake <pid>'); exit 1 }
        Invoke-Wake -TargetPid ([int]$args[1]); exit 0
    }
    'status' {
        if ($args.Count -lt 2) { [Console]::Error.WriteLine('usage: caproom status <pid>'); exit 1 }
        Invoke-Status -TargetPid ([int]$args[1]); exit 0
    }
    'guard'  {
        if ($args.Count -lt 2) { [Console]::Error.WriteLine('usage: caproom guard [--threshold <pct>] [--interval <sec>] <pid...>'); exit 1 }
        $threshold = 10
        $gInterval = 5
        $gPids = @()
        for ($i = 1; $i -lt $args.Count; $i++) {
            if ($args[$i] -eq '--threshold') { $threshold = [int]$args[$i + 1]; $i++ }
            elseif ($args[$i] -eq '--interval') { $gInterval = [double]$args[$i + 1]; $i++ }
            elseif ($args[$i] -eq '--') { continue }
            else { $gPids += [int]$args[$i] }
        }
        if ($gPids.Count -eq 0) { [Console]::Error.WriteLine('usage: caproom guard [--threshold <pct>] [--interval <sec>] <pid...>'); exit 1 }
        Invoke-Guard -Threshold $threshold -Interval $gInterval -TargetPids $gPids
        exit 0
    }
    'init'   {
        if ($args.Count -lt 2) { [Console]::Error.WriteLine('usage: caproom init <command> [--limit <mb>]'); exit 1 }
        $target = $args[1]
        $limit = 4096
        for ($i = 2; $i -lt $args.Count; $i++) {
            if ($args[$i] -eq '--limit') { $limit = [int]$args[$i + 1]; $i++ }
            else { [Console]::Error.WriteLine("caproom: unknown init flag $($args[$i])"); exit 1 }
        }
        Invoke-Init -Target $target -LimitMb $limit
        exit 0
    }
}

$limitMb = if ($env:CAPROOM_LIMIT_MB) { [int]$env:CAPROOM_LIMIT_MB } else { 4096 }
$interval = if ($env:CAPROOM_INTERVAL) { [double]$env:CAPROOM_INTERVAL } else { 0.2 }
$forceWatchdog = $false
$i = 0
$parsing = $true
while ($parsing -and $i -lt $args.Count) {
    $a = $args[$i]
    if ($a -eq '--limit')                { $limitMb = [int]$args[$i + 1]; $i += 2 }
    elseif ($a -eq '--interval')         { $interval = [double]$args[$i + 1]; $i += 2 }
    elseif ($a -eq '--force-watchdog')   { $forceWatchdog = $true; $i++ }
    elseif ($a -eq '-h' -or $a -eq '--help') { Show-Usage -AsHelp }
    elseif ($a -eq '--')                 { $i++; $parsing = $false }
    else                                 { $parsing = $false }
}

if ($i -ge $args.Count) { Show-Usage }
$command = @($args[$i..($args.Count - 1)])

Invoke-Capped -LimitMb $limitMb -Interval $interval -ForceWatchdog $forceWatchdog -Command $command
