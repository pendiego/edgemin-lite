use crate::config::Config;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::ProcessStatus::{
    EmptyWorkingSet, GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
};

pub struct GroupStat {
    pub name: String,
    pub procs: u32,
    pub bytes: u64,
}

pub fn should_trim(total_bytes: u64, limit_enabled: bool, limit_mb: u64) -> bool {
    !limit_enabled || total_bytes > limit_mb.saturating_mul(1024 * 1024)
}

fn for_each_process(mut f: impl FnMut(u32, &str)) {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e);
        while ok != 0 {
            let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            f(e.th32ProcessID, &String::from_utf16_lossy(&e.szExeFile[..n]));
            ok = Process32NextW(snap, &mut e);
        }
        CloseHandle(snap);
    }
}

pub fn process_exists(name: &str) -> bool {
    let mut found = false;
    for_each_process(|_, n| found |= n.eq_ignore_ascii_case(name));
    found
}

pub fn run_tick(cfg: &Config) -> Vec<GroupStat> {
    let mut names: Vec<&str> = vec![cfg.core_exe()];
    if cfg.extended_enabled {
        for n in &cfg.extended {
            if !names.iter().any(|x| x.eq_ignore_ascii_case(n)) {
                names.push(n);
            }
        }
    }
    let mut pids: Vec<Vec<u32>> = vec![Vec::new(); names.len()];
    for_each_process(|pid, exe| {
        for (i, n) in names.iter().enumerate() {
            if exe.eq_ignore_ascii_case(n) {
                pids[i].push(pid);
            }
        }
    });

    let mut out = Vec::new();
    for (name, group) in names.iter().zip(pids) {
        let mut handles: Vec<HANDLE> = Vec::new();
        let mut sum = 0u64;
        for pid in group {
            unsafe {
                let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_QUOTA, 0, pid);
                if h.is_null() {
                    continue;
                }
                let mut c: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
                c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
                if GetProcessMemoryInfo(h, &mut c, c.cb) != 0 {
                    sum += c.WorkingSetSize as u64;
                }
                handles.push(h);
            }
        }
        let trim = cfg.enabled && should_trim(sum, cfg.limit_enabled, cfg.limit_mb);
        for &h in &handles {
            unsafe {
                if trim {
                    EmptyWorkingSet(h);
                }
                CloseHandle(h);
            }
        }
        out.push(GroupStat { name: name.to_string(), procs: handles.len() as u32, bytes: sum });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_trim_cases() {
        assert!(should_trim(0, false, 100));
        assert!(!should_trim(100 << 20, true, 100));
        assert!(should_trim((100 << 20) + 1, true, 100));
        assert!(!should_trim(1, true, 100));
    }

    #[test]
    fn oversized_limit_never_trims() {
        assert!(!should_trim(u64::MAX - 1, true, 17_592_186_044_416)); // 2^44 MB wrapped to 0 before
        assert!(!should_trim(1 << 40, true, u64::MAX));
    }
}
