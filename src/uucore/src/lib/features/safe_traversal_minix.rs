//! The minix arm of `safe_traversal`: the `nix` surface it uses, over the port's
//! C library.
//!
//! `safe_traversal` is written against `nix`, which does not build for minix
//! (its `Signal` enum and errno tables assume a Linux-shaped libc). Every call
//! the module makes exists in the port's own C library (`crates/minix-libc`), so
//! this module offers the same names and shapes and `safe_traversal` imports
//! from here when the target is minix.
//!
//! Two deliberate deviations, both documented at the item they touch:
//!
//! - `ok_or` errors are plain `c_int` errno values, not `nix`'s enum, because
//!   the call sites already convert them with `from_raw_os_error(e as i32)`.
//! - `openat`/`fstatat`/`fchmodat`/`fchownat`/`unlinkat`/`mkdirat` take the
//!   descriptor by `impl AsFd`, so a caller can pass `&OwnedFd` or a
//!   `BorrowedFd` as it does with `nix`.
//!
//! The descriptor-relative calls go through the port's `with_dirfd`, which
//! saves the cwd, `fchdir`s into the directory and restores it; VFS has no
//! dirfd-relative lookup. That is why a `Dir` here holds a real descriptor: the
//! traversal stays pinned to the opened inode even though each call names it by
//! path.

use std::ffi::{CStr, CString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;

pub use libc::stat as FileStat;

use libc::{c_int, mode_t};

/// A raw errno value, which is what the port's C calls report.
pub type Errno = c_int;

fn last_errno() -> Errno {
    io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// The `fchmodat`/`fstatat`/`fchownat` lookup flag.
pub const AT_SYMLINK_NOFOLLOW: c_int = 0x200;

// ---- fcntl ----

/// `open(2)` flags. The port's values are NetBSD's
/// (`tools/c-include/fcntl.h`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct OFlag(c_int);

impl OFlag {
    pub const O_RDONLY: OFlag = OFlag(libc::O_RDONLY);
    pub const O_WRONLY: OFlag = OFlag(libc::O_WRONLY);
    pub const O_CREAT: OFlag = OFlag(libc::O_CREAT);
    pub const O_EXCL: OFlag = OFlag(libc::O_EXCL);
    pub const O_NOFOLLOW: OFlag = OFlag(libc::O_NOFOLLOW);
    pub const O_DIRECTORY: OFlag = OFlag(libc::O_DIRECTORY);
    pub const O_CLOEXEC: OFlag = OFlag(libc::O_CLOEXEC);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn bits(self) -> c_int {
        self.0
    }
}

impl core::ops::BitOr for OFlag {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for OFlag {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// The `*at` lookup flags.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AtFlags(c_int);

impl AtFlags {
    pub const AT_SYMLINK_NOFOLLOW: AtFlags = AtFlags(AT_SYMLINK_NOFOLLOW);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn bits(self) -> c_int {
        self.0
    }
}

/// The `fchmodat` behaviour when the name is a symlink.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FchmodatFlags {
    FollowSymlink,
    NoFollowSymlink,
}

/// The `unlinkat` behaviour: remove a directory instead of a name.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UnlinkatFlags {
    RemoveDir,
    NoRemoveDir,
}

/// A mode, as `open`/`mkdirat`/`fchmod` take it.
#[derive(Clone, Copy)]
pub struct Mode(mode_t);

impl Mode {
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Keep the permission and set-id bits, as `nix`'s `from_bits_truncate`
    /// does.
    pub const fn from_bits_truncate(mode: mode_t) -> Self {
        Self(mode & 0o7777)
    }

    pub const fn bits(self) -> mode_t {
        self.0
    }
}

/// A user id, in the two places `fchownat` names one.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Uid(u32);

impl Uid {
    pub const fn from_raw(uid: u32) -> Self {
        Self(uid)
    }

    pub const fn as_raw(self) -> u32 {
        self.0
    }
}

/// A group id, as above.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Gid(u32);

impl Gid {
    pub const fn from_raw(gid: u32) -> Self {
        Self(gid)
    }

    pub const fn as_raw(self) -> u32 {
        self.0
    }
}

/// `dup(2)`: a second descriptor, owned.
///
/// # Safety of the cast
///
/// `OwnedFd::from_raw_fd` requires a descriptor nothing else owns, which is
/// exactly what a successful `dup` returns.
pub fn dup(fd: &OwnedFd) -> Result<OwnedFd, Errno> {
    let r = unsafe { libc::dup(fd.as_raw_fd()) };
    if r < 0 {
        return Err(last_errno());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(r) })
}

fn owned(fd: c_int) -> Result<OwnedFd, Errno> {
    if fd < 0 {
        return Err(last_errno());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn cvt(r: c_int) -> Result<(), Errno> {
    if r < 0 { Err(last_errno()) } else { Ok(()) }
}

/// `open(2)`.
pub fn open(path: &Path, flags: OFlag, mode: Mode) -> Result<OwnedFd, Errno> {
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| libc::EINVAL)?;
    owned(unsafe { libc::open(path.as_ptr(), flags.bits(), mode.bits()) })
}

/// `openat(2)`.
pub fn openat(
    dirfd: impl AsFd,
    path: &CStr,
    flags: OFlag,
    mode: Mode,
) -> Result<OwnedFd, Errno> {
    owned(unsafe {
        libc::openat(
            dirfd.as_fd().as_raw_fd(),
            path.as_ptr(),
            flags.bits(),
            mode.bits(),
        )
    })
}

/// `fstatat(2)`.
pub fn fstatat(dirfd: impl AsFd, path: &CStr, flags: AtFlags) -> Result<FileStat, Errno> {
    let mut st: FileStat = unsafe { core::mem::zeroed() };
    let r = unsafe {
        libc::fstatat(
            dirfd.as_fd().as_raw_fd(),
            path.as_ptr(),
            &mut st,
            flags.bits(),
        )
    };
    if r < 0 {
        return Err(last_errno());
    }
    Ok(st)
}

/// `fstat(2)`.
pub fn fstat(fd: impl AsFd) -> Result<FileStat, Errno> {
    let mut st: FileStat = unsafe { core::mem::zeroed() };
    let r = unsafe { libc::fstat(fd.as_fd().as_raw_fd(), &mut st) };
    if r < 0 {
        return Err(last_errno());
    }
    Ok(st)
}

/// `fchmod(2)`.
pub fn fchmod(fd: impl AsFd, mode: Mode) -> Result<(), Errno> {
    cvt(unsafe { libc::fchmod(fd.as_fd().as_raw_fd(), mode.bits()) })
}

/// `fchmodat(2)`. The port answers `ENOSYS` when asked not to follow a
/// symlink, which is the signal `safe_traversal` already falls back on.
pub fn fchmodat(
    dirfd: impl AsFd,
    path: &CStr,
    mode: Mode,
    flags: FchmodatFlags,
) -> Result<(), Errno> {
    let flags = match flags {
        FchmodatFlags::FollowSymlink => 0,
        FchmodatFlags::NoFollowSymlink => AT_SYMLINK_NOFOLLOW,
    };
    cvt(unsafe { libc::fchmodat(dirfd.as_fd().as_raw_fd(), path.as_ptr(), mode.bits(), flags) })
}

/// `fchown(2)`; a `None` id is `-1`, "leave this one alone".
pub fn fchown(fd: impl AsFd, uid: Option<Uid>, gid: Option<Gid>) -> Result<(), Errno> {
    let uid = uid.map_or(!0, Uid::as_raw);
    let gid = gid.map_or(!0, Gid::as_raw);
    cvt(unsafe { libc::fchown(fd.as_fd().as_raw_fd(), uid, gid) })
}

/// `fchownat(2)`; a `None` id is `-1`, "leave this one alone".
pub fn fchownat(
    dirfd: impl AsFd,
    path: &CStr,
    uid: Option<Uid>,
    gid: Option<Gid>,
    flags: AtFlags,
) -> Result<(), Errno> {
    let uid = uid.map_or(!0, Uid::as_raw);
    let gid = gid.map_or(!0, Gid::as_raw);
    cvt(unsafe {
        libc::fchownat(
            dirfd.as_fd().as_raw_fd(),
            path.as_ptr(),
            uid,
            gid,
            flags.bits(),
        )
    })
}

/// `mkdirat(2)`.
pub fn mkdirat(dirfd: impl AsFd, path: &CStr, mode: Mode) -> Result<(), Errno> {
    cvt(unsafe { libc::mkdirat(dirfd.as_fd().as_raw_fd(), path.as_ptr(), mode.bits()) })
}

/// `unlinkat(2)`.
pub fn unlinkat(dirfd: impl AsFd, path: &CStr, flags: UnlinkatFlags) -> Result<(), Errno> {
    let flags = match flags {
        UnlinkatFlags::RemoveDir => libc::AT_REMOVEDIR,
        UnlinkatFlags::NoRemoveDir => 0,
    };
    cvt(unsafe { libc::unlinkat(dirfd.as_fd().as_raw_fd(), path.as_ptr(), flags) })
}

// ---- dirent ----

/// A directory stream over an owned descriptor.
///
/// `fdopendir` takes ownership of the descriptor, so `Drop` closes it through
/// `closedir` alone.
pub struct Dir {
    dirp: *mut libc::DIR,
}

impl Dir {
    /// A stream over `fd`, which this takes ownership of.
    pub fn from_fd(fd: OwnedFd) -> Result<Self, Errno> {
        let dirp = unsafe { libc::fdopendir(fd.as_raw_fd()) };
        if dirp.is_null() {
            return Err(last_errno());
        }
        // `closedir` will close it.
        core::mem::forget(fd);
        Ok(Self { dirp })
    }

    /// The entries, each with its name copied out of the stream's buffer.
    pub fn iter(&mut self) -> DirIter<'_> {
        DirIter { dir: self }
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        unsafe { libc::closedir(self.dirp) };
    }
}

/// One entry's name, copied because `readdir` reuses its buffer.
pub struct DirEntry {
    name: CString,
}

impl DirEntry {
    pub fn file_name(&self) -> &CStr {
        self.name.as_c_str()
    }
}

/// The iterator `Dir::iter` returns.
pub struct DirIter<'a> {
    dir: &'a mut Dir,
}

impl Iterator for DirIter<'_> {
    type Item = Result<DirEntry, Errno>;

    fn next(&mut self) -> Option<Self::Item> {
        let entry = unsafe { libc::readdir(self.dir.dirp) };
        if entry.is_null() {
            return None;
        }
        // SAFETY: `readdir` returned a valid entry whose `d_name` is a NUL
        // terminated array inside the stream's buffer.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        match CString::new(name.to_bytes()) {
            Ok(name) => Some(Ok(DirEntry { name })),
            Err(_) => Some(Err(libc::EINVAL)),
        }
    }
}
