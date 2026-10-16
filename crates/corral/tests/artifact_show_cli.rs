use std::path::Path;
use std::process::{Command, Output, Stdio};

use corral_core::{PinRef, RetentionAction, Status, Store};
use serde_json::Value;

fn invoke(root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("CORRAL_") || name.starts_with("HERDR_") || name == "COLUMNS" {
            command.env_remove(key);
        }
    }
    command
        .env("CORRAL_ROOT", root)
        .env("CORRAL_HERDR_SESSION", "corral-test")
        .arg("--root")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn ok(root: &Path, args: &[&str]) -> String {
    let output = invoke(root, args);
    assert!(output.status.success(), "{args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

struct Fx {
    tmp: tempfile::TempDir,
    store: Store,
    task: String,
}

impl Fx {
    fn new(action: RetentionAction) -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = Store::init(tmp.path()).unwrap();
        store.config.artifacts.retention.action = action;
        store.config.artifacts.retention.larger_than = Some("1B".into());
        store.save_config().unwrap();
        let task = store.create("Show CLI", None, Status::Done, "").unwrap().id;
        Fx { tmp, store, task }
    }

    fn root(&self) -> &Path {
        self.tmp.path()
    }

    fn add(&self, name: &str, body: &str, kind: &str, title: &str) -> String {
        let path = self.root().join(name);
        std::fs::write(&path, body).unwrap();
        ok(
            self.root(),
            &[
                "artifact",
                "add",
                &self.task,
                path.to_str().unwrap(),
                "--kind",
                kind,
                "--title",
                title,
            ],
        );
        let a = self
            .store
            .get(&self.task)
            .unwrap()
            .artifacts
            .into_iter()
            .find(|a| a.title == title)
            .unwrap();
        PinRef::of_artifact(&self.task, &a).to_string()
    }
}

#[test]
fn show_renders_markdown_for_reading_and_raw_prints_the_stored_text() {
    let f = Fx::new(RetentionAction::Keep);
    let body = "# Report\n\n- first\n- second\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
    let reference = f.add("report.md", body, "report", "Report");
    let rendered = ok(f.root(), &["artifact", "show", &reference]);
    assert!(
        rendered.contains("Report\n") && !rendered.contains("# Report"),
        "{rendered}"
    );
    assert!(
        rendered.contains("• first") && rendered.contains("┌"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("\x1b"),
        "piped output carries no escapes: {rendered:?}"
    );
    assert_eq!(
        ok(f.root(), &["artifact", "show", &reference, "--raw"]),
        body
    );
}

#[test]
fn show_reads_the_whole_compressed_log_and_raw_matches_the_stored_bytes() {
    let f = Fx::new(RetentionAction::Compress);
    let log: String = (1..=2500)
        .map(|i| format!("line {i}\n"))
        .chain(["error: final failure\n".to_string()])
        .collect();
    let reference = f.add("long.log", &log, "log", "Long log");
    ok(f.root(), &["artifact", "gc", "--yes"]);
    let stored = f.store.get(&f.task).unwrap().artifacts.remove(0);
    assert!(stored.compressed_bytes.is_some(), "compressed on disk");
    let rendered = ok(f.root(), &["artifact", "show", &reference]);
    assert_eq!(
        rendered.lines().count(),
        2501,
        "complete, not a 2000-line preview"
    );
    assert!(rendered.trim_end().ends_with("error: final failure"));
    assert_eq!(
        ok(f.root(), &["artifact", "show", &reference, "--raw"]),
        log
    );
}

#[test]
fn show_prints_the_purge_stub_as_text() {
    let f = Fx::new(RetentionAction::Purge);
    let reference = f.add("old.md", "# Old\n", "report", "Old");
    ok(f.root(), &["artifact", "set-tier", &reference, "normal"]);
    ok(f.root(), &["artifact", "gc", "--yes"]);
    let shown = ok(f.root(), &["artifact", "show", &reference]);
    assert!(shown.contains("Artifact content purged"), "{shown}");
}

#[test]
fn show_line_ranges_select_exact_decoded_source_lines() {
    for action in [RetentionAction::Keep, RetentionAction::Compress] {
        let f = Fx::new(action);
        let first = format!("{}\r\n", "first ".repeat(2000));
        let body = format!("{first}\r\n# 三\r\nlast");
        let reference = f.add("range.log", &body, "log", "Range log");
        ok(f.root(), &["artifact", "gc", "--yes"]);
        if action == RetentionAction::Compress {
            assert!(
                f.store.get(&f.task).unwrap().artifacts[0]
                    .compressed_bytes
                    .is_some()
            );
        }
        f.store.pin(&reference, None, false).unwrap();
        for reference in [&reference[..], "1"] {
            for (range, expected) in [
                ("1-1", first.as_str()),
                ("2-3", "\r\n# 三\r\n"),
                ("4-4", "last"),
                ("3-100", "# 三\r\nlast"),
                ("5-100", ""),
            ] {
                for raw in [false, true] {
                    let mut args = vec!["artifact", "show", reference, "--lines", range];
                    if raw {
                        args.push("--raw");
                    }
                    assert_eq!(ok(f.root(), &args), expected, "{args:?}");
                }
            }
        }
    }
}

#[test]
fn show_line_ranges_handle_empty_files_and_trailing_newlines() {
    for (name, body, range, expected) in [
        ("empty.txt", "", "1-10", ""),
        ("trailing.txt", "one\n\n", "2-2", "\n"),
        ("no-phantom.txt", "one\n\n", "3-3", ""),
    ] {
        let f = Fx::new(RetentionAction::Keep);
        let reference = f.add(name, body, "evidence", name);
        assert_eq!(
            ok(
                f.root(),
                &["artifact", "show", &reference, "--lines", range]
            ),
            expected
        );
    }
}

#[test]
fn show_rejects_invalid_line_ranges_before_writing_content() {
    let f = Fx::new(RetentionAction::Keep);
    let reference = f.add("invalid.txt", "content\n", "evidence", "Invalid");
    for range in ["0-1", "3-2", "1", "1-", "1-2-3", "1-18446744073709551616"] {
        let output = invoke(
            f.root(),
            &["artifact", "show", &reference, "--lines", range],
        );
        assert!(!output.status.success(), "{range}");
        assert!(output.stdout.is_empty(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("line range"),
            "{output:?}"
        );
    }
}

#[test]
fn list_and_show_order_by_importance_tier_and_honor_overrides_and_filters() {
    let f = Fx::new(RetentionAction::Keep);
    f.add("a.log", "log body\n", "log", "Gate log");
    let evidence = f.add("b.txt", "evidence body\n", "evidence", "Plain evidence");
    f.add("c.md", "# S\n", "summary", "Summary");
    f.add("d.md", "# R\n", "report", "Report");
    let first = f.add("1.diff", "+1\n", "diff", "Old diff");
    f.add("2.diff", "+2\n", "diff", "Latest diff");
    ok(f.root(), &["artifact", "set-tier", &evidence, "transient"]);

    let listing = ok(f.root(), &["artifact", "list"]);
    let order: Vec<&str> = listing
        .lines()
        .map(|l| {
            [
                "Report",
                "Summary",
                "Latest diff",
                "Plain evidence",
                "Old diff",
                "Gate log",
            ]
            .into_iter()
            .find(|t| l.contains(&format!(" {t} ")))
            .unwrap_or("?")
        })
        .collect();
    assert_eq!(
        order[..3],
        ["Report", "Summary", "Latest diff"],
        "keep then normal first\n{listing}"
    );
    assert!(
        order[3..]
            .iter()
            .all(|t| ["Plain evidence", "Old diff", "Gate log"].contains(t)),
        "raw last\n{listing}"
    );

    let keep = ok(f.root(), &["artifact", "list", "--tier", "keep", "--json"]);
    let keep: Vec<Value> = serde_json::from_str(&keep).unwrap();
    assert_eq!(keep.len(), 2);
    assert!(keep.iter().all(|r| r["tier"] == "keep"));
    let transient = ok(f.root(), &["artifact", "list", "--tier", "transient"]);
    assert!(
        transient.contains("Old diff")
            && transient.contains("Plain evidence")
            && !transient.contains("Latest diff"),
        "{transient}"
    );
    assert!(
        !invoke(f.root(), &["artifact", "list", "--tier", "bogus"])
            .status
            .success()
    );
    let _ = first;

    let shown = ok(f.root(), &["show", &f.task]);
    let at = |needle: &str| {
        shown
            .find(needle)
            .unwrap_or_else(|| panic!("{needle}\n{shown}"))
    };
    assert!(at("[report] Report") < at("[summary] Summary"), "{shown}");
    assert!(
        at("[summary] Summary") < at("[diff] Latest diff"),
        "{shown}"
    );
    assert!(at("[diff] Latest diff") < at("raw (3)"), "{shown}");
    assert!(at("raw (3)") < at("[diff] Old diff"), "{shown}");
}

/// Runs `artifact show` with stdout on a pseudo-terminal of `cols` columns and `COLUMNS` unset.
fn show_on_pty(root: &Path, reference: &str, cols: u16) -> String {
    const HARNESS: &str = r#"import fcntl, os, pty, struct, subprocess, sys, termios
corral, root, ref, cols = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, cols, 0, 0))
# A decoy controlling terminal of another size: a size lookup that asks the controlling terminal instead of
# stdout (what happens when stdin is redirected) reports 50 columns here.
decoy_master, decoy = pty.openpty()
fcntl.ioctl(decoy, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 50, 0, 0))
def own_session():
    os.setsid()
    fcntl.ioctl(decoy, termios.TIOCSCTTY, 0)
env = {k: v for k, v in os.environ.items() if k != "COLUMNS" and not k.startswith(("CORRAL_", "HERDR_"))}
env["CORRAL_ROOT"] = root
env["CORRAL_HERDR_SESSION"] = "corral-test"
p = subprocess.Popen([corral, "--root", root, "artifact", "show", ref], stdin=subprocess.DEVNULL, stdout=slave, env=env, preexec_fn=own_session)
os.close(slave)
os.close(decoy)
out = b""
while True:
    try:
        chunk = os.read(master, 4096)
    except OSError:
        break
    if not chunk:
        break
    out += chunk
status = p.wait()
sys.stdout.buffer.write(out)
sys.exit(status)
"#;
    let output = Command::new("python3")
        .args(["-I", "-c", HARNESS])
        .arg(env!("CARGO_BIN_EXE_corral"))
        .arg(root)
        .arg(reference)
        .arg(cols.to_string())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn visible_width_of_lines(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    for line in text.split('\n') {
        let mut width = 0;
        let mut chars = line.trim_end_matches('\r').chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for e in chars.by_ref() {
                    if e.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                width += 1;
            }
        }
        out.push(width);
    }
    out
}

#[test]
fn show_lays_out_for_the_real_terminal_width_when_columns_is_unset() {
    let f = Fx::new(RetentionAction::Keep);
    let prose = "word ".repeat(60);
    let reference = f.add(
        "report.md",
        &format!("# Report\n\n{prose}\n"),
        "report",
        "Report",
    );
    for cols in [80u16, 132] {
        let screen = show_on_pty(f.root(), &reference, cols);
        assert!(
            screen.contains("\x1b["),
            "a terminal gets ANSI styling: {screen:?}"
        );
        let widths = visible_width_of_lines(&screen);
        assert_eq!(
            widths.iter().max().copied(),
            Some(usize::from(cols)),
            "{cols} columns: the heading rule spans the terminal\n{screen}"
        );
        assert!(
            widths.iter().all(|w| *w <= usize::from(cols)),
            "{cols} columns: nothing is wider than the terminal\n{screen}"
        );
    }
}
