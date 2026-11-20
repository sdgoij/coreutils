// This file is part of the uutils coreutils package.
//
// For the full copyright and license information, please view the LICENSE
// file that was distributed with this source code.

// spell-checker:ignore (ToDO) sigstr setpgid sigchld sigwait getpid TTIN TTOU SIGALRM

//! Minix implementation of `timeout`'s platform facade.
//!
//! The shared unix module is built on `nix` (which has no minix arm) and POSIX
//! timers; this one sits on the port's own C library, which has `alarm`,
//! `sigprocmask`, `sigsuspend`, `waitpid` and `signal`. The wait loop is the
//! same shape: block the signals the child must not inherit for the wait, arm
//! an alarm for the remains of the timeout, and `sigsuspend` until one of them
//! arrives. `timeout` here is whole-second, because `alarm(2)` is.
//!
//! Two port limits shape the code:
//!
//! - There are no process groups (`setpgid` is a PM call the port does not
//!   have), so the child stays in the caller's group and `send_signal_group`
//!   is a no-op: signalling the group would signal `timeout` and the shell.
//! - The std PAL does not run `pre_exec` closures, so the signal mask is
//!   blocked *after* the spawn, not in the child's pre-exec hook. The child
//!   therefore starts with the mask the shell gave `timeout`, which is what the
//!   command should have.

use std::io;
use std::process::Child;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, getpid, kill_process, test_kill_process};

use uucore::process::TimeoutRet;

/// The signals held while waiting, matching `uucore`'s unix timeout set.
const WAIT_SIGNALS: [i32; 9] = [
    libc::SIGALRM,
    libc::SIGINT,
    libc::SIGQUIT,
    libc::SIGHUP,
    libc::SIGTERM,
    libc::SIGPIPE,
    libc::SIGUSR1,
    libc::SIGUSR2,
    libc::SIGCHLD,
];

/// The signal the recorded handler last saw, 0 before any.
static LAST_SIGNAL: AtomicI32 = AtomicI32::new(0);

extern "C" fn record_signal(sig: core::ffi::c_int) {
    LAST_SIGNAL.store(sig, Ordering::SeqCst);
}

/// Per-spawn platform state. Minix keeps none: there is no job object and no
/// process group to track.
pub(crate) struct SpawnState;

fn empty_set() -> libc::sigset_t {
    let mut set: libc::sigset_t = [0; 2];
    // SAFETY: `set` is a 16-byte mask buffer, which every one of these calls
    // expects.
    unsafe {
        libc::sigemptyset(&mut set);
    }
    set
}

fn wait_set() -> libc::sigset_t {
    let mut set = empty_set();
    // SAFETY: `set` is initialised and the signal numbers are this port's own.
    unsafe {
        for sig in WAIT_SIGNALS {
            libc::sigaddset(&mut set, sig);
        }
    }
    set
}

/// Record every wait signal so `sigsuspend` says which one woke it.
fn install_handlers() {
    for sig in WAIT_SIGNALS {
        // SIGPIPE is left alone: it is not a wakeup this loop waits for, and
        // taking its default disposition is what a command expects.
        if sig == libc::SIGPIPE {
            continue;
        }
        // SAFETY: `record_signal` has the `signal(2)` handler signature, and
        // ignoring the previous disposition is what the wakeup loop wants.
        unsafe {
            libc::signal(sig, record_signal as *const () as libc::sighandler_t);
        }
    }
}

fn block_wait_signals() -> Option<libc::sigset_t> {
    let set = wait_set();
    let mut old = empty_set();
    // SAFETY: both masks are 16-byte buffers.
    let r = unsafe { libc::sigprocmask(libc::SIG_BLOCK, &set, &mut old) };
    (r == 0).then_some(old)
}

fn restore_mask(old: &libc::sigset_t) {
    // SAFETY: `old` came from a successful `sigprocmask` above.
    unsafe {
        libc::sigprocmask(libc::SIG_SETMASK, old, core::ptr::null_mut());
    }
}

/// Configure the child's spawn attributes, right before it is spawned.
///
/// Nothing to do on this port: `pre_exec` would be the place for the child's
/// mask and terminal-signal reset, but the std PAL does not run those closures,
/// so the wait blocks its set after the spawn instead (see the module comment).
pub(crate) fn prepare(
    _cmd_builder: &mut std::process::Command,
    _foreground: bool,
    _signal: usize,
) -> io::Result<()> {
    Ok(())
}

/// Nothing to do after spawning on this port.
pub(crate) fn post_spawn(_child: &Child, _foreground: bool) -> SpawnState {
    SpawnState
}

/// Wait for the child or for the timeout, whichever comes first.
pub(crate) fn wait_or_timeout(
    child: &mut Child,
    timeout: Duration,
    ignore_term: bool,
) -> io::Result<TimeoutRet> {
    if timeout.is_zero() {
        return child.wait().map(TimeoutRet::Exited);
    }
    // `.try_wait()` elsewhere does the same: holding the pipe open would keep a
    // command that reads stdin from ever seeing EOF.
    drop(child.stdin.take());

    install_handlers();
    let Some(old_mask) = block_wait_signals() else {
        return Err(io::Error::last_os_error());
    };

    let start = Instant::now();
    let mut remaining = timeout;
    let result = loop {
        if remaining.is_zero() {
            break Ok(TimeoutRet::TimedOut);
        }
        LAST_SIGNAL.store(0, Ordering::SeqCst);
        // `alarm(2)` counts whole seconds; round up so a sub-second remainder
        // still wakes the loop.
        let secs = (remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0))
            .clamp(1, u64::from(u32::MAX)) as libc::c_uint;
        // SAFETY: `alarm` takes a plain second count.
        unsafe { libc::alarm(secs) };
        let empty = empty_set();
        // SAFETY: `empty` is an initialised mask; the call returns once a
        // handler has run, which is the wakeup this loop wants.
        unsafe { libc::sigsuspend(&empty) };
        unsafe { libc::alarm(0) };

        let sig = LAST_SIGNAL.load(Ordering::SeqCst);

        // Reap through std: it owns the `ExitStatus` this needs to return, and
        // its non-blocking wait is the same `waitpid(WNOHANG)` the unix path
        // spells out.
        match child.try_wait() {
            Ok(Some(status)) => break Ok(TimeoutRet::Exited(status)),
            Ok(None) => {}
            Err(e) => break Err(e),
        }

        match sig {
            // No handler ran (spurious return), or the alarm's own tick while
            // the child is still running: the loop re-arms with the remainder.
            s if s == 0 || s == libc::SIGALRM || s == libc::SIGCHLD => {}
            // The caller is in `timeout --kill-after`, which keeps waiting
            // through its own termination request.
            s if s == libc::SIGTERM && ignore_term => {}
            s => break Ok(TimeoutRet::Interrupted(s as usize)),
        }
        remaining = timeout.saturating_sub(start.elapsed());
    };

    restore_mask(&old_mask);
    result
}

/// Send a signal to the child itself.
///
/// There is no process group to fan out to on this port, and signalling the
/// caller's group would reach `timeout` and the shell as well.
pub(crate) fn send_signal(
    process: &mut Child,
    signal: usize,
    _foreground: bool,
    _external: Option<usize>,
    _state: &SpawnState,
) {
    let Some(pid) = Pid::from_raw(process.id() as i32) else {
        return;
    };
    if signal == 0 {
        let _ = test_kill_process(pid);
    } else if let Some(sig) = Signal::from_named_raw(signal as i32) {
        // GNU timeout ignores a failed signal: the child may have exited
        // between the timeout and this call.
        let _ = kill_process(pid, sig);
    }
}

/// The signal the child was terminated by, if it was terminated by a signal.
///
/// Minix's `std::process::ExitStatus` is the raw wait word and the platform
/// exposes no `ExitStatusExt`, so the signal is not recoverable here. A
/// `--preserve-status` run of a signalled child therefore reports the generic
/// timeout status (124) instead of 128 plus the signal number; the default path
/// reports 124 either way. Recovering it needs `ExitStatusExt` in the std PAL.
pub(crate) fn status_signal(_status: std::process::ExitStatus) -> Option<i32> {
    None
}

/// Exit the way a signalled child did: unblock its signal and take it.
pub(crate) fn preserve_signal_info(signal: core::ffi::c_int) -> core::ffi::c_int {
    if let Some(sig) = Signal::from_named_raw(signal) {
        let mut set = empty_set();
        // SAFETY: `set` is initialised and the signal number is this port's.
        unsafe {
            libc::sigaddset(&mut set, signal);
            libc::sigprocmask(libc::SIG_UNBLOCK, &set, core::ptr::null_mut());
        }
        let _ = kill_process(getpid(), sig);
    }
    signal
}
