// This file is part of the uutils coreutils package.
//
// For the full copyright and license information, please view the LICENSE
// file that was distributed with this source code.

//! A `nix`-shaped termios surface for MINIX.
//!
//! `nix` has no minix support and no fork, and `stty` is written against its
//! `Termios`: flag groups with `contains`/`intersects`/`set`, a `control_chars`
//! array indexed by `SpecialCharacterIndices`, and `tcgetattr`/`tcsetattr` over
//! a borrowed fd. The port has all of that underneath — the tty server keeps a
//! NetBSD-shaped `struct termios`, `minix_std::termios` owns the request codes,
//! and `libc` (the fork) declares the flags and the `tc*` calls — so this
//! module re-exposes it in the shape `stty` already uses instead of rewriting
//! `stty`.
//!
//! `stty.rs` and `flags.rs` reach it through `use crate::minixterm as nix;`, so
//! every other line of both files is the upstream one. Speeds are plain numbers
//! here (the tty server keeps `c_ispeed`/`c_ospeed` as rates, not as flag
//! bits), which is why the build treats minix as a `bsd` target: that is the
//! path `stty` takes for a `u32` speed.

pub mod libc {
    //! The names `stty` takes from `nix::libc`.
    pub use ::libc::{NCCS, O_NONBLOCK, TIOCGWINSZ, TIOCSWINSZ, c_int, c_ulong, c_ushort, cc_t};
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

/// Define a flag group: `tcflag_t` bits with the named constants, in the shape
/// `stty` uses (nix's `bitflags` surface, minus what it does not touch).
macro_rules! flag_group {
    ($name:ident, $($flag:ident),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
        pub struct $name(u32);

        impl $name {
            $(pub const $flag: Self = Self(::libc::$flag);)*

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

pub mod sys {
    pub mod termios {
        //! `termios.h`: the flag groups, the control-character indices and the
        //! `tc*` calls, over the port's C library.

        use super::super::Result;
        use std::os::fd::{AsFd, AsRawFd};

        /// The length of `c_cc`, from `termios.h`.
        pub const NCCS: usize = ::libc::NCCS;

        flag_group!(
            InputFlags, IGNBRK, BRKINT, IGNPAR, PARMRK, INPCK, ISTRIP, INLCR, IGNCR, ICRNL, IXON,
            IXOFF, IXANY, IMAXBEL,
        );

        flag_group!(OutputFlags, OPOST, ONLCR, OCRNL, ONOCR, ONLRET);

        flag_group!(
            ControlFlags,
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
            LocalFlags, ECHOKE, ECHOE, ECHOK, ECHO, ECHONL, ECHOPRT, ECHOCTL, ISIG, ICANON, IEXTEN,
            EXTPROC, TOSTOP, FLUSHO, NOFLSH,
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
        /// speeds are not flag groups — `stty` sets and reads them through
        /// `cfsetospeed`/`cfgetospeed` — so they are carried here to survive the
        /// round trip through `tcsetattr`.
        #[derive(Clone, Debug)]
        pub struct Termios {
            pub input_flags: InputFlags,
            pub output_flags: OutputFlags,
            pub control_flags: ControlFlags,
            pub local_flags: LocalFlags,
            pub control_chars: [u8; NCCS],
            ispeed: u32,
            ospeed: u32,
        }

        impl Termios {
            fn from_raw(raw: &::libc::termios) -> Self {
                Self {
                    input_flags: InputFlags::from_bits_truncate(raw.c_iflag),
                    output_flags: OutputFlags::from_bits_truncate(raw.c_oflag),
                    control_flags: ControlFlags::from_bits_truncate(raw.c_cflag),
                    local_flags: LocalFlags::from_bits_truncate(raw.c_lflag),
                    control_chars: raw.c_cc,
                    ispeed: raw.c_ispeed,
                    ospeed: raw.c_ospeed,
                }
            }

            fn to_raw(&self) -> ::libc::termios {
                ::libc::termios {
                    c_iflag: self.input_flags.bits(),
                    c_oflag: self.output_flags.bits(),
                    c_cflag: self.control_flags.bits(),
                    c_lflag: self.local_flags.bits(),
                    c_cc: self.control_chars,
                    c_ispeed: self.ispeed,
                    c_ospeed: self.ospeed,
                }
            }
        }

        /// `tcgetattr(fd)`: the attributes `fd`'s terminal has now.
        pub fn tcgetattr<Fd: AsFd>(fd: Fd) -> Result<Termios> {
            let mut raw = ::libc::termios {
                c_iflag: 0,
                c_oflag: 0,
                c_cflag: 0,
                c_lflag: 0,
                c_cc: [0; NCCS],
                c_ispeed: 0,
                c_ospeed: 0,
            };
            let r = unsafe { ::libc::tcgetattr(fd.as_fd().as_raw_fd(), &raw mut raw) };
            if r < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Termios::from_raw(&raw))
        }

        /// `tcsetattr(fd, actions, termios)`.
        pub fn tcsetattr<Fd: AsFd>(fd: Fd, actions: SetArg, termios: &Termios) -> Result<()> {
            let actions = match actions {
                SetArg::TCSANOW => ::libc::TCSANOW,
                SetArg::TCSADRAIN => ::libc::TCSADRAIN,
                SetArg::TCSAFLUSH => ::libc::TCSAFLUSH,
            };
            let raw = termios.to_raw();
            let r = unsafe { ::libc::tcsetattr(fd.as_fd().as_raw_fd(), actions, &raw) };
            if r < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }

        /// `cfgetospeed`: the output speed, in baud, not a flag bit pattern.
        pub fn cfgetospeed(termios: &Termios) -> u32 {
            let raw = termios.to_raw();
            unsafe { ::libc::cfgetospeed(&raw) as u32 }
        }

        /// `cfsetospeed`.
        pub fn cfsetospeed(termios: &mut Termios, speed: u32) -> Result<()> {
            set_speed(termios, speed, true)
        }

        /// `cfsetispeed`.
        pub fn cfsetispeed(termios: &mut Termios, speed: u32) -> Result<()> {
            set_speed(termios, speed, false)
        }

        fn set_speed(termios: &mut Termios, speed: u32, output: bool) -> Result<()> {
            let mut raw = termios.to_raw();
            let r = unsafe {
                if output {
                    ::libc::cfsetospeed(&raw mut raw, speed as ::libc::c_int)
                } else {
                    ::libc::cfsetispeed(&raw mut raw, speed as ::libc::c_int)
                }
            };
            if r < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let raw = Termios::from_raw(&raw);
            if output {
                termios.ospeed = raw.ospeed;
            } else {
                termios.ispeed = raw.ispeed;
            }
            Ok(())
        }
    }
}

/// One ioctl, in the shape of the fork's `libc::ioctl` (which is variadic).
pub(crate) fn ioctl(fd: libc::c_int, request: libc::c_ulong, arg: *mut u8) -> Result<libc::c_int> {
    let r = unsafe { ::libc::ioctl(fd, request, arg) };
    if r < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(r)
}

/// `ioctl_read_bad!(name, request, Ty)`: an ioctl that reads into `*mut Ty`.
macro_rules! ioctl_read_bad {
    ($(#[$attr:meta])* $name:ident, $nr:expr, $ty:ty) => {
        $(#[$attr])*
        pub unsafe fn $name(
            fd: $crate::minixterm::libc::c_int,
            data: *mut $ty,
        ) -> $crate::minixterm::Result<$crate::minixterm::libc::c_int> {
            $crate::minixterm::ioctl(fd, $nr, data as *mut u8)
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
            $crate::minixterm::ioctl(fd, $nr, data as *mut u8)
        }
    };
}

pub(crate) use {ioctl_read_bad, ioctl_write_ptr_bad};
