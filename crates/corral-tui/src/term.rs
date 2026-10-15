//! Terminal mouse modes. corral enables click (`?1000`), drag (`?1002`) and SGR coordinates (`?1006`) but never
//! any-motion `?1003`, which floods events inside herdr (docs/DESIGN.md §10).

use std::io::Write;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};

pub const MOUSE_ON: &[u8] = b"\x1b[?1000h\x1b[?1002h\x1b[?1006h";
pub const MOUSE_OFF: &[u8] = b"\x1b[?1006l\x1b[?1002l\x1b[?1000l";

static ENABLED: AtomicBool = AtomicBool::new(false);
static HOOK: Once = Once::new();

fn write_stdout(bytes: &[u8]) -> std::io::Result<()> {
    let mut out = std::io::stdout();
    out.write_all(bytes)?;
    out.flush()
}

fn disable() {
    if ENABLED.swap(false, Ordering::SeqCst) {
        // Best effort: on a closed or broken stdout there is no terminal left to restore.
        let _ = write_stdout(MOUSE_OFF);
    }
}

/// Mouse reporting for the lifetime of the guard. Dropping it, or a panic anywhere while it lives, turns the
/// modes back off. Create it after `ratatui::try_init` so the panic hook runs before ratatui's restore.
pub struct MouseGuard(());

impl MouseGuard {
    pub fn enable() -> Result<MouseGuard> {
        HOOK.call_once(|| {
            let prev = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                disable();
                prev(info);
            }));
        });
        write_stdout(MOUSE_ON).context("enabling terminal mouse reporting")?;
        ENABLED.store(true, Ordering::SeqCst);
        Ok(MouseGuard(()))
    }
}

impl Drop for MouseGuard {
    fn drop(&mut self) {
        disable();
    }
}

/// Whether to capture the mouse: off when `--no-mouse` was passed, when `CORRAL_MOUSE` is `0`/`false`/`off`/`no`,
/// or when `[ui] mouse = false`; on otherwise.
pub fn mouse_wanted(no_mouse_flag: bool, env: Option<&str>, config: bool) -> bool {
    let env_off = env.is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        )
    });
    !no_mouse_flag && !env_off && config
}

/// The width of the terminal stdout is attached to, asked of stdout's own file descriptor. The controlling
/// terminal is deliberately not consulted: it is a different terminal whenever output goes to a pty the caller
/// owns, or stdin is redirected. None when stdout is not a terminal.
#[cfg(unix)]
pub fn width() -> Option<u16> {
    let size = rustix::termios::tcgetwinsize(std::io::stdout()).ok()?;
    (size.ws_col > 0).then_some(size.ws_col)
}

#[cfg(not(unix))]
pub fn width() -> Option<u16> {
    ratatui::crossterm::terminal::size()
        .ok()
        .map(|(w, _)| w)
        .filter(|w| *w > 0)
}

#[cfg(test)]
mod tests {
    use super::{MOUSE_OFF, MOUSE_ON, MouseGuard, mouse_wanted};

    const CHILD: &str = "CORRAL_MOUSE_GUARD_CHILD";

    fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    fn child_stdout(test: &str, mode: &str) -> Vec<u8> {
        let exe = std::env::current_exe().unwrap();
        let root = tempfile::tempdir().unwrap();
        let out = crate::board::tests::fixture_command(exe, root.path())
            .args(["--exact", test, "--nocapture", "--test-threads=1"])
            .env(CHILD, mode)
            .output()
            .unwrap();
        out.stdout
    }

    fn assert_restored(stdout: &[u8], what: &str) {
        let on = find(stdout, MOUSE_ON).unwrap_or_else(|| panic!("{what}: modes never enabled"));
        let off = find(&stdout[on..], MOUSE_OFF).unwrap_or_else(|| {
            let text = String::from_utf8_lossy(stdout);
            panic!("{what}: modes not restored: {text:?}")
        });
        assert!(off > 0);
    }

    #[test]
    fn mouse_guard_restores_on_drop() {
        if std::env::var(CHILD).as_deref() == Ok("drop") {
            drop(MouseGuard::enable().unwrap());
            return;
        }
        let out = child_stdout("term::tests::mouse_guard_restores_on_drop", "drop");
        assert_restored(&out, "normal quit");
    }

    #[test]
    fn mouse_guard_restores_from_panic_hook() {
        if std::env::var(CHILD).as_deref() == Ok("panic") {
            // Leak the guard so only the panic hook can restore the terminal.
            std::mem::forget(MouseGuard::enable().unwrap());
            panic!("forced panic with mouse capture on");
        }
        let out = child_stdout("term::tests::mouse_guard_restores_from_panic_hook", "panic");
        assert_restored(&out, "panic");
    }

    #[test]
    fn mouse_off_switches() {
        assert!(mouse_wanted(false, None, true));
        assert!(!mouse_wanted(true, None, true), "--no-mouse");
        for v in ["0", "false", "OFF", "no"] {
            assert!(!mouse_wanted(false, Some(v), true), "CORRAL_MOUSE={v}");
        }
        assert!(mouse_wanted(false, Some("1"), true));
        assert!(!mouse_wanted(false, None, false), "[ui] mouse = false");
    }
}
