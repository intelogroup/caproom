use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcInfo {
    pub pid: i32,
    pub ppid: i32,
    pub pgid: i32,
    /// session ID (getsid), 0 if unknown — for is_session_leader = pid == sid
    #[serde(default)]
    pub sid: i32,
    /// process start time (macOS pbi_start_tvsec*1e6+usec or Linux starttime ticks) — for PID reuse guard (pid, start)
    #[serde(default)]
    pub start_time: u64,
    /// phys_footprint (macOS) or PSS/RSS fallback, KB
    pub footprint_kb: u64,
    /// total cpu time user+system in nanoseconds (monotonic, for delta)
    #[serde(default)]
    pub cpu_time_ns: u64,
    /// state char: R/S/I/T/Z
    pub state: char,
    pub cmd: String,
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub procs: Vec<ProcInfo>,
}

impl Snapshot {
    pub fn ppid_map(&self) -> std::collections::HashMap<i32, i32> {
        self.procs.iter().map(|p| (p.pid, p.ppid)).collect()
    }
    pub fn by_pid(&self, pid: i32) -> Option<&ProcInfo> {
        self.procs.iter().find(|p| p.pid == pid)
    }
}

/// Snapshot via libproc on macOS, /proc on Linux, ps fallback.
/// Week 1: libproc FFI. Until then, ps fallback keeps CLI functional.
pub fn snapshot_current_user() -> Snapshot {
    #[cfg(target_os = "macos")]
    {
        if let Some(s) = snapshot_libproc() {
            return s;
        }
    }
    snapshot_ps()
}

#[cfg(target_os = "macos")]
fn snapshot_libproc() -> Option<Snapshot> {
    use std::ffi::CStr;
    use std::mem::MaybeUninit;
    use std::os::raw::{c_int, c_void};

    const PROC_PIDTBSDINFO: c_int = 3;
    const RUSAGE_INFO_V4: c_int = 4;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ProcBsdInfo {
        pbi_flags: u32,
        pbi_status: u32,
        pbi_xstatus: u32,
        pbi_pid: u32,
        pbi_ppid: u32,
        pbi_uid: u32,
        pbi_gid: u32,
        pbi_ruid: u32,
        pbi_rgid: u32,
        pbi_svuid: u32,
        pbi_svgid: u32,
        rfu_1: u32,
        pbi_comm: [u8; 16],
        pbi_name: [u8; 32],
        pbi_nfiles: u32,
        pbi_pgid: u32,
        pbi_pjobc: u32,
        e_tdev: u32,
        e_tpgid: u32,
        pbi_nice: i32,
        pbi_start_tvsec: u64,
        pbi_start_tvusec: u64,
    }

    #[repr(C)]
    struct RusageInfoV4 {
        ri_uuid: [u8; 16],
        ri_user_time: u64,
        ri_system_time: u64,
        ri_pkg_idle_wkups: u64,
        ri_interrupt_wkups: u64,
        ri_pageins: u64,
        ri_wired_size: u64,
        ri_resident_size: u64,
        ri_phys_footprint: u64,
        ri_proc_start_abstime: u64,
        ri_proc_exit_abstime: u64,
        ri_child_user_time: u64,
        ri_child_system_time: u64,
        ri_child_pkg_idle_wkups: u64,
        ri_child_interrupt_wkups: u64,
        ri_child_pageins: u64,
        ri_child_elapsed_abstime: u64,
        ri_diskio_bytesread: u64,
        ri_diskio_byteswritten: u64,
        ri_cpu_time_qos_default: u64,
        ri_cpu_time_qos_maintenance: u64,
        ri_cpu_time_qos_background: u64,
        ri_cpu_time_qos_utility: u64,
        ri_cpu_time_qos_legacy: u64,
        ri_cpu_time_qos_user_initiated: u64,
        ri_cpu_time_qos_user_interactive: u64,
        ri_billed_system_time: u64,
        ri_serviced_system_time: u64,
        ri_logical_writes: u64,
        ri_lifetime_max_phys_footprint: u64,
        ri_instructions: u64,
        ri_cycles: u64,
        ri_billed_energy: u64,
        ri_serviced_energy: u64,
        ri_interval_max_phys_footprint: u64,
        ri_runnable_time: u64,
    }

    extern "C" {
        fn proc_listallpids(buffer: *mut c_void, buffersize: c_int) -> c_int;
        fn proc_pidinfo(
            pid: c_int,
            flavor: c_int,
            arg: u64,
            buffer: *mut c_void,
            buffersize: c_int,
        ) -> c_int;
        fn proc_pid_rusage(pid: c_int, flavor: c_int, buffer: *mut c_void) -> c_int;
        fn proc_pidpath(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int;
    }

    // 1. list all pids — returns count, not bytes (empirically 698 vs ps 695).
    // Grow-and-retry: a full system can exceed any fixed guess; truncation would
    // silently drop trees from enforcement.
    let mut cap = 8192usize;
    let pids_buf = loop {
        let mut buf = vec![0i32; cap];
        let count =
            unsafe { proc_listallpids(buf.as_mut_ptr() as *mut c_void, (buf.len() * 4) as c_int) };
        if count < 0 {
            return None;
        }
        let n = count as usize;
        if n < cap {
            buf.truncate(n);
            break buf;
        }
        cap *= 2;
    };

    let mut procs = Vec::with_capacity(pids_buf.len());
    let mut path_buf = vec![0u8; 4096];
    for pid in pids_buf {
        if pid <= 1 {
            continue;
        }
        let mut bsd = MaybeUninit::<ProcBsdInfo>::uninit();
        let ret = unsafe {
            proc_pidinfo(
                pid,
                PROC_PIDTBSDINFO,
                0,
                bsd.as_mut_ptr() as *mut c_void,
                std::mem::size_of::<ProcBsdInfo>() as c_int,
            )
        };
        if ret != std::mem::size_of::<ProcBsdInfo>() as c_int {
            continue;
        }
        let bsd = unsafe { bsd.assume_init() };

        // phys_footprint via rusage — fallback to TASKINFO resident if rusage fails (perm)
        let mut ru = MaybeUninit::<RusageInfoV4>::uninit();
        let rret = unsafe { proc_pid_rusage(pid, RUSAGE_INFO_V4, ru.as_mut_ptr() as *mut c_void) };
        let (footprint_kb, cpu_time_ns) = if rret == 0 {
            let ru = unsafe { ru.assume_init() };
            (
                ru.ri_phys_footprint / 1024,
                ru.ri_user_time + ru.ri_system_time,
            )
        } else {
            // fallback: try taskinfo resident_size, else skip
            const PROC_PIDTASKINFO: c_int = 4;
            #[repr(C)]
            struct ProcTaskInfo {
                pti_virtual_size: u64,
                pti_resident_size: u64,
                pti_total_user: u64,
                pti_total_system: u64,
                pti_threads_user: u64,
                pti_threads_system: u64,
                pti_policy: i32,
                pti_faults: i32,
                pti_pageins: i32,
                pti_cow_faults: i32,
                pti_messages_sent: i32,
                pti_messages_received: i32,
                pti_syscalls_mach: i32,
                pti_syscalls_unix: i32,
                pti_csw: i32,
                pti_threadnum: i32,
                pti_numrunning: i32,
                pti_priority: i32,
            }
            let mut ti = MaybeUninit::<ProcTaskInfo>::uninit();
            let tret = unsafe {
                proc_pidinfo(
                    pid,
                    PROC_PIDTASKINFO,
                    0,
                    ti.as_mut_ptr() as *mut c_void,
                    std::mem::size_of::<ProcTaskInfo>() as c_int,
                )
            };
            if tret == std::mem::size_of::<ProcTaskInfo>() as c_int {
                let ti = unsafe { ti.assume_init() };
                (
                    ti.pti_resident_size / 1024,
                    ti.pti_total_user + ti.pti_total_system,
                )
            } else {
                continue;
            }
        };

        // R vs S: pbi_status lies — reports SRUN for processes blocked in
        // mach_msg/time.sleep. Use TASKINFO's live thread-run count instead.
        const PROC_PIDTASKINFO: c_int = 4;
        #[repr(C)]
        struct TaskRunState {
            _pad: [u64; 2], // virtual+resident size
            _cpu: [u64; 2], // total user+system
            _thr: [u64; 2], // threads user+system
            _policy: i32,
            _faults: [i32; 4],
            _msgs: [i32; 2],
            _sysc: [i32; 2],
            _csw: i32,
            threadnum: i32,
            numrunning: i32,
            _prio: i32,
        }
        let mut tr = MaybeUninit::<TaskRunState>::uninit();
        let trret = unsafe {
            proc_pidinfo(
                pid,
                PROC_PIDTASKINFO,
                0,
                tr.as_mut_ptr() as *mut c_void,
                std::mem::size_of::<TaskRunState>() as c_int,
            )
        };
        let num_running = if trret == std::mem::size_of::<TaskRunState>() as c_int {
            unsafe { tr.assume_init().numrunning }
        } else {
            -1
        };

        // state mapping: pbi_status 1=SIDL, 5=SZOMB, 4=SSTOP are trustworthy;
        // R/S decided by numrunning (0 threads on CPU => sleeping)
        let state_char = match bsd.pbi_status {
            5 => 'Z',
            4 => 'T',
            _ => {
                if num_running < 0 {
                    'S'
                }
                // taskinfo failed — assume sleeping
                else if num_running > 0 {
                    'R'
                } else {
                    'S'
                }
            }
        };

        // prefer proc_pidpath for full cmd, fallback to pbi_comm
        let plen = unsafe {
            proc_pidpath(
                pid,
                path_buf.as_mut_ptr() as *mut c_void,
                path_buf.len() as u32,
            )
        };
        let cmd = if plen > 0 {
            let cstr = unsafe { CStr::from_ptr(path_buf.as_ptr() as *const i8) };
            let s = cstr.to_string_lossy().into_owned();
            if s.is_empty() {
                String::from_utf8_lossy(&bsd.pbi_comm)
                    .trim_end_matches('\0')
                    .to_string()
            } else {
                s
            }
        } else {
            String::from_utf8_lossy(&bsd.pbi_comm)
                .trim_end_matches('\0')
                .to_string()
        };

        // sid via getsid (session leader = pid == sid), start via pbi_start
        #[cfg(unix)]
        let sid = {
            let r = unsafe { libc::getsid(pid) };
            if r < 0 {
                0
            } else {
                r as i32
            }
        };
        #[cfg(not(unix))]
        let sid: i32 = 0;
        let start_time = bsd.pbi_start_tvsec * 1_000_000 + bsd.pbi_start_tvusec;
        procs.push(ProcInfo {
            pid,
            ppid: bsd.pbi_ppid as i32,
            pgid: bsd.pbi_pgid as i32,
            sid,
            start_time,
            footprint_kb,
            cpu_time_ns,
            state: state_char,
            cmd,
        });
    }
    if procs.is_empty() {
        None
    } else {
        Some(Snapshot { procs })
    }
}

/// Parse /proc/[pid]/stat: fields after the comm field (which may contain
/// spaces/parens — hence rsplit on ')'). Returns (cpu_time_ns, start_time, sid).
/// Layout after ')': 0=state 1=ppid 2=pgrp 3=session 4=tty_nr 5=tpgid ...
/// 11=utime 12=stime ... 19=starttime.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_proc_stat(s: &str) -> (u64, u64, i32) {
    let after = s.rsplit(')').next().unwrap_or("");
    let f: Vec<&str> = after.split_whitespace().collect();
    if f.len() > 19 {
        let ut: u64 = f[11].parse().unwrap_or(0);
        let st: u64 = f[12].parse().unwrap_or(0);
        let sid_v: i32 = f[3].parse().unwrap_or(0);
        let start_v: u64 = f[19].parse().unwrap_or(0);
        ((ut + st) * 10_000_000, start_v, sid_v)
    } else {
        (0, 0, 0)
    }
}

fn snapshot_ps() -> Snapshot {
    use std::process::Command;
    let out = Command::new("ps")
        .args(["-eo", "pid=,ppid=,pgid=,rss=,state=,command="])
        .output();
    let Ok(out) = out else {
        return Snapshot::default();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut procs = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        if t.is_empty() {
            continue;
        }
        // split_whitespace then re-join command tail
        let toks: Vec<&str> = t.split_whitespace().collect();
        if toks.len() < 5 {
            continue;
        }
        let pid: i32 = toks[0].parse().unwrap_or(0);
        let ppid: i32 = toks[1].parse().unwrap_or(0);
        let pgid: i32 = toks[2].parse().unwrap_or(0);
        let rss: u64 = toks[3].parse().unwrap_or(0);
        let state = toks[4].chars().next().unwrap_or('?');
        // command is original line after the 5th token's position — recover verbatim tail
        let cmd = if toks.len() > 5 {
            toks[5..].join(" ")
        } else {
            String::new()
        };
        if pid == 0 {
            continue;
        }
        // Linux: derive cpu_time_ns + start_time + sid from /proc/[pid]/stat
        let (cpu_time_ns, start_time, sid) = {
            #[cfg(target_os = "linux")]
            {
                std::fs::read_to_string(format!("/proc/{}/stat", pid))
                    .ok()
                    .as_deref()
                    .map(parse_proc_stat)
                    .unwrap_or_default()
            }
            #[cfg(not(target_os = "linux"))]
            {
                // macOS fallback ps path also gets sid/start via getsid if needed
                #[cfg(unix)]
                let sid_raw = unsafe { libc::getsid(pid) };
                #[cfg(not(unix))]
                let sid_raw: i32 = -1;
                let sid = if sid_raw < 0 { 0 } else { sid_raw as i32 };
                let sid = if sid < 0 { 0 } else { sid as i32 };
                (0u64, 0u64, sid)
            }
        };
        procs.push(ProcInfo {
            pid,
            ppid,
            pgid,
            sid,
            start_time,
            footprint_kb: rss,
            cpu_time_ns,
            state,
            cmd,
        });
    }
    Snapshot { procs }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ps_snapshot_not_empty() {
        // ps is blocked in sandboxed env (Operation not permitted), so test the
        // real entry point snapshot_current_user which prefers libproc on macOS.
        // This exercises the FFI path that real consumers use.
        let s = snapshot_current_user();
        assert!(
            !s.procs.is_empty(),
            "snapshot_current_user empty — libproc and ps both failed"
        );
    }
    #[test]
    fn snapshot_current_user_has_ppid() {
        let s = snapshot_current_user();
        assert!(
            s.procs.iter().any(|p| p.ppid != 0),
            "ppid_map should be populated"
        );
    }
    #[test]
    fn proc_stat_parses_session_not_tpgid() {
        // synthetic stat line: comm contains spaces+parens to force rsplit path
        // fields after ')': state=S ppid=100 pgrp=200 session=300 tty_nr=7 tpgid=400 ...
        let line = "1234 (my hog) S 100 200 300 7 400 4194560 1 0 0 0 5 3 0 0 0 0 0 0 123456789 0";
        let (cpu_ns, start, sid) = parse_proc_stat(line);
        assert_eq!(sid, 300, "session is f[3], not f[4] (tpgid)");
        assert_eq!(cpu_ns, (5 + 3) * 10_000_000);
        assert_eq!(start, 123456789);
    }
    #[test]
    fn by_pid_and_ppid_map_deterministic() {
        let s = Snapshot {
            procs: vec![
                ProcInfo {
                    pid: 1,
                    ppid: 0,
                    pgid: 1,
                    sid: 1,
                    start_time: 0,
                    footprint_kb: 0,
                    cpu_time_ns: 0,
                    state: 'S',
                    cmd: "init".into(),
                },
                ProcInfo {
                    pid: 2,
                    ppid: 1,
                    pgid: 2,
                    sid: 2,
                    start_time: 1,
                    footprint_kb: 100,
                    cpu_time_ns: 10_000_000,
                    state: 'R',
                    cmd: "child".into(),
                },
            ],
        };
        assert!(s.by_pid(1).is_some());
        assert!(s.by_pid(99).is_none());
        let m = s.ppid_map();
        assert_eq!(m.get(&1), Some(&0));
        assert_eq!(m.get(&2), Some(&1));
    }
}
