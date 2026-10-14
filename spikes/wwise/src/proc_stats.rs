//! Process resource snapshots (resident memory, threads, CPU time) for cost reports.

use std::time::Instant;

/// One snapshot of this process.
#[derive(Debug, Clone, Copy)]
pub struct Snapshot {
    /// When it was taken.
    pub at: Instant,
    /// Resident memory, bytes.
    pub resident: u64,
    /// Threads.
    pub threads: i32,
    /// User + system CPU time, seconds.
    pub cpu: f64,
}

/// Takes a snapshot. Falls back to zeros if the kernel refuses.
#[must_use]
pub fn snapshot() -> Snapshot {
    let at = Instant::now();
    let mut info: libc::proc_taskinfo = unsafe_zeroed();
    let size = i32::try_from(size_of::<libc::proc_taskinfo>()).unwrap_or(i32::MAX);
    // SAFETY: `info` is a properly sized, writable proc_taskinfo; the kernel fills at most
    // `size` bytes.
    let n = unsafe {
        libc::proc_pidinfo(libc::getpid(), libc::PROC_PIDTASKINFO, 0, (&raw mut info).cast(), size)
    };
    let (resident, threads) =
        if n == size { (info.pti_resident_size, info.pti_threadnum) } else { (0, 0) };
    let mut ru: libc::rusage = unsafe_zeroed();
    // SAFETY: `ru` is a writable rusage.
    let ok = unsafe { libc::getrusage(libc::RUSAGE_SELF, &raw mut ru) } == 0;
    let secs = |t: libc::timeval| t.tv_sec as f64 + f64::from(t.tv_usec) / 1e6;
    let cpu = if ok { secs(ru.ru_utime) + secs(ru.ru_stime) } else { 0.0 };
    Snapshot { at, resident, threads, cpu }
}

unsafe extern "C" {
    fn mach_port_deallocate(
        task: libc::mach_port_t,
        name: libc::mach_port_t,
    ) -> libc::kern_return_t;
}

/// One thread of this process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadInfo {
    /// `pthread` name ("" if unnamed; Wwise doesn't name its threads on macOS).
    pub name: String,
    /// Current scheduling priority (Mach; 31 = default, 47 = Wwise's high-priority
    /// threads if raised, 97 = CoreAudio real-time IO).
    pub priority: i32,
    /// User + system CPU time, ms.
    pub cpu_ms: u64,
}

/// This process's threads (Mach `THREAD_EXTENDED_INFO`).
#[must_use]
#[allow(deprecated)] // libc's mach_task_self; the mach2 crate isn't worth a dependency here
pub fn threads() -> Vec<ThreadInfo> {
    let mut out = Vec::new();
    // SAFETY: standard Mach enumeration of our own task's threads. The returned array and
    // each thread port right are released below; `info` is a writable, correctly sized
    // thread_extended_info and `count` says so.
    unsafe {
        let task = libc::mach_task_self();
        let mut list: libc::thread_act_array_t = std::ptr::null_mut();
        let mut n: libc::mach_msg_type_number_t = 0;
        if libc::task_threads(task, &raw mut list, &raw mut n) != libc::KERN_SUCCESS {
            return out;
        }
        for i in 0..n as usize {
            let port = *list.add(i);
            let mut info: libc::thread_extended_info = unsafe_zeroed();
            let mut count = libc::THREAD_EXTENDED_INFO_COUNT;
            let kr = libc::thread_info(
                port,
                libc::THREAD_EXTENDED_INFO as libc::thread_flavor_t,
                (&raw mut info).cast(),
                &raw mut count,
            );
            if kr == libc::KERN_SUCCESS {
                out.push(ThreadInfo {
                    name: std::ffi::CStr::from_ptr(info.pth_name.as_ptr())
                        .to_string_lossy()
                        .into_owned(),
                    priority: info.pth_curpri,
                    cpu_ms: (info.pth_user_time + info.pth_system_time) / 1_000_000,
                });
            }
            mach_port_deallocate(task, port);
        }
        libc::vm_deallocate(
            task,
            list as libc::vm_address_t,
            n as usize * size_of::<libc::thread_act_t>(),
        );
    }
    out
}

/// Short form for reports: `name/priority/cpu`.
#[must_use]
pub fn describe_threads() -> String {
    threads()
        .iter()
        .map(|t| {
            format!(
                "{}/pri {}/{} ms",
                if t.name.is_empty() { "?" } else { &t.name },
                t.priority,
                t.cpu_ms
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Paths of loaded images (dylibs, bundles) whose file name contains `needle`.
#[must_use]
#[allow(deprecated)] // libc points at the mach2 crate; not worth a dependency here
pub fn loaded_images(needle: &str) -> Vec<String> {
    use libc::{_dyld_get_image_name, _dyld_image_count};
    // SAFETY: dyld's image list; indices below the count are valid, and names are static
    // NUL-terminated strings (or null for an image being unloaded, which we skip).
    unsafe {
        (0.._dyld_image_count())
            .filter_map(|i| {
                let p = _dyld_get_image_name(i);
                (!p.is_null()).then(|| std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned())
            })
            .filter(|n| n.rsplit('/').next().is_some_and(|f| f.contains(needle)))
            .collect()
    }
}

fn unsafe_zeroed<T: Copy>() -> T {
    // SAFETY: only used for plain C structs (proc_taskinfo, rusage) where all-zero bytes is
    // a valid value.
    unsafe { std::mem::zeroed() }
}

impl Snapshot {
    /// CPU used between `earlier` and `self`, as a percentage of one core.
    #[must_use]
    pub fn cpu_percent_since(&self, earlier: &Self) -> f64 {
        let wall = self.at.duration_since(earlier.at).as_secs_f64();
        if wall <= 0.0 { 0.0 } else { 100.0 * (self.cpu - earlier.cpu) / wall }
    }

    /// Resident memory in MiB.
    #[must_use]
    pub fn resident_mib(&self) -> f64 {
        self.resident as f64 / (1024.0 * 1024.0)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn snapshot_reports_this_process() {
        let s = super::snapshot();
        assert!(s.resident > 0);
        assert!(s.threads >= 1);
    }
}
