use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use anyhow::Result;

static TOTAL_PHYSICAL_MEMORY_BYTES: OnceLock<u64> = OnceLock::new();
static PROCESS_MEMORY_CACHE: OnceLock<Mutex<Option<(Instant, u64)>>> = OnceLock::new();
const PROCESS_MEMORY_CACHE_TTL: Duration = Duration::from_millis(1500);

/// Returns the current process physical memory footprint (or RSS / WorkingSet) in bytes.
/// Results are cached briefly to avoid invoking OS memory syscalls on every UI frame.
pub fn current_process_memory_bytes() -> u64 {
    let cache = PROCESS_MEMORY_CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(guard) = cache.lock()
        && let Some((captured_at, bytes)) = *guard
        && captured_at.elapsed() < PROCESS_MEMORY_CACHE_TTL
    {
        return bytes;
    }
    let measured = read_current_process_memory_bytes();
    if let Ok(mut guard) = cache.lock() {
        *guard = Some((Instant::now(), measured));
    }
    measured
}

fn read_current_process_memory_bytes() -> u64 {
    #[cfg(target_os = "macos")]
    unsafe {
        let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
        let rc = libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V4,
            info.as_mut_ptr().cast::<libc::rusage_info_t>(),
        );
        if rc == 0 {
            let footprint = info.assume_init().ri_phys_footprint;
            if footprint > 0 {
                return footprint;
            }
        }

        let mut task_info = std::mem::MaybeUninit::<libc::mach_task_basic_info_data_t>::zeroed();
        let mut count = libc::MACH_TASK_BASIC_INFO_COUNT;
        #[allow(deprecated)]
        let task = libc::mach_task_self();
        let kr = libc::task_info(
            task,
            libc::MACH_TASK_BASIC_INFO,
            task_info.as_mut_ptr().cast::<libc::integer_t>(),
            &mut count,
        );
        if kr == libc::KERN_SUCCESS {
            let rss = task_info.assume_init().resident_size;
            if rss > 0 {
                return rss;
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if let Some(rest) = line.strip_prefix("VmRSS:") {
                    let kb_str = rest.trim().split_whitespace().next().unwrap_or("");
                    if let Ok(kb) = kb_str.parse::<u64>()
                        && kb > 0
                    {
                        return kb.saturating_mul(1024);
                    }
                }
            }
        }
        if let Ok(statm) = fs::read_to_string("/proc/self/statm")
            && let Some(res_pages_str) = statm.split_whitespace().nth(1)
            && let Ok(pages) = res_pages_str.parse::<u64>()
            && pages > 0
        {
            let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
            let page_bytes = if page_size > 0 {
                page_size as u64
            } else {
                4096
            };
            return pages.saturating_mul(page_bytes);
        }
    }
    #[cfg(target_os = "windows")]
    unsafe {
        #[repr(C)]
        struct ProcessMemoryCounters {
            cb: u32,
            page_fault_count: u32,
            peak_working_set_size: usize,
            working_set_size: usize,
            quota_peak_paged_pool_usage: usize,
            quota_paged_pool_usage: usize,
            quota_peak_non_paged_pool_usage: usize,
            quota_non_paged_pool_usage: usize,
            pagefile_usage: usize,
            peak_pagefile_usage: usize,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentProcess() -> *mut std::ffi::c_void;
            fn K32GetProcessMemoryInfo(
                process: *mut std::ffi::c_void,
                counters: *mut ProcessMemoryCounters,
                cb: u32,
            ) -> i32;
        }
        let mut counters = std::mem::MaybeUninit::<ProcessMemoryCounters>::zeroed();
        let cb = std::mem::size_of::<ProcessMemoryCounters>() as u32;
        if K32GetProcessMemoryInfo(GetCurrentProcess(), counters.as_mut_ptr(), cb) != 0 {
            let ws = counters.assume_init().working_set_size as u64;
            if ws > 0 {
                return ws;
            }
        }
    }
    0
}

/// Returns the host total physical memory capacity in bytes.
pub fn total_physical_memory_bytes() -> u64 {
    *TOTAL_PHYSICAL_MEMORY_BYTES.get_or_init(read_total_physical_memory_bytes)
}

fn read_total_physical_memory_bytes() -> u64 {
    #[cfg(target_os = "macos")]
    unsafe {
        let mut mib = [libc::CTL_HW, libc::HW_MEMSIZE];
        let mut memsize: u64 = 0;
        let mut len = std::mem::size_of::<u64>();
        let rc = libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            (&raw mut memsize).cast::<libc::c_void>(),
            &mut len,
            std::ptr::null_mut(),
            0,
        );
        if rc == 0 && memsize > 0 {
            return memsize;
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(meminfo) = fs::read_to_string("/proc/meminfo") {
            for line in meminfo.lines() {
                if let Some(rest) = line.strip_prefix("MemTotal:") {
                    let kb_str = rest.trim().split_whitespace().next().unwrap_or("");
                    if let Ok(kb) = kb_str.parse::<u64>()
                        && kb > 0
                    {
                        return kb.saturating_mul(1024);
                    }
                }
            }
        }
        unsafe {
            let pages = libc::sysconf(libc::_SC_PHYS_PAGES);
            let page_size = libc::sysconf(libc::_SC_PAGESIZE);
            if pages > 0 && page_size > 0 {
                return (pages as u64).saturating_mul(page_size as u64);
            }
        }
    }
    #[cfg(target_os = "windows")]
    unsafe {
        #[repr(C)]
        struct MemoryStatusEx {
            length: u32,
            memory_load: u32,
            total_phys: u64,
            avail_phys: u64,
            total_page_file: u64,
            avail_page_file: u64,
            total_virtual: u64,
            avail_virtual: u64,
            avail_extended_virtual: u64,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
        }
        let mut status = std::mem::MaybeUninit::<MemoryStatusEx>::zeroed();
        (*status.as_mut_ptr()).length = std::mem::size_of::<MemoryStatusEx>() as u32;
        if GlobalMemoryStatusEx(status.as_mut_ptr()) != 0 {
            let total = status.assume_init().total_phys;
            if total > 0 {
                return total;
            }
        }
    }
    0
}

/// Measures the total disk size of a `.mio` SQLite project file including `-wal` and `-shm` sidecars.
pub fn measure_project_disk_bytes(path: &Path) -> Result<u64> {
    let mut total = fs::metadata(path)?.len();
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        match fs::metadata(PathBuf::from(sidecar)) {
            Ok(metadata) => total = total.saturating_add(metadata.len()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_memory_and_project_disk_bytes_report_real_values() {
        let mem = current_process_memory_bytes();
        let total_mem = total_physical_memory_bytes();
        assert!(
            mem > 1024 * 1024,
            "expected >1 MiB process memory, got {mem}"
        );
        assert!(
            total_mem >= mem && total_mem > 256 * 1024 * 1024,
            "expected physical memory >= process memory ({mem}) and >256 MiB, got {total_mem}"
        );

        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("sample.mio");
        let wal = dir.path().join("sample.mio-wal");
        let shm = dir.path().join("sample.mio-shm");
        fs::write(&main, vec![b'a'; 4096]).unwrap();
        fs::write(&wal, vec![b'b'; 2048]).unwrap();
        fs::write(&shm, vec![b'c'; 512]).unwrap();
        assert_eq!(
            measure_project_disk_bytes(&main).unwrap(),
            4096 + 2048 + 512
        );
    }
}
