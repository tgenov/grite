//! Host identity and process liveness helpers.
//!
//! Daemon locks record which host and which process wrote them. Both the
//! daemon and the CLI need the same answers, so the logic lives here rather
//! than being duplicated (and diverging) in each crate.

/// Get a stable identifier for this host.
///
/// Resolution order:
/// 1. `HOSTNAME` environment variable
/// 2. `gethostname(2)` (works on macOS, which has no `/etc/hostname`)
/// 3. `/etc/hostname`
/// 4. `"unknown-host"`
///
/// This must never fall back to a random value: a random host id makes every
/// lock look like it came from a foreign machine, which disables the
/// process-liveness check used for stale-lock recovery.
pub fn host_id() -> String {
    if let Ok(name) = std::env::var("HOSTNAME") {
        let name = name.trim().to_string();
        if !name.is_empty() {
            return name;
        }
    }

    #[cfg(unix)]
    if let Some(name) = gethostname() {
        return name;
    }

    if let Ok(contents) = std::fs::read_to_string("/etc/hostname") {
        let name = contents.trim().to_string();
        if !name.is_empty() {
            return name;
        }
    }

    "unknown-host".to_string()
}

#[cfg(unix)]
fn gethostname() -> Option<String> {
    let mut buf = vec![0u8; 256];
    // SAFETY: `buf` is a valid writable allocation of `buf.len()` bytes.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..end]).trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// Check whether a process with the given PID exists on this host.
///
/// Uses `kill(pid, 0)`. `EPERM` means the process exists but belongs to
/// another user, which still counts as alive. Only `ESRCH` (no such process)
/// is reported as dead, so this never claims a live daemon is gone.
///
/// Callers must confirm the PID was recorded on *this* host before trusting
/// the result — PIDs are not meaningful across machines.
#[cfg(unix)]
pub fn process_alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        // A PID that does not fit in pid_t cannot name a real process.
        return false;
    };
    if pid <= 0 {
        return false;
    }

    // SAFETY: `kill` with signal 0 performs error checking only.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }

    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Non-Unix fallback: assume the process is alive so we never delete a lock
/// we cannot verify.
#[cfg(not(unix))]
pub fn process_alive(_pid: u32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_id_is_stable_and_not_random() {
        let first = host_id();
        let second = host_id();
        assert_eq!(first, second, "host id must be stable across calls");
        assert!(!first.is_empty());
    }

    #[test]
    fn current_process_is_alive() {
        assert!(process_alive(std::process::id()));
    }

    #[test]
    fn pid_zero_is_not_alive() {
        // PID 0 addresses a process group, never a process we could own.
        assert!(!process_alive(0));
    }

    #[test]
    fn reaped_child_is_not_alive() {
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn /usr/bin/true");
        let pid = child.id();
        child.wait().expect("wait for child");
        assert!(!process_alive(pid), "reaped PID {pid} should be dead");
    }
}
