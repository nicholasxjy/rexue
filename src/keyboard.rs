//! Normalize extended keyboard reports before interactive fzf reads its terminal.
//! Catalog stdin and selection stdout retain their existing, separate protocols.
use anyhow::{Context, Result, anyhow};
use std::{
    collections::VecDeque,
    ffi::CStr,
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::net::UnixStream,
    },
    process::Command,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const ESCAPE_DELAY: Duration = Duration::from_millis(50);
const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

pub(crate) struct Keyboard {
    terminal: File,
    saved: libc::termios,
    master: File,
    slave: File,
    stop: UnixStream,
    wake: Option<UnixStream>,
    worker: Option<JoinHandle<io::Result<()>>>,
}

impl Keyboard {
    pub(crate) fn prepare(command: &mut Command) -> Result<Option<Self>> {
        // Non-terminal test doubles and fzf's own missing-tty diagnostics still work.
        let Ok(terminal) = OpenOptions::new().read(true).write(true).open("/dev/tty") else {
            return Ok(None);
        };
        let mut saved = std::mem::MaybeUninit::uninit();
        if unsafe { libc::tcgetattr(terminal.as_raw_fd(), saved.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error()).context("read picker terminal settings");
        }
        let saved = unsafe { saved.assume_init() };
        let mut raw = saved;
        unsafe { libc::cfmakeraw(&mut raw) };
        let mut master = -1;
        let mut slave = -1;
        let mut name = [0 as libc::c_char; 1024];
        if unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                name.as_mut_ptr(),
                &mut raw,
                std::ptr::null_mut(),
            )
        } != 0
        {
            return Err(io::Error::last_os_error()).context("create fzf keyboard terminal");
        }
        // openpty transferred both descriptors to us, including on later errors.
        let master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        for file in [&master, &slave] {
            if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
                return Err(io::Error::last_os_error())
                    .context("protect picker terminal descriptor");
            }
        }
        let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
        if flags == -1
            || unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                == -1
        {
            return Err(io::Error::last_os_error()).context("configure picker terminal relay");
        }
        copy_size(&terminal, &slave)?;
        let (stop, wake) = UnixStream::pair()?;
        let name = unsafe { CStr::from_ptr(name.as_ptr()) }.to_str()?;
        // Supported since fzf 0.63. Both keyboard input and terminal rendering use
        // this PTY, while stdout remains the machine-readable selection pipe.
        command.arg("--tty-default").arg(name);
        if unsafe { libc::tcsetattr(terminal.as_raw_fd(), libc::TCSANOW, &raw) } != 0 {
            return Err(io::Error::last_os_error()).context("prepare picker keyboard input");
        }
        Ok(Some(Self {
            terminal,
            saved,
            master,
            slave,
            stop,
            wake: Some(wake),
            worker: None,
        }))
    }

    pub(crate) fn resize_handles(&self) -> Result<(File, File)> {
        Ok((self.terminal.try_clone()?, self.slave.try_clone()?))
    }

    pub(crate) fn saved_modes(&self) -> libc::termios {
        self.saved
    }

    pub(crate) fn start(&mut self, pid: u32) -> Result<()> {
        let terminal = self.terminal.try_clone()?;
        let master = self.master.try_clone()?;
        let wake = self.wake.take().context("keyboard relay already started")?;
        self.worker = Some(thread::Builder::new().name("fzf-keyboard".into()).spawn(
            move || {
                let result = relay(terminal, master, wake);
                if result.is_err() {
                    unsafe {
                        libc::kill(pid as i32, libc::SIGTERM);
                    }
                }
                result
            },
        )?);
        Ok(())
    }

    pub(crate) fn finish(&mut self) -> Result<()> {
        if let Some(worker) = self.worker.take() {
            let _ = self.stop.write_all(&[0]);
            worker
                .join()
                .map_err(|_| anyhow!("picker keyboard relay panicked"))?
                .map_err(|error| anyhow!("picker keyboard relay failed: {error}"))?;
        }
        Ok(())
    }
}

impl Drop for Keyboard {
    fn drop(&mut self) {
        let _ = self.finish();
        unsafe {
            libc::tcsetattr(self.terminal.as_raw_fd(), libc::TCSANOW, &self.saved);
        }
    }
}

pub(crate) fn set_raw(terminal: &File, saved: &libc::termios, raw: bool) -> io::Result<()> {
    let mut modes = *saved;
    if raw {
        unsafe {
            libc::cfmakeraw(&mut modes);
        }
    }
    if unsafe { libc::tcsetattr(terminal.as_raw_fd(), libc::TCSANOW, &modes) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(crate) fn copy_size(terminal: &File, slave: &File) -> io::Result<()> {
    let mut size = std::mem::MaybeUninit::<libc::winsize>::uninit();
    if unsafe { libc::ioctl(terminal.as_raw_fd(), libc::TIOCGWINSZ, size.as_mut_ptr()) } == -1
        || unsafe { libc::ioctl(slave.as_raw_fd(), libc::TIOCSWINSZ, size.as_ptr()) } == -1
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn drain_output(master: &mut File, terminal: &mut File) -> io::Result<()> {
    let mut bytes = [0; 8192];
    loop {
        match master.read(&mut bytes) {
            Ok(0) => return Ok(()),
            Ok(n) => terminal.write_all(&bytes[..n])?,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}

fn relay(mut terminal: File, mut master: File, wake: UnixStream) -> io::Result<()> {
    let mut decoder = Decoder::default();
    let mut input = VecDeque::new();
    let mut pending_since: Option<Instant> = None;
    let mut bytes = [0; 8192];
    let terminal_fd = terminal.as_raw_fd();
    let master_fd = master.as_raw_fd();
    let wake_fd = wake.as_raw_fd();
    let nfds = terminal_fd.max(master_fd).max(wake_fd) + 1;
    if nfds as usize > libc::FD_SETSIZE {
        return Err(io::Error::other(
            "picker terminal descriptor exceeds select limit",
        ));
    }
    loop {
        if pending_since.is_some_and(|since| since.elapsed() >= ESCAPE_DELAY) {
            input.extend(decoder.flush());
            pending_since = None;
        }
        // macOS poll reports POLLNVAL for /dev/tty. select supports the real
        // terminal, private PTY, and cancellation socket on both macOS and Linux.
        let mut readable = unsafe { std::mem::zeroed::<libc::fd_set>() };
        let mut writable = unsafe { std::mem::zeroed::<libc::fd_set>() };
        unsafe {
            if input.len() < 65536 {
                libc::FD_SET(terminal_fd, &mut readable);
            }
            libc::FD_SET(master_fd, &mut readable);
            libc::FD_SET(wake_fd, &mut readable);
            if !input.is_empty() {
                libc::FD_SET(master_fd, &mut writable);
            }
        }
        let mut timeout = libc::timeval {
            tv_sec: 0,
            tv_usec: pending_since.map_or(0, |since| {
                ESCAPE_DELAY
                    .saturating_sub(since.elapsed())
                    .as_micros()
                    .max(1) as _
            }),
        };
        let timeout_ptr = if pending_since.is_some() {
            &mut timeout as *mut _
        } else {
            std::ptr::null_mut()
        };
        if unsafe {
            libc::select(
                nfds,
                &mut readable,
                &mut writable,
                std::ptr::null_mut(),
                timeout_ptr,
            )
        } == -1
        {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if unsafe { libc::FD_ISSET(wake_fd, &readable) } {
            // Deliver fzf's final screen and mode restoration before stopping.
            return drain_output(&mut master, &mut terminal);
        }
        if unsafe { libc::FD_ISSET(master_fd, &readable) } {
            drain_output(&mut master, &mut terminal)?;
        }
        if unsafe { libc::FD_ISSET(terminal_fd, &readable) } {
            match terminal.read(&mut bytes) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "picker terminal closed",
                    ));
                }
                Ok(n) => {
                    input.extend(decoder.push(&bytes[..n]));
                    pending_since = (!decoder.pending.is_empty()).then(Instant::now);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        if unsafe { libc::FD_ISSET(master_fd, &writable) } && !input.is_empty() {
            match master.write(input.make_contiguous()) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "fzf terminal closed",
                    ));
                }
                Ok(n) => {
                    input.drain(..n);
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(error),
            }
        }
    }
}

#[derive(Default)]
struct Decoder {
    pending: Vec<u8>,
    paste: bool,
}

impl Decoder {
    fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(bytes);
        let mut output = Vec::new();
        let mut offset = 0;
        while offset < self.pending.len() {
            let rest = &self.pending[offset..];
            if self.paste {
                if rest.starts_with(PASTE_END) {
                    self.paste = false;
                    output.extend_from_slice(PASTE_END);
                    offset += PASTE_END.len();
                } else if PASTE_END.starts_with(rest) {
                    break;
                } else {
                    output.push(rest[0]);
                    offset += 1;
                }
                continue;
            }
            if rest[0] != 0x1b || (rest.len() > 1 && rest[1] != b'[') {
                output.push(rest[0]);
                offset += 1;
                continue;
            }
            let end = rest
                .iter()
                .enumerate()
                .skip(2)
                .find(|(_, byte)| (0x40..=0x7e).contains(*byte))
                .map(|(i, _)| i + 1);
            let Some(end) = end else {
                if rest.len() <= 256 {
                    break;
                }
                output.push(rest[0]);
                offset += 1;
                continue;
            };
            let sequence = &rest[..end];
            if sequence == PASTE_START {
                self.paste = true;
            }
            output.extend(legacy_key(sequence).unwrap_or_else(|| sequence.to_vec()));
            offset += end;
        }
        self.pending.drain(..offset);
        output
    }

    fn flush(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending)
    }
}

fn legacy_key(sequence: &[u8]) -> Option<Vec<u8>> {
    let final_byte = *sequence.last()?;
    if !matches!(final_byte, b'u' | b'~') {
        return None;
    }
    let fields: Vec<_> = std::str::from_utf8(&sequence[2..sequence.len() - 1])
        .ok()?
        .split(';')
        .collect();
    let (key, modifier, text) = if final_byte == b'~' {
        if fields.len() != 3 || fields[0] != "27" {
            return None;
        }
        (fields[2], fields[1], None)
    } else {
        if fields.is_empty() || fields.len() > 3 {
            return None;
        }
        (
            fields[0],
            *fields.get(1).unwrap_or(&"1"),
            fields.get(2).copied(),
        )
    };
    let keys: Vec<_> = key.split(':').collect();
    let code: u32 = keys.first()?.parse().ok()?;
    let modifiers: Vec<_> = modifier.split(':').collect();
    let flags = modifiers.first()?.parse::<u32>().ok()?.checked_sub(1)?;
    let event = modifiers
        .get(1)
        .map_or(Some(1), |value| value.parse::<u32>().ok())?;
    // Super/Hyper/Meta have no equivalent in fzf's legacy input protocol.
    if flags & !(1 | 2 | 4 | 64 | 128) != 0 || !(1..=3).contains(&event) {
        return None;
    }
    let shifted = flags & 1 != 0;
    let alt = flags & 2 != 0;
    let ctrl = flags & 4 != 0;
    let mut key = if ctrl {
        match code {
            32 | 64 => vec![0],
            65..=95 | 97..=122 => vec![(code as u8) & 0x1f],
            63 | 127 => vec![127],
            9 | 13 | 27 => vec![code as u8],
            _ => return None,
        }
    } else if code == 9 && shifted {
        b"\x1b[Z".to_vec()
    } else if matches!(code, 9 | 13 | 27 | 127) {
        vec![code as u8]
    } else {
        if (57344..=63743).contains(&code) {
            return None;
        }
        let mut text_bytes = Vec::new();
        if let Some(text) = text.filter(|text| !text.is_empty()) {
            for code in text.split(':') {
                let character = char::from_u32(code.parse().ok()?)?;
                let mut utf8 = [0; 4];
                text_bytes.extend_from_slice(character.encode_utf8(&mut utf8).as_bytes());
            }
        } else {
            let code = if shifted {
                keys.get(1)
                    .filter(|key| !key.is_empty())
                    .map_or(Some(code), |key| key.parse().ok())?
            } else {
                code
            };
            let character = char::from_u32(code)?;
            let mut utf8 = [0; 4];
            text_bytes.extend_from_slice(character.encode_utf8(&mut utf8).as_bytes());
        }
        text_bytes
    };
    if event == 3 {
        return Some(Vec::new());
    }
    if alt {
        key.insert(0, 0x1b);
    }
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_reports_match_legacy_control_bytes() {
        for (down, up) in [
            (b"\x1b[106;5u".as_slice(), b"\x1b[107;5u".as_slice()),
            (b"\x1b[106;5;106u", b"\x1b[107;5;107u"),
            (b"\x1b[106;5:2u", b"\x1b[107;5:2u"),
            (b"\x1b[106:74:106;5u", b"\x1b[107:75:107;5u"),
            (b"\x1b[27;5;106~", b"\x1b[27;5;107~"),
        ] {
            assert_eq!(Decoder::default().push(down), b"\n");
            assert_eq!(Decoder::default().push(up), b"\x0b");
        }
    }

    #[test]
    fn fragmented_sequences_are_decoded_once() {
        let report = b"\x1b[106;5:2;106u";
        for split in 0..=report.len() {
            let mut decoder = Decoder::default();
            let mut output = decoder.push(&report[..split]);
            output.extend(decoder.push(&report[split..]));
            assert_eq!(output, b"\n", "split at {split}");
        }
        let mut decoder = Decoder::default();
        let output: Vec<_> = report
            .iter()
            .flat_map(|byte| decoder.push(&[*byte]))
            .collect();
        assert_eq!(output, b"\n");
    }

    #[test]
    fn key_release_does_not_repeat_a_navigation_action() {
        let input = b"\x1b[106;5:1u\x1b[106;5:3u\x1b[106;5:2u";
        assert_eq!(Decoder::default().push(input), b"\n\n");
    }

    #[test]
    fn ordinary_input_and_unrecognized_terminal_sequences_are_preserved() {
        let input =
            "中文\n\u{b}\u{1b}[A\u{1b}[1;5D\u{1b}[?1u\u{1b}[12;34R\u{1b}[57352;5u\u{1b}[106;9u";
        assert_eq!(Decoder::default().push(input.as_bytes()), input.as_bytes());
    }

    #[test]
    fn pasted_keyboard_sequences_remain_literal_even_when_fragmented() {
        let input = b"\x1b[200~\x1b[106;5u\x1b[27;5;107~\x1b[201~\x1b[106;5u";
        let expected = b"\x1b[200~\x1b[106;5u\x1b[27;5;107~\x1b[201~\n";
        let mut decoder = Decoder::default();
        let output: Vec<_> = input
            .iter()
            .flat_map(|byte| decoder.push(&[*byte]))
            .collect();
        assert_eq!(output, expected);
    }

    #[test]
    fn text_modifiers_and_other_picker_controls_have_legacy_equivalents() {
        let input = b"\x1b[99;5u\x1b[106;69u\x1b[74;6u\x1b[97:65;2u\x1b[97;1;20013u\x1b[120;3u\x1b[9;2u\x1b[13u\x1b[27u";
        assert_eq!(
            Decoder::default().push(input),
            "\u{3}\n\nA中\u{1b}x\u{1b}[Z\r\u{1b}".as_bytes()
        );
    }

    #[test]
    fn solitary_escape_and_incomplete_reports_can_be_flushed() {
        for input in [b"\x1b".as_slice(), b"\x1b[106;", b"\x1b[27;5;"] {
            let mut decoder = Decoder::default();
            assert!(decoder.push(input).is_empty());
            assert_eq!(decoder.flush(), input);
            assert!(decoder.pending.is_empty());
        }
    }
}
