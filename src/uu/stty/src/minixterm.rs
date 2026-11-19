// This file is part of the uutils coreutils package.
//
// For the full copyright and license information, please view the LICENSE
// file that was distributed with this source code.

//! A `nix`-shaped termios surface for MINIX, over `rustix::termios`.
//!
//! `nix` has no minix support and no fork, and `stty` is written against its
//! `Termios`: flag groups with `contains`/`intersects`/`set`, a `control_chars`
//! array indexed by `SpecialCharacterIndices`, and `tcgetattr`/`tcsetattr` over
//! a borrowed fd. The port's `rustix` fork now serves that surface (`termios`
//! backend over the tty server's `TIOCGETA`/`TIOCSETA` family), so this module
//! re-exposes it in the shape `stty` already uses instead of rewriting `stty`.
//!
//! `stty.rs` and `flags.rs` reach it through `use crate::minixterm as nix;`, so
//! every other line of both files is the upstream one. Speeds are plain numbers
//! (the tty server keeps `c_ispeed`/`c_ospeed` as rates, not as flag bits),
//! which is why the build treats minix as a `bsd` target: that is the path
//! `stty` takes for a `u32` speed.

pub mod libc {
    //! The names `stty` takes from `nix::libc`. These are scalar constants and
    //! types only; every termios call goes through `rustix`.
    pub use ::libc::{NCCS, O_NONBLOCK, TIOCGWINSZ, TIOCSWINSZ, c_int, c_ushort, cc_t};
}

/// `nix::errno`: the one errno `stty` raises itself.
pub mod errno {
    /// One errno value, convertible into the `io::Error` that `Result` carries.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Errno(pub i32);

    impl Errno {
        /// Result too large: what `stty` answers for a control character above
        /// the eight bits `c_cc` holds.
        pub const ERANGE: Errno = Errno(34);
    }

    impl From<Errno> for std::io::Error {
        fn from(e: Errno) -> Self {
            std::io::Error::from_raw_os_error(e.0)
        }
    }
}

/// `nix::Result`: what `stty`'s helpers return.
pub type Result<T> = std::io::Result<T>;

/// `rustix` reports an `Errno` rather than an `io::Error`; the number is the
/// same one `errno` would hold.
fn io_err(err: rustix::io::Errno) -> std::io::Error {
    std::io::Error::from_raw_os_error(err.raw_os_error())
}

/// Define a flag group: `tcflag_t` bits with the named constants, in the shape
/// `stty` uses (nix's `bitflags` surface, minus what it does not touch).
///
/// The bits come from the matching `rustix::termios` type, so a flag the port
/// does not have cannot silently get a made-up value.
macro_rules! flag_group {
    ($name:ident, $modes:path, $($flag:ident),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
        pub struct $name(u32);

        impl $name {
            $(pub const $flag: Self = Self(<$modes>::$flag.bits());)*

            pub const fn bits(self) -> u32 {
                self.0
            }

            pub const fn from_bits_truncate(bits: u32) -> Self {
                Self(bits)
            }

            pub const fn contains(self, other: Self) -> bool {
                self.0 & other.0 == other.0
            }

            pub const fn intersects(self, other: Self) -> bool {
                self.0 & other.0 != 0
            }

            pub fn set(&mut self, other: Self, value: bool) {
                if value {
                    self.0 |= other.0;
                } else {
                    self.0 &= !other.0;
                }
            }
        }

        impl core::ops::Sub for $name {
            type Output = Self;
            fn sub(self, other: Self) -> Self {
                Self(self.0 & !other.0)
            }
        }
    };
}

/// Get the window size into a caller-owned `struct winsize`-shaped value.
///
/// # Safety
///
/// `fd` must be open and `data` must be writable for a `Winsize`.
pub fn winsize_get(
    fd: libc::c_int,
    data: *mut rustix::termios::Winsize,
) -> Result<libc::c_int> {
    // SAFETY: the caller guarantees `fd` is open.
    let ws = rustix::termios::tcgetwinsize(unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) })
        .map_err(io_err)?;
    // SAFETY: the caller guarantees `data` is writable.
    unsafe { data.write(ws) };
    Ok(0)
}

/// Set the window size from a caller-owned `struct winsize`-shaped value.
///
/// # Safety
///
/// `fd` must be open and `data` must be readable for a `Winsize`.
pub fn winsize_set(
    fd: libc::c_int,
    data: *const rustix::termios::Winsize,
) -> Result<libc::c_int> {
    // SAFETY: the caller guarantees `data` is readable.
    let ws = unsafe { data.read() };
    // SAFETY: the caller guarantees `fd` is open.
    rustix::termios::tcsetwinsize(unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) }, ws)
        .map_err(io_err)?;
    Ok(0)
}

pub mod sys {
    pub mod termios {
        //! `termios.h`: the flag groups, the control-character indices and the
        //! `tc*` calls, over `rustix::termios`.

        use super::super::Result;
        use rustix::termios::{
            ControlModes, InputModes, LocalModes, OptionalActions, OutputModes, SpecialCodes,
            Termios as RustixTermios,
        };
        use std::os::fd::AsFd;

        /// The length of `c_cc`, from `termios.h`.
        pub const NCCS: usize = ::libc::NCCS;

        flag_group!(
            InputFlags,
            InputModes,
            IGNBRK,
            BRKINT,
            IGNPAR,
            PARMRK,
            INPCK,
            ISTRIP,
            INLCR,
            IGNCR,
            ICRNL,
            IXON,
            IXOFF,
            IXANY,
            IMAXBEL,
        );

        flag_group!(OutputFlags, OutputModes, OPOST, ONLCR, OCRNL, ONOCR, ONLRET);

        flag_group!(
            ControlFlags,
            ControlModes,
            CSIZE,
            CS5,
            CS6,
            CS7,
            CS8,
            CSTOPB,
            CREAD,
            PARENB,
            PARODD,
            HUPCL,
            CLOCAL,
            CRTSCTS,
        );

        flag_group!(
            LocalFlags,
            LocalModes,
            ECHOKE,
            ECHOE,
            ECHOK,
            ECHO,
            ECHONL,
            ECHOPRT,
            ECHOCTL,
            ISIG,
            ICANON,
            IEXTEN,
            EXTPROC,
            TOSTOP,
            FLUSHO,
            NOFLSH,
        );

        /// `c_cc` subscripts, the ones `termios.h` names.
        #[allow(non_camel_case_types)]
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(usize)]
        pub enum SpecialCharacterIndices {
            VEOF = ::libc::VEOF,
            VEOL = ::libc::VEOL,
            VEOL2 = ::libc::VEOL2,
            VERASE = ::libc::VERASE,
            VWERASE = ::libc::VWERASE,
            VKILL = ::libc::VKILL,
            VREPRINT = ::libc::VREPRINT,
            VINTR = ::libc::VINTR,
            VQUIT = ::libc::VQUIT,
            VSUSP = ::libc::VSUSP,
            VSTART = ::libc::VSTART,
            VSTOP = ::libc::VSTOP,
            VLNEXT = ::libc::VLNEXT,
            VDISCARD = ::libc::VDISCARD,
            VMIN = ::libc::VMIN,
            VTIME = ::libc::VTIME,
        }

        /// `tcsetattr`'s third argument.
        #[allow(non_camel_case_types)]
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum SetArg {
            /// The change takes effect immediately.
            TCSANOW,
            /// After queued output has been transmitted.
            TCSADRAIN,
            /// After queued output has been transmitted and input discarded.
            ///
            /// `stty` only ever asks for the other two, but the surface matches
            /// `tcsetattr`'s third argument, so the variant stays.
            #[allow(dead_code)]
            TCSAFLUSH,
        }

        /// Terminal attributes, with the flag groups already split out.
        ///
        /// The field names are `nix`'s (and so `stty`'s), not `termios.h`'s:
        /// each is a `c_*flag` the `tc*` calls pack back into the C struct. The
        /// speeds are not flag groups - `stty` sets and reads them through
        /// `cfsetospeed`/`cfgetospeed` - so they are carried here to survive the
        /// round trip through `tcsetattr`.
        ///
        /// `raw` is the struct `tcgetattr` read; it is the mould `tcsetattr`
        /// hands back, because `rustix::termios::Termios` has no public
        /// constructor and no way to build an invalid one.
        #[derive(Clone, Debug)]
        pub struct Termios {
            pub input_flags: InputFlags,
            pub output_flags: OutputFlags,
            pub control_flags: ControlFlags,
            pub local_flags: LocalFlags,
            pub control_chars: [u8; NCCS],
            ispeed: u32,
            ospeed: u32,
            raw: RustixTermios,
        }

        impl Termios {
            fn from_raw(raw: &RustixTermios) -> Self {
                Self {
                    input_flags: InputFlags::from_bits_truncate(raw.input_modes.bits()),
                    output_flags: OutputFlags::from_bits_truncate(raw.output_modes.bits()),
                    control_flags: ControlFlags::from_bits_truncate(raw.control_modes.bits()),
                    local_flags: LocalFlags::from_bits_truncate(raw.local_modes.bits()),
                    control_chars: raw
                        .special_codes
                        .as_bytes()
                        .try_into()
                        .expect("c_cc length matches NCCS"),
                    ispeed: raw.input_speed(),
                    ospeed: raw.output_speed(),
                    raw: raw.clone(),
                }
            }

            fn to_raw(&self) -> Result<RustixTermios> {
                let mut raw = self.raw.clone();
                raw.input_modes = InputModes::from_bits_truncate(self.input_flags.bits());
                raw.output_modes = OutputModes::from_bits_truncate(self.output_flags.bits());
                raw.control_modes = ControlModes::from_bits_truncate(self.control_flags.bits());
                raw.local_modes = LocalModes::from_bits_truncate(self.local_flags.bits());
                raw.special_codes = SpecialCodes::from_bytes(&self.control_chars);
                raw.set_input_speed(self.ispeed).map_err(super::super::io_err)?;
                raw.set_output_speed(self.ospeed)
                    .map_err(super::super::io_err)?;
                Ok(raw)
            }
        }

        /// `tcgetattr(fd)`: the attributes `fd`'s terminal has now.
        pub fn tcgetattr<Fd: AsFd>(fd: Fd) -> Result<Termios> {
            let raw = rustix::termios::tcgetattr(fd).map_err(super::super::io_err)?;
            Ok(Termios::from_raw(&raw))
        }

        /// `tcsetattr(fd, actions, termios)`.
        pub fn tcsetattr<Fd: AsFd>(fd: Fd, actions: SetArg, termios: &Termios) -> Result<()> {
            let actions = match actions {
                SetArg::TCSANOW => OptionalActions::Now,
                SetArg::TCSADRAIN => OptionalActions::Drain,
                SetArg::TCSAFLUSH => OptionalActions::Flush,
            };
            rustix::termios::tcsetattr(fd, actions, &termios.to_raw()?)
                .map_err(super::super::io_err)
        }

        /// `cfgetospeed`: the output speed, in baud, not a flag bit pattern.
        pub fn cfgetospeed(termios: &Termios) -> u32 {
            termios.ospeed
        }

        /// `cfsetospeed`.
        pub fn cfsetospeed(termios: &mut Termios, speed: u32) -> Result<()> {
            termios.ospeed = speed;
            Ok(())
        }

        /// `cfsetispeed`.
        pub fn cfsetispeed(termios: &mut Termios, speed: u32) -> Result<()> {
            termios.ispeed = speed;
            Ok(())
        }
    }
}

/// `ioctl_read_bad!(name, request, Ty)`: an ioctl that reads into `*mut Ty`.
///
/// The shim's only use is `TIOCGWINSZ`, so the target is a `rustix` `Winsize`;
/// `stty`'s `TermSize` is `#[repr(C)]` with the same four `u16`s.
macro_rules! ioctl_read_bad {
    ($(#[$attr:meta])* $name:ident, $nr:expr, $ty:ty) => {
        $(#[$attr])*
        pub unsafe fn $name(
            fd: $crate::minixterm::libc::c_int,
            data: *mut $ty,
        ) -> $crate::minixterm::Result<$crate::minixterm::libc::c_int> {
            debug_assert_eq!($nr, $crate::minixterm::libc::TIOCGWINSZ);
            $crate::minixterm::winsize_get(fd, data.cast())
        }
    };
}

/// `ioctl_write_ptr_bad!(name, request, Ty)`: an ioctl that reads through
/// `*const Ty`.
macro_rules! ioctl_write_ptr_bad {
    ($(#[$attr:meta])* $name:ident, $nr:expr, $ty:ty) => {
        $(#[$attr])*
        pub unsafe fn $name(
            fd: $crate::minixterm::libc::c_int,
            data: *const $ty,
        ) -> $crate::minixterm::Result<$crate::minixterm::libc::c_int> {
            debug_assert_eq!($nr, $crate::minixterm::libc::TIOCSWINSZ);
            $crate::minixterm::winsize_set(fd, data.cast())
        }
    };
}

pub(crate) use {ioctl_read_bad, ioctl_write_ptr_bad};
