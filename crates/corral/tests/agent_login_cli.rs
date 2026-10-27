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

/// Whether this machine can run the pseudo-terminal driver at all. This is the only check that
/// may skip; once a scenario starts, every driver failure fails the test.
fn pty_prerequisites() -> bool {
    let probe = Command::new("python3")
        .args(["-I", "-c", "import os, pty, select, signal"])
        .stdin(Stdio::null())
        .output();
    match probe {
        Ok(output) if output.status.success() => true,
        other => {
            println!(
                "skipping the pseudo-terminal checks: python3 with pty is unavailable ({other:?})"
            );
            false
        }
    }
}

/// Runs `corral agent login` on a fresh pseudo-terminal, sends `keys` once the picker is drawn
/// and returns everything the terminal showed plus the exit code. A scenario that does not
/// finish within `timeout_secs` is killed and reaped, and reported as an error with its screen.
fn on_pty(home: &Path, keys: &[u8], timeout_secs: u32) -> Result<(String, i32), String> {
    const DRIVER: &str = r#"
import os, pty, select, signal, sys, time
binary, keys, timeout = sys.argv[1], bytes.fromhex(sys.argv[2]), float(sys.argv[3])
pid, fd = pty.fork()
if pid == 0:
    os.execv(binary, [binary, "agent", "login"])
deadline = time.monotonic() + timeout
seen = b""
def fail(reason):
    os.kill(pid, signal.SIGKILL)
    os.waitpid(pid, 0)
    sys.stdout.buffer.write(seen)
    sys.stdout.write("\nDRIVER_FAILED %s; child %d killed and reaped\n" % (reason, pid))
    sys.exit(3)
def pump(until=None):
    global seen
    while until is None or until not in seen:
        left = deadline - time.monotonic()
        if left <= 0:
            fail("timed out after %ss waiting for %r" % (timeout, until or "exit"))
        ready, _, _ = select.select([fd], [], [], left)
        if not ready:
            continue
        try:
            chunk = os.read(fd, 4096)
        except OSError:
            chunk = b""
        if not chunk:
            if until is not None:
                fail("the terminal closed before %r appeared" % until)
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
        .args([
            "-I",
            "-c",
            DRIVER,
            env!("CARGO_BIN_EXE_corral"),
            &hex,
            &timeout_secs.to_string(),
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("spawning the pseudo-terminal driver: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(format!(
            "driver exited {}: {text}\nstderr: {stderr}",
            output.status
        ));
    }
    let (screen, code) = text
        .rsplit_once("\nEXIT ")
        .ok_or_else(|| format!("driver output has no exit line: {text}"))?;
    let code = code
        .trim()
        .parse()
        .map_err(|error| format!("malformed exit line {code:?}: {error}"))?;
    Ok((screen.to_owned(), code))
}

fn scenario(home: &Path, keys: &[u8]) -> (String, i32) {
    on_pty(home, keys, 30)
        .unwrap_or_else(|error| panic!("pseudo-terminal scenario failed: {error}"))
}

#[test]
fn on_a_terminal_q_cancels_with_130_and_arrow_down_enter_selects_anthropic() {
    if !pty_prerequisites() {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    // When the owner presses arrow-down then q:
    let (screen, code) = scenario(home.path(), b"\x1b[Bq");
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
    let (screen, code) = scenario(home.path(), b"\x1b[B\r");
    assert!(screen.contains("assets_unavailable"), "{screen}");
    assert!(!screen.contains("login cancelled"), "{screen}");
    assert_eq!(code, 1, "{screen}");
    assert!(!home.path().join("data/corral/pi-native/auth").exists());
}

#[test]
fn a_hung_pty_scenario_is_a_failure_and_its_child_is_reaped() {
    if !pty_prerequisites() {
        return;
    }
    // Given a key the picker ignores, so login keeps waiting (the timeout is the behavior here):
    let home = tempfile::tempdir().unwrap();
    let error = on_pty(home.path(), b"x", 2).unwrap_err();
    // Then the driver reports the hang as an error, with the screen, after killing the child:
    assert!(error.contains("DRIVER_FAILED timed out"), "{error}");
    assert!(error.contains("killed and reaped"), "{error}");
    assert!(
        error.contains("Log in a model provider for pi-native"),
        "{error}"
    );
}
