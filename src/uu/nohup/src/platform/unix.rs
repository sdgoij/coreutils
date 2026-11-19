// This file is part of the uutils coreutils package.
//
// For the full copyright and license information, please view the LICENSE
// file that was distributed with this source code.

// spell-checker:ignore (ToDO) SIGHUP cproc vprocmgr homeout

use std::fs::File;
#[cfg(not(target_os = "minix"))]
use std::fs::OpenOptions;
use std::io::Error;
#[cfg(not(target_os = "minix"))]
use std::io::IsTerminal as _;
#[cfg(not(target_os = "minix"))]
use std::os::unix::fs::OpenOptionsExt as _;
use std::os::unix::process::CommandExt as _;
use std::process::Command;
use thiserror::Error as ThisError;
use uucore::error::{UError, UResult};
use uucore::translate;

use crate::find_stdout;

#[derive(Debug, ThisError)]
enum PlatformError {
    #[cfg(target_vendor = "apple")]
    #[error("{}", translate!("nohup-error-cannot-detach"))]
    CannotDetach,

    #[error("{}", translate!("nohup-error-cannot-replace", "name" => (*_0), "err" => _1))]
    CannotReplace(&'static str, #[source] Error),
}

impl UError for PlatformError {
    fn code(&self) -> i32 {
        2
    }
}

/// Detach from the controlling terminal: redirect the standard streams and
/// ignore `SIGHUP` so the command survives the shell it was started from.
pub(crate) fn prepare() -> UResult<()> {
    replace_fds()?;

    unsafe { libc::signal(libc::SIGHUP, libc::SIG_IGN) };

    #[cfg(target_vendor = "apple")]
    if unsafe { !_vprocmgr_detach_from_console(0).is_null() } {
        return Err(PlatformError::CannotDetach.into());
    }

    Ok(())
}

/// Replace the current process image with `command`.
///
/// Returns the error that made `execvp` fail; on success it never returns.
#[allow(
    clippy::unnecessary_wraps,
    reason = "signature shared with the Windows implementation, which can fail"
)]
pub(crate) fn run(command: &mut Command) -> UResult<Option<Error>> {
    Ok(Some(command.exec()))
}

/// POSIX nohup creates the output file with mode 0600 so that other users on a
/// shared host can't read whatever the detached job logs.
///
/// This only affects newly-created files; if the file already exists its
/// permissions are left alone.
#[cfg(not(target_os = "minix"))]
pub(crate) fn set_output_file_mode(opt: &mut OpenOptions) {
    opt.mode(0o600);
}

fn replace_fds() -> UResult<()> {
    use rustix::stdio::{dup2_stderr, dup2_stdin, dup2_stdout, stdout};
    if is_terminal(0) {
        let new_stdin = File::open(std::path::Path::new("/dev/null"))
            .map_err(|e| PlatformError::CannotReplace("STDIN", e))?;
        dup2_stdin(&new_stdin).map_err(|e| PlatformError::CannotReplace("STDIN", e.into()))?;
    }

    if is_terminal(1) {
        let new_stdout = find_stdout()?;

        dup2_stdout(&new_stdout).map_err(|e| PlatformError::CannotReplace("STDOUT", e.into()))?;
    }

    if is_terminal(2) {
        dup2_stderr(stdout()).map_err(|e| PlatformError::CannotReplace("STDERR", e.into()))?;
    }
    Ok(())
}

/// Whether a standard descriptor is a terminal.
///
/// The std PAL answers `IsTerminal` with `false` for every stream on this port (it predates
/// the libc `isatty`, which the port now has), so asking it would leave all three streams in
/// place and `nohup.out` would never be created.
#[cfg(target_os = "minix")]
fn is_terminal(fd: i32) -> bool {
    // SAFETY: isatty only inspects the descriptor number it is given.
    unsafe { libc::isatty(fd) == 1 }
}

#[cfg(not(target_os = "minix"))]
fn is_terminal(fd: i32) -> bool {
    match fd {
        0 => std::io::stdin().is_terminal(),
        1 => std::io::stdout().is_terminal(),
        _ => std::io::stderr().is_terminal(),
    }
}

#[cfg(target_vendor = "apple")]
unsafe extern "C" {
    fn _vprocmgr_detach_from_console(flags: u32) -> *const core::ffi::c_int;
}
