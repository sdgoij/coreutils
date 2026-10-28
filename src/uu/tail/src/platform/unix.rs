// This file is part of the uutils coreutils package.
//
// For the full copyright and license information, please view the LICENSE
// file that was distributed with this source code.

// spell-checker:ignore (ToDO) stdlib, ISCHR, GETFD
// spell-checker:ignore (options) EPERM, ENOSYS, NOSYS

use rustix::process::{test_kill_process, Pid as RustixPid};

pub type Pid = i32;

// Only the `tail -f` path uses this, and that path needs inotify.
#[cfg(not(target_os = "minix"))]
pub struct ProcessChecker {
    pid: Pid,
}

#[cfg(not(target_os = "minix"))]
impl ProcessChecker {
    pub fn new(process_id: Pid) -> Self {
        Self { pid: process_id }
    }

    pub fn is_dead(&self) -> bool {
        RustixPid::from_raw(self.pid)
            .is_none_or(|pid| test_kill_process(pid).is_err_and(|e| e != rustix::io::Errno::PERM))
    }
}

#[cfg(not(target_os = "minix"))]
impl Drop for ProcessChecker {
    fn drop(&mut self) {}
}

pub fn supports_pid_checks(pid: Pid) -> bool {
    RustixPid::from_raw(pid).is_some_and(|p| test_kill_process(p) != Err(rustix::io::Errno::NOSYS))
}
