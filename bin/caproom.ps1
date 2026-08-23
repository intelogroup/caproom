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
       caproom init <command> [--limit <mb>]

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

env vars (override flags): CAPROOM_LIMIT_MB, CAPROOM_INTERVAL

examples:
  caproom --limit 2048 -- npm run build
  caproom park 12345
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

function Invoke-Init {
    param([string]$Target, [int]$LimitMb)
    @"
# caproom: auto-cap '$Target' -- added by 'caproom init $Target'
# override per-shell: `$env:CAPROOM_LIMIT_MB = 8192
function ${Target}_capped {
    `$limit = if (`$env:CAPROOM_LIMIT_MB) { `$env:CAPROOM_LIMIT_MB } else { $LimitMb }
    caproom --limit `$limit -- $Target @args
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

function Invoke-Capped {
    param([int]$LimitMb, [double]$Interval, [bool]$ForceWatchdog, [string[]]$Command)

    $exe = $Command[0]
    $rest = if ($Command.Length -gt 1) { ConvertTo-ArgString $Command[1..($Command.Length - 1)] } else { $null }
    $startArgs = @{ FilePath = $exe; PassThru = $true; NoNewWindow = $true }
    if ($rest) { $startArgs.ArgumentList = $rest }

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

            # Assign THIS process to the job before spawning. Children are
            # associated automatically at CreateProcess time, so the cap is live
            # before the child runs a single instruction -- assigning the child
            # after Start-Process leaves a window where it can already allocate.
            if (-not [Caproom]::AssignProcessToJobObject($job, [Diagnostics.Process]::GetCurrentProcess().Handle)) {
                throw "AssignProcessToJobObject failed (error $([Runtime.InteropServices.Marshal]::GetLastWin32Error()))"
            }

            [Console]::Error.WriteLine("caproom: job object backend, limit=${LimitMb}m (committed memory, kernel-enforced, includes child processes)")
            $proc = Start-Process @startArgs
            $proc.WaitForExit()
            $proc.Refresh()
            $code = $proc.ExitCode
            if ($null -eq $code) { $code = 1 }
            exit $code
        } catch {
            [Console]::Error.WriteLine("caproom: job object backend unavailable ($($_.Exception.Message)) -- falling back to watchdog")
        }
    }

    [Console]::Error.WriteLine("caproom: watchdog backend, limit=${LimitMb}m poll=${Interval}s (hard kill on breach -- Windows has no SIGTERM equivalent)")
    $limitBytes = [uint64]$LimitMb * 1MB
    $proc = Start-Process @startArgs
    while (-not $proc.HasExited) {
        $proc.Refresh()
        if (-not $proc.HasExited -and $proc.WorkingSet64 -gt $limitBytes) {
            [Console]::Error.WriteLine("caproom: pid $($proc.Id) working set $([math]::Round($proc.WorkingSet64/1MB))MB exceeded ${LimitMb}MB cap -- killing")
            Stop-Process -Id $proc.Id -Force
            exit 137
        }
        Start-Sleep -Seconds $Interval
    }
    $proc.Refresh()
    $code = $proc.ExitCode
    if ($null -eq $code) { $code = 1 }
    exit $code
}

# ---- argument parsing ----

if ($args.Count -eq 0) { Show-Usage }

switch ($args[0]) {
    'help'   { Show-Usage -AsHelp }
    '-h'     { Show-Usage -AsHelp }
    '--help' { Show-Usage -AsHelp }
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
