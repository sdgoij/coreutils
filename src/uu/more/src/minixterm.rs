// This file is part of the uutils coreutils package.
//
// For the full copyright and license information, please view the LICENSE
// file that would have been received by the crate.

//! The crossterm surface `more` uses, over this port's console.
//!
//! `crossterm`'s `events` feature selects `mio`, and `mio` has no minix arm
//! (`KNOWN_ISSUES.md` item 75). What the applet needs from the terminal is
//! small: the ANSI sequences for the alternate screen, the cursor and erasing,
//! the console's size, a switch to raw mode, and a decoder that turns the bytes
//! on standard input into `Event::Key`. This module is that surface in
//! crossterm's shape, and `more.rs` reaches it through
//! `use crate::minixterm as crossterm;`, so every other line of the applet is
//! the upstream one.
//!
//! Two choices are the port's own rather than crossterm's:
//!
//!   * Raw mode is a no-op when standard input is not a terminal. crossterm
//!     answers `ENOTTY` there, which would stop the pager running from a
//!     script (`cat keys | more file`). The line discipline raw mode switches
//!     off exists only on a terminal, so there is nothing to switch off, and a
//!     control character typed at the console keeps reaching the foreground
//!     job, which is what a reader wants when the keys come from elsewhere.
//!   * Keys are taken from standard input through std's own handle, and every
//!     byte is moved into a queue this module owns (see `event::Keys`). That is
//!     the only handle that reaches the console: descriptor 0 is served by the
//!     kernel's console path until the shell redirects it, and `rustix` and std
//!     both talk to VFS directly for file descriptors.

use std::fmt;
use std::io::{self, Write};

/// A value that knows how to render itself as the bytes a terminal acts on.
///
/// crossterm's `Command`; only the ANSI path is needed, since this is the only
/// platform whose terminal is reached through one.
pub trait Command {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result;
}

/// Queue a command on a writer without flushing it.
pub trait QueueableCommand {
    fn queue(&mut self, command: impl Command) -> io::Result<&mut Self>;
}

/// Queue a command and flush, so it reaches the terminal before the next call.
pub trait ExecutableCommand {
    fn execute(&mut self, command: impl Command) -> io::Result<&mut Self>;
}

impl<W: Write + ?Sized> QueueableCommand for W {
    fn queue(&mut self, command: impl Command) -> io::Result<&mut Self> {
        command
            .write_ansi(&mut AnsiWriter(self))
            .map_err(io::Error::other)?;
        Ok(self)
    }
}

impl<W: Write + ?Sized> ExecutableCommand for W {
    fn execute(&mut self, command: impl Command) -> io::Result<&mut Self> {
        self.queue(command)?;
        self.flush()?;
        Ok(self)
    }
}

/// An `io::Write` as an `fmt::Write`, so a command can render straight into
/// the writer without an intermediate `String`.
struct AnsiWriter<'a, W: Write + ?Sized>(&'a mut W);

impl<W: Write + ?Sized> fmt::Write for AnsiWriter<'_, W> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.write_all(s.as_bytes()).map_err(|_| fmt::Error)
    }
}

/// The errno `rustix` reports as an `io::Error`, which is the number C's
/// `errno` would hold.
fn errno(err: rustix::io::Errno) -> io::Error {
    io::Error::from_raw_os_error(err.raw_os_error())
}

pub mod cursor {
    use std::fmt;

    use super::Command;

    /// `ESC[?25l`: stop drawing the cursor.
    pub struct Hide;

    /// `ESC[?25h`: draw the cursor again.
    pub struct Show;

    /// `ESC[row;colH`: put the cursor at a zero-based cell.
    pub struct MoveTo(pub u16, pub u16);

    impl Command for Hide {
        fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
            f.write_str("\x1b[?25l")
        }
    }

    impl Command for Show {
        fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
            f.write_str("\x1b[?25h")
        }
    }

    impl Command for MoveTo {
        fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
            // The sequence counts rows and columns from one; `MoveTo` takes them
            // from zero.
            write!(f, "\x1b[{};{}H", self.1 + 1, self.0 + 1)
        }
    }
}

pub mod style {
    use std::fmt;

    use super::Command;

    /// The attributes `more` sets: reverse video for the status bar and the
    /// search matches, and the reset that ends them.
    pub enum Attribute {
        Reverse,
        Reset,
    }

    impl Command for Attribute {
        fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
            f.write_str(match self {
                Self::Reverse => "\x1b[7m",
                Self::Reset => "\x1b[0m",
            })
        }
    }

    impl fmt::Display for Attribute {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            // `more` formats the attribute straight into a `String`, and a
            // command only renders into an `fmt::Write`, so this is the one
            // place a buffer is spent.
            let mut rendered = String::new();
            self.write_ansi(&mut rendered)?;
            f.write_str(&rendered)
        }
    }
}

pub mod terminal {
    use std::fmt;
    use std::io;
    use std::sync::{Mutex, MutexGuard};

    use rustix::termios::{
        OptionalActions, Termios, isatty, tcgetattr, tcgetwinsize, tcsetattr,
    };

    use super::{Command, errno};

    /// What the pager needs to hand back when it leaves: the settings it found,
    /// from before it switched to raw mode.
    static ORIGINAL: Mutex<Option<Termios>> = Mutex::new(None);

    /// The saved settings, even if a thread panicked while holding them.
    fn original() -> MutexGuard<'static, Option<Termios>> {
        ORIGINAL.lock().unwrap_or_else(|held| held.into_inner())
    }

    /// Switch descriptor 0 to raw mode, remembering the settings to restore.
    ///
    /// A redirected standard input has no line discipline to switch off, so it
    /// is not an error (see the module comment).
    pub fn enable_raw_mode() -> io::Result<()> {
        let stdin = io::stdin();
        if !isatty(&stdin) {
            return Ok(());
        }
        let original_settings = tcgetattr(&stdin).map_err(errno)?;
        let mut raw = original_settings.clone();
        raw.make_raw();
        tcsetattr(&stdin, OptionalActions::Now, &raw).map_err(errno)?;
        *original() = Some(original_settings);
        Ok(())
    }

    /// Put back the settings `enable_raw_mode` saved.
    pub fn disable_raw_mode() -> io::Result<()> {
        if let Some(settings) = original().take() {
            tcsetattr(&io::stdin(), OptionalActions::Now, &settings).map_err(errno)?;
        }
        Ok(())
    }

    /// The console's size as `(columns, rows)`.
    ///
    /// Both descriptors are asked because a pager's output and its keys may
    /// arrive on different ones. The port's tty keeps no window size, so the
    /// answer is `(0, 0)` there, which `more` then replaces with `-n`.
    pub fn size() -> io::Result<(u16, u16)> {
        if let Ok(ws) = tcgetwinsize(&io::stdout()) {
            return Ok((ws.ws_col, ws.ws_row));
        }
        if let Ok(ws) = tcgetwinsize(&io::stdin()) {
            return Ok((ws.ws_col, ws.ws_row));
        }
        Ok((0, 0))
    }

    /// `ESC[?1049h`: switch to the alternate screen, so the pager's screen
    /// replaces what was on the console while it runs.
    pub struct EnterAlternateScreen;

    /// `ESC[?1049l`: switch back.
    pub struct LeaveAlternateScreen;

    /// Which part of the screen `Clear` erases.
    pub enum ClearType {
        All,
        FromCursorDown,
        CurrentLine,
    }

    /// Erase part of the screen.
    pub struct Clear(pub ClearType);

    impl Command for EnterAlternateScreen {
        fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
            f.write_str("\x1b[?1049h")
        }
    }

    impl Command for LeaveAlternateScreen {
        fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
            f.write_str("\x1b[?1049l")
        }
    }

    impl Command for Clear {
        fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
            f.write_str(match self.0 {
                ClearType::All => "\x1b[2J",
                ClearType::FromCursorDown => "\x1b[J",
                ClearType::CurrentLine => "\x1b[2K",
            })
        }
    }
}

pub mod event {
    use std::collections::VecDeque;
    use std::io::{self, BufRead};
    use std::sync::{Mutex, MutexGuard};
    use std::time::Duration;

    use rustix::event::{PollFd, PollFlags, Timespec, poll as poll_fds};

    use super::errno;

    /// How long an `ESC` with no second byte already queued waits for one
    /// before it is read as the Escape key on its own.
    const ESC_TIMEOUT: Duration = Duration::from_millis(25);

    /// A key the decoder names.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum KeyCode {
        Backspace,
        Enter,
        Left,
        Right,
        Up,
        Down,
        Home,
        End,
        PageUp,
        PageDown,
        Tab,
        Esc,
        Char(char),
    }

    /// The modifier keys held when a key arrived.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct KeyModifiers(u8);

    impl KeyModifiers {
        pub const NONE: Self = Self(0);
        pub const CONTROL: Self = Self(1);
    }

    /// Whether the key went down or came back up. A terminal line sends only
    /// the first; the second is named so the applet's arm for it still means
    /// what it says.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum KeyEventKind {
        Press,
        // Nothing on this port can produce a release event.
        #[allow(dead_code)]
        Release,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct KeyEvent {
        pub code: KeyCode,
        pub modifiers: KeyModifiers,
        pub kind: KeyEventKind,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Event {
        Key(KeyEvent),
        // A serial line reports no window size, so nothing on this port can
        // produce one; the applet's arm for it is still upstream's.
        #[allow(dead_code)]
        Resize(u16, u16),
    }

    /// The bytes taken from standard input and not yet turned into keys.
    ///
    /// std's standard input is buffered, and one read can bring back an escape
    /// sequence's three bytes or a burst of typed keys. Holding those in a
    /// queue this module can see is what lets `poll` answer "is a key waiting"
    /// without asking the descriptor, which has already given its bytes up.
    struct Keys {
        queued: VecDeque<u8>,
    }

    static KEYS: Mutex<Keys> = Mutex::new(Keys {
        queued: VecDeque::new(),
    });

    fn keys() -> MutexGuard<'static, Keys> {
        KEYS.lock().unwrap_or_else(|held| held.into_inner())
    }

    /// Whether a key is waiting on standard input within `timeout`.
    pub fn poll(timeout: Duration) -> io::Result<bool> {
        // The lock is taken before the descriptor is asked, so a byte that a
        // read brought back cannot be missed between the two.
        let keys = keys();
        if !keys.queued.is_empty() {
            return Ok(true);
        }
        // `None` is a descriptor that cannot be polled (VFS answers `POLLNVAL`
        // for anything it does not own), where a read is the only way to find
        // out, and it blocks rather than returns: that counts as ready, because
        // a read is what the applet does next.
        Ok(fd_ready(timeout)?.unwrap_or(true))
    }

    /// Read one event. Blocks until a key arrives, as crossterm's `read` does.
    pub fn read() -> io::Result<Event> {
        keys().event()
    }

    /// Readiness of descriptor 0 within `timeout`.
    ///
    /// `Some(true)` for data, `Some(false)` for the deadline passing, and `None`
    /// for a descriptor VFS does not poll (the `POLLNVAL`, `POLLERR` and
    /// `POLLHUP` answers), where only a read can say.
    fn fd_ready(timeout: Duration) -> io::Result<Option<bool>> {
        let stdin = io::stdin();
        let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
        let deadline = Timespec {
            tv_sec: i64::try_from(timeout.as_secs()).unwrap_or(i64::MAX),
            tv_nsec: i64::from(timeout.subsec_nanos()),
        };
        if poll_fds(&mut fds, Some(&deadline)).map_err(errno)? == 0 {
            return Ok(Some(false));
        }
        let ready = fds[0].revents();
        if ready.intersects(PollFlags::IN | PollFlags::RDNORM) {
            return Ok(Some(true));
        }
        Ok(None)
    }

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
        })
    }

    impl Keys {
        /// The next byte, blocking. End of input is an error rather than a
        /// silent key: the pager has nothing left to read keys from.
        fn byte(&mut self) -> io::Result<u8> {
            if let Some(byte) = self.queued.pop_front() {
                return Ok(byte);
            }
            self.fill()?;
            self.queued
                .pop_front()
                .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))
        }

        /// Take everything std's buffer is holding, blocking only when it is
        /// empty.
        fn fill(&mut self) -> io::Result<()> {
            let stdin = io::stdin();
            let mut buffered = stdin.lock();
            let available = buffered.fill_buf()?;
            self.queued.extend(available.iter().copied());
            let taken = available.len();
            buffered.consume(taken);
            Ok(())
        }

        /// The next byte if one is queued or arrives within `timeout`.
        fn byte_within(&mut self, timeout: Duration) -> io::Result<Option<u8>> {
            if let Some(byte) = self.queued.pop_front() {
                return Ok(Some(byte));
            }
            if fd_ready(timeout)? != Some(true) {
                // Either the deadline passed, or the descriptor cannot be
                // polled: there is no second byte to wait for without blocking.
                return Ok(None);
            }
            match self.byte() {
                Ok(byte) => Ok(Some(byte)),
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
                Err(e) => Err(e),
            }
        }

        fn event(&mut self) -> io::Result<Event> {
            let first = self.byte()?;
            Ok(match first {
                0x1b => self.escape()?,
                b'\r' | b'\n' => key(KeyCode::Enter),
                0x7f | 0x08 => key(KeyCode::Backspace),
                b'\t' => key(KeyCode::Tab),
                0x00 => key(KeyCode::Char('\0')),
                // The control characters that stand for a letter: `^C` is 0x03.
                0x01..=0x1a => Event::Key(KeyEvent {
                    code: KeyCode::Char((b'a' + first - 1) as char),
                    modifiers: KeyModifiers::CONTROL,
                    kind: KeyEventKind::Press,
                }),
                _ => key(KeyCode::Char(self.character(first)?)),
            })
        }

        /// A key that started with `ESC`: a CSI or SS3 sequence, or a lone Esc.
        fn escape(&mut self) -> io::Result<Event> {
            let Some(next) = self.byte_within(ESC_TIMEOUT)? else {
                return Ok(key(KeyCode::Esc));
            };
            match next {
                b'[' => self.csi(),
                b'O' => {
                    let fin = self.byte()?;
                    Ok(key(match fin {
                        b'A' => KeyCode::Up,
                        b'B' => KeyCode::Down,
                        b'C' => KeyCode::Right,
                        b'D' => KeyCode::Left,
                        b'H' => KeyCode::Home,
                        b'F' => KeyCode::End,
                        _ => KeyCode::Esc,
                    }))
                }
                // `ESC` then anything else is a two-byte sequence this port's
                // terminals do not send; the Esc is the key it stands for.
                _ => Ok(key(KeyCode::Esc)),
            }
        }

        /// A CSI sequence: parameters, any number of intermediate bytes, then a
        /// final byte in `0x40..=0x7e`. Only the last parameter is kept, which
        /// is the one the keys this decoder names carry.
        fn csi(&mut self) -> io::Result<Event> {
            let mut param: u32 = 0;
            let mut private = false;
            loop {
                match self.byte()? {
                    digit @ b'0'..=b'9' => {
                        param = param
                            .saturating_mul(10)
                            .saturating_add(u32::from(digit - b'0'));
                    }
                    b';' => param = 0,
                    // A private sequence (`ESC[?25l`, `ESC[?1049h`) carries a
                    // marker no key this decoder names uses; drop it whole.
                    b'?' | b'<' | b'>' | b'=' => private = true,
                    // Intermediate bytes (`ESC[1 q`) are part of the sequence.
                    0x20..=0x2f => {}
                    fin => {
                        if private {
                            return Ok(key(KeyCode::Esc));
                        }
                        return Ok(key(sequence(fin, param)));
                    }
                }
            }
        }

        /// The character a printable (or non-ASCII) key stands for, reading the
        /// continuation bytes of a UTF-8 sequence.
        fn character(&mut self, first: u8) -> io::Result<char> {
            let len = match first {
                0xc0..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf7 => 4,
                // ASCII, and a continuation byte that arrived on its own: one
                // byte, and the latter decodes to the replacement character.
                _ => 1,
            };
            let mut buf = [0u8; 4];
            buf[0] = first;
            for slot in buf.iter_mut().take(len).skip(1) {
                *slot = self.byte()?;
            }
            Ok(std::str::from_utf8(&buf[..len])
                .ok()
                .and_then(|s| s.chars().next())
                .unwrap_or(char::REPLACEMENT_CHARACTER))
        }
    }

    /// The key a CSI final byte names, with the sequence's last parameter.
    fn sequence(fin: u8, param: u32) -> KeyCode {
        match fin {
            b'A' => KeyCode::Up,
            b'B' => KeyCode::Down,
            b'C' => KeyCode::Right,
            b'D' => KeyCode::Left,
            b'H' => KeyCode::Home,
            b'F' => KeyCode::End,
            b'Z' => KeyCode::Tab,
            b'~' => match param {
                1 | 7 => KeyCode::Home,
                2 => KeyCode::Tab,
                3 => KeyCode::Backspace,
                4 | 8 => KeyCode::End,
                5 => KeyCode::PageUp,
                6 => KeyCode::PageDown,
                _ => KeyCode::Esc,
            },
            _ => KeyCode::Esc,
        }
    }
}
