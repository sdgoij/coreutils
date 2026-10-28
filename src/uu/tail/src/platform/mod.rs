// This file is part of the uutils coreutils package.
//
// For the full copyright and license information, please view the LICENSE
// file that was distributed with this source code.

#[cfg(unix)]
pub use self::unix::{supports_pid_checks, Pid};

// `tail -f` cannot work without inotify, so nothing constructs the checker on
// minix.
#[cfg(all(unix, not(target_os = "minix")))]
pub use self::unix::ProcessChecker;

#[cfg(windows)]
pub use self::windows::{supports_pid_checks, Pid, ProcessChecker};

// WASI has no process-management APIs exposed yet; provide stubs so tail
// compiles.
#[cfg(target_os = "wasi")]
pub type Pid = u64;

#[cfg(target_os = "wasi")]
pub fn supports_pid_checks(_pid: Pid) -> bool {
    false
}

#[cfg(unix)]
mod unix;

#[cfg(windows)]
mod windows;
