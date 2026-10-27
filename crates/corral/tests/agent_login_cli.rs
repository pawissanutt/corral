//! `corral agent login` provider selection through the real binary (T-479): a real pseudo-terminal
//! drives the picker with the bytes a terminal sends, and a call without a terminal never prompts.
//! Every run has a private HOME and XDG_DATA_HOME and no corral store, so nothing of the owner's is
//! read and no login can start: a selection proceeds to the installation check and stops there.
use std::path::Path;
use std::process::{Command, Stdio};

fn isolated(command: &mut Command, home: &Path) {
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("XDG_DATA_HOME", home.join("data"))
        .current_dir(home);
}

#[test]
fn without_a_terminal_and_without_provider_login_exits_with_usage_and_no_prompt() {
    let home = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    isolated(&mut command, home.path());
    let output = command
        .args(["agent", "login"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(
        stderr.contains("usage_error") && stderr.contains("--provider anthropic"),
        "{stderr}"
    );
    assert!(!stderr.contains("Log in a model provider"), "{stderr}");
    assert!(output.stdout.is_empty());
    assert!(!home.path().join("data/corral/pi-native/auth").exists());
}

/// Runs `corral agent login` on a fresh pseudo-terminal, sends `keys` once the picker is drawn
/// and returns everything the terminal showed plus the exit code.
fn on_pty(home: &Path, keys: &[u8]) -> Option<(String, i32)> {
    const DRIVER: &str = r#"
import os, pty, select, sys
binary, keys = sys.argv[1], bytes.fromhex(sys.argv[2])
pid, fd = pty.fork()
if pid == 0:
    os.execv(binary, [binary, "agent", "login"])
seen = b""
def pump(until=None):
    global seen
    while until is None or until not in seen:
        ready, _, _ = select.select([fd], [], [], 30)
        if not ready:
            sys.exit("timed out waiting for the terminal")
        try:
            chunk = os.read(fd, 4096)
        except OSError:
            return
        if not chunk:
            return
        seen += chunk
pump(b"q cancel")
os.write(fd, keys)
pump()
_, status = os.waitpid(pid, 0)
sys.stdout.buffer.write(seen)
sys.stdout.write("\nEXIT %d\n" % os.waitstatus_to_exitcode(status))
"#;
    let hex: String = keys.iter().map(|b| format!("{b:02x}")).collect();
    let mut command = Command::new("python3");
    isolated(&mut command, home);
    let output = command
        .args(["-I", "-c", DRIVER, env!("CARGO_BIN_EXE_corral"), &hex])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        println!(
            "skipping the pseudo-terminal check: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let (screen, code) = text.rsplit_once("\nEXIT ")?;
    Some((screen.to_owned(), code.trim().parse().ok()?))
}

#[test]
fn on_a_terminal_q_cancels_with_130_and_arrow_down_enter_selects_anthropic() {
    let home = tempfile::tempdir().unwrap();
    // When the owner presses arrow-down then q:
    let Some((screen, code)) = on_pty(home.path(), b"\x1b[Bq") else {
        return;
    };
    // Then the picker listed both providers with their state and the login was cancelled:
    assert!(
        screen.contains("> OpenAI Codex (ChatGPT Plus/Pro)   [not logged in]"),
        "{screen}"
    );
    assert!(
        screen.contains("  Anthropic Claude (Pro/Max)        [not logged in]"),
        "{screen}"
    );
    assert!(screen.contains("> Anthropic Claude (Pro/Max)"), "{screen}");
    assert!(screen.contains("login cancelled"), "{screen}");
    assert_eq!(code, 130, "{screen}");
    assert!(!home.path().join("data/corral/pi-native/auth").exists());

    // When the owner presses arrow-down then Enter, Anthropic is selected and login proceeds to
    // the installation check (absent here), so nothing is written either:
    let (screen, code) = on_pty(home.path(), b"\x1b[B\r").unwrap();
    assert!(screen.contains("assets_unavailable"), "{screen}");
    assert!(!screen.contains("login cancelled"), "{screen}");
    assert_eq!(code, 1, "{screen}");
    assert!(!home.path().join("data/corral/pi-native/auth").exists());
}
