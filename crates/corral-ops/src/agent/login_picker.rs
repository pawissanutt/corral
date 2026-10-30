//! The arrow-key provider picker of `corral agent login` (T-479): one row per provider with its
//! login state from file metadata, arrows or j/k to move, Enter to select, q or Esc to cancel.
//! The key decoding and drawing run over any byte source, so tests drive the UI with the exact
//! bytes a terminal sends; `Tty` puts the real terminal in raw mode only while it is shown.
use std::io::Write;

use anyhow::{Context, Result};

pub const TITLE: &str = "Log in a model provider for pi-native";
pub const HINT: &str = "\u{2191}/\u{2193} move \u{b7} Enter select \u{b7} q cancel";
const LABEL_WIDTH: usize = 34;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Enter,
    Cancel,
    Other,
}

/// Bytes from the terminal. `next(false)` returns only a byte that is already pending, which
/// tells a lone Esc from the start of an arrow-key sequence.
pub trait KeySource {
    fn next(&mut self, wait: bool) -> Result<Option<u8>>;
}

impl KeySource for std::collections::VecDeque<u8> {
    fn next(&mut self, _wait: bool) -> Result<Option<u8>> {
        Ok(self.pop_front())
    }
}

pub fn read_key(source: &mut impl KeySource) -> Result<Key> {
    Ok(match source.next(true)? {
        None | Some(b'q' | b'Q' | 0x03 | 0x04) => Key::Cancel,
        Some(b'\r' | b'\n') => Key::Enter,
        Some(b'k') => Key::Up,
        Some(b'j') => Key::Down,
        Some(0x1b) => match source.next(false)? {
            None => Key::Cancel,
            Some(b'[' | b'O') => match source.next(false)? {
                Some(b'A') => Key::Up,
                Some(b'B') => Key::Down,
                _ => Key::Other,
            },
            Some(_) => Key::Other,
        },
        Some(_) => Key::Other,
    })
}

pub struct Row<'a> {
    pub label: &'a str,
    pub logged_in: bool,
}

fn draw(out: &mut impl Write, rows: &[Row<'_>], selected: usize, redraw: bool) -> Result<()> {
    if redraw {
        write!(out, "\x1b[{}A\x1b[J", rows.len() + 2)?;
    }
    write!(out, "{TITLE}\r\n")?;
    for (index, row) in rows.iter().enumerate() {
        let marker = if index == selected { '>' } else { ' ' };
        let state = if row.logged_in {
            "[logged in]"
        } else {
            "[not logged in]"
        };
        write!(
            out,
            "{marker} {:<width$}{state}\r\n",
            row.label,
            width = LABEL_WIDTH
        )?;
    }
    write!(out, "{HINT}\r\n")?;
    out.flush().context("drawing the provider picker")
}

/// Shows the picker and returns the selected row, or `None` when the owner cancels.
pub fn pick(
    source: &mut impl KeySource,
    out: &mut impl Write,
    rows: &[Row<'_>],
) -> Result<Option<usize>> {
    let mut selected = 0;
    draw(out, rows, selected, false)?;
    loop {
        match read_key(source)? {
            Key::Enter => return Ok(Some(selected)),
            Key::Cancel => return Ok(None),
            Key::Up => selected = selected.saturating_sub(1),
            Key::Down => selected = (selected + 1).min(rows.len().saturating_sub(1)),
            Key::Other => continue,
        }
        draw(out, rows, selected, true)?;
    }
}

/// Standard input in raw mode for as long as this lives; the saved mode is restored on drop.
#[cfg(unix)]
pub struct Tty {
    saved: rustix::termios::Termios,
}

#[cfg(unix)]
impl Tty {
    pub fn open() -> Result<Tty> {
        use rustix::termios::{OptionalActions, tcgetattr, tcsetattr};
        let stdin = std::io::stdin();
        let saved = tcgetattr(&stdin).context("reading the terminal mode")?;
        let mut raw = saved.clone();
        raw.make_raw();
        tcsetattr(&stdin, OptionalActions::Flush, &raw).context("entering raw terminal mode")?;
        Ok(Tty { saved })
    }
}

#[cfg(unix)]
impl Drop for Tty {
    fn drop(&mut self) {
        use rustix::termios::{OptionalActions, tcsetattr};
        let _ = tcsetattr(std::io::stdin(), OptionalActions::Flush, &self.saved);
    }
}

#[cfg(unix)]
impl KeySource for Tty {
    fn next(&mut self, wait: bool) -> Result<Option<u8>> {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        let stdin = std::io::stdin();
        if !wait {
            // An escape sequence arrives in one write; a lone Esc has nothing behind it.
            let pending = Timespec {
                tv_sec: 0,
                tv_nsec: 50_000_000,
            };
            let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
            if poll(&mut fds, Some(&pending)).context("polling the terminal")? == 0 {
                return Ok(None);
            }
        }
        let mut byte = [0u8; 1];
        let read = rustix::io::read(&stdin, &mut byte).context("reading the terminal")?;
        Ok((read == 1).then_some(byte[0]))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    fn rows() -> [Row<'static>; 2] {
        [
            Row {
                label: "OpenAI Codex (ChatGPT Plus/Pro)",
                logged_in: true,
            },
            Row {
                label: "Anthropic Claude (Pro/Max)",
                logged_in: false,
            },
        ]
    }

    fn drive(keys: &[u8]) -> (Option<usize>, String) {
        let mut source: VecDeque<u8> = keys.iter().copied().collect();
        let mut out = Vec::new();
        let picked = pick(&mut source, &mut out, &rows()).unwrap();
        (picked, String::from_utf8(out).unwrap())
    }

    #[test]
    fn first_frame_lists_each_provider_with_its_login_state() {
        let (_, screen) = drive(b"q");
        assert_eq!(
            screen,
            "Log in a model provider for pi-native\r\n\
             > OpenAI Codex (ChatGPT Plus/Pro)   [logged in]\r\n  \
             Anthropic Claude (Pro/Max)        [not logged in]\r\n\
             \u{2191}/\u{2193} move \u{b7} Enter select \u{b7} q cancel\r\n"
        );
    }

    #[test]
    fn arrow_down_then_enter_selects_anthropic_and_redraws_the_cursor() {
        let (picked, screen) = drive(b"\x1b[B\r");
        assert_eq!(picked, Some(1));
        let last = screen.rsplit("\x1b[J").next().unwrap();
        assert!(last.contains("> Anthropic Claude (Pro/Max)"), "{last:?}");
        assert!(last.contains("  OpenAI Codex"), "{last:?}");
    }

    #[test]
    fn j_k_and_application_mode_arrows_move_and_stay_in_bounds() {
        assert_eq!(drive(b"j\r").0, Some(1));
        assert_eq!(drive(b"jjjj\r").0, Some(1));
        assert_eq!(drive(b"jk\r").0, Some(0));
        assert_eq!(drive(b"\x1bOB\x1b[A\r").0, Some(0));
        assert_eq!(drive(b"\x1b[B\n").0, Some(1));
        assert_eq!(drive(b"x\x1b[C\r").0, Some(0));
    }

    #[test]
    fn q_esc_ctrl_c_and_end_of_input_cancel() {
        for keys in [&b"q"[..], b"Q", b"\x1b", b"\x03", b"\x1b[Bq", b""] {
            assert_eq!(drive(keys).0, None, "{keys:?}");
        }
    }
}
