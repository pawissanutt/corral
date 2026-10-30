#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    bin: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().canonicalize().expect("canonical");
        let bin = root.join("bin");
        std::fs::create_dir(&bin).expect("bin dir");
        Fixture {
            _tmp: tmp,
            root,
            bin,
        }
    }

    /// An executable `corral-ext-<name>` that appends its argv to `<name>.argv`, echoes one
    /// stdin line to stdout, writes to stderr and exits with `code`.
    fn ext(&self, name: &str, code: i32) -> PathBuf {
        let path = self.bin.join(format!("corral-ext-{name}"));
        let log = self.root.join(format!("{name}.argv"));
        let script = format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf '[%s]\\n' \"$a\"; done >> '{}'\n\
             IFS= read -r line; printf 'stdin=%s\\n' \"$line\"; printf 'ext stderr\\n' >&2\nexit {code}\n",
            log.display()
        );
        std::fs::write(&path, script).expect("write ext");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    fn argv(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join(format!("{name}.argv"))).ok()
    }

    fn path_var(&self, with_bin: bool) -> String {
        let system = std::env::var("PATH").unwrap_or_default();
        if with_bin {
            format!("{}:{system}", self.bin.display())
        } else {
            system
        }
    }

    fn run(&self, args: &[&str], stdin: &str, with_bin: bool) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_corral"))
            .env("PATH", self.path_var(with_bin))
            .env("CORRAL_ROOT", &self.root)
            .env_remove("CORRAL_GATE_ATTEMPT")
            .env_remove("CORRAL_CANDIDATE_HEAD")
            .env_remove("CORRAL_INTEGRATION_BASE")
            .env_remove("HERDR_PANE_ID")
            .env_remove("HERDR_WORKSPACE_ID")
            .env_remove("HERDR_TAB_ID")
            .env_remove("CORRAL_HERDR_SESSION")
            .env("HERDR_BIN_PATH", "/nonexistent/herdr")
            .args(args)
            .current_dir(&self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("corral runs");
        {
            use std::io::Write;
            let mut pipe = child.stdin.take().expect("stdin");
            pipe.write_all(stdin.as_bytes()).expect("write stdin");
        }
        child.wait_with_output().expect("corral exits")
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn unknown_name_forwards_exact_argv_inherited_stdio_and_exit_status() {
    let fx = Fixture::new();
    fx.ext("foo", 7);
    let out = fx.run(
        &["foo", "a", "b c", "$HOME", "*", "--help"],
        "hello\n",
        true,
    );
    assert_eq!(out.status.code(), Some(7), "stderr: {}", text(&out.stderr));
    assert_eq!(
        fx.argv("foo").as_deref(),
        Some("[a]\n[b c]\n[$HOME]\n[*]\n[--help]\n"),
        "argv reaches the child verbatim with no shell expansion"
    );
    assert_eq!(text(&out.stdout), "stdin=hello\n");
    assert_eq!(text(&out.stderr), "ext stderr\n");

    let root = fx.root.to_str().expect("utf8");
    let out = fx.run(&["--root", root, "foo", "-x"], "\n", true);
    assert_eq!(out.status.code(), Some(7));
    assert_eq!(
        fx.argv("foo").as_deref(),
        Some("[a]\n[b c]\n[$HOME]\n[*]\n[--help]\n[-x]\n"),
        "a leading global --root is consumed by corral, not forwarded"
    );
}

#[test]
fn builtin_wins_over_extension_and_ext_run_reaches_it() {
    let fx = Fixture::new();
    fx.ext("list", 3);
    let init = fx.run(&["init"], "", true);
    assert!(init.status.success(), "init: {}", text(&init.stderr));

    let builtin = fx.run(&["list", "--json"], "", true);
    assert!(builtin.status.success(), "list: {}", text(&builtin.stderr));
    let tasks: serde_json::Value =
        serde_json::from_slice(&builtin.stdout).expect("built-in list prints JSON");
    assert!(tasks.is_array());
    assert_eq!(
        fx.argv("list"),
        None,
        "built-in list never runs the extension"
    );

    let ext = fx.run(&["ext", "run", "list", "--json", "x y"], "in\n", true);
    assert_eq!(ext.status.code(), Some(3), "stderr: {}", text(&ext.stderr));
    assert_eq!(fx.argv("list").as_deref(), Some("[--json]\n[x y]\n"));
    assert_eq!(text(&ext.stdout), "stdin=in\n");
}

#[test]
fn ext_run_forwards_every_argument_after_name_including_corral_flags() {
    let fx = Fixture::new();
    fx.ext("foo", 7);
    fx.ext("list", 7);
    let cases: [&[&str]; 6] = [
        &["--help"],
        &["-h"],
        &["--root", "/tmp/root"],
        &["--root=/tmp/root"],
        &["--version"],
        &["--", "x"],
    ];
    for name in ["foo", "list"] {
        for case in cases {
            let _ = std::fs::remove_file(fx.root.join(format!("{name}.argv")));
            let mut args = vec!["ext", "run", name];
            args.extend_from_slice(case);
            let out = fx.run(&args, "", true);
            assert_eq!(
                out.status.code(),
                Some(7),
                "{args:?}: {}",
                text(&out.stderr)
            );
            let expected: String = case.iter().map(|a| format!("[{a}]\n")).collect();
            assert_eq!(
                fx.argv(name).as_deref(),
                Some(expected.as_str()),
                "{args:?}"
            );
        }
    }

    let root = fx.root.to_str().expect("utf8");
    let _ = std::fs::remove_file(fx.root.join("foo.argv"));
    let out = fx.run(
        &["--root", root, "ext", "run", "foo", "--root", "/tmp/root"],
        "",
        true,
    );
    assert_eq!(out.status.code(), Some(7));
    assert_eq!(fx.argv("foo").as_deref(), Some("[--root]\n[/tmp/root]\n"));

    let help = fx.run(&["ext", "run", "--help"], "", true);
    assert!(help.status.success(), "{}", text(&help.stderr));
    assert!(text(&help.stdout).contains("Usage: corral ext run [OPTIONS] <NAME> [ARGS]..."));

    fx.ext("help", 5);
    let out = fx.run(&["ext", "run", "help", "x"], "", true);
    assert_eq!(
        out.status.code(),
        Some(5),
        "explicit run reaches a reserved name"
    );
    assert_eq!(fx.argv("help").as_deref(), Some("[x]\n"));

    let missing = fx.run(&["ext", "run"], "", true);
    assert_eq!(missing.status.code(), Some(2), "NAME is required");
}

#[test]
fn ext_list_names_candidates_without_executing_them() {
    let fx = Fixture::new();
    let foo = fx.ext("foo", 0);
    let report = fx.ext("report", 0);
    let plain = fx.bin.join("corral-ext-noexec");
    std::fs::write(&plain, "#!/bin/sh\n").expect("write");
    std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o644)).expect("chmod");

    let out = fx.run(&["ext", "list"], "", true);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    let foo_line = stdout
        .lines()
        .find(|line| line.starts_with("foo "))
        .expect("foo listed");
    assert!(foo_line.contains(&foo.display().to_string()), "{stdout}");
    let report_line = stdout
        .lines()
        .find(|line| line.starts_with("report "))
        .expect("report listed");
    assert!(report_line.contains("corral ext run report"), "{stdout}");
    assert!(
        !stdout.contains("noexec"),
        "non-executable files are not candidates"
    );

    let json = fx.run(&["ext", "list", "--json"], "", true);
    assert!(json.status.success());
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&json.stdout).expect("json");
    let row = |path: &Path| {
        rows.iter()
            .find(|r| r["path"] == path.to_str().expect("utf8"))
            .cloned()
            .expect("row listed")
    };
    assert_eq!(row(&foo)["name"], "foo");
    assert_eq!(row(&foo)["reserved"], false);
    assert_eq!(row(&report)["reserved"], true);

    assert_eq!(fx.argv("foo"), None, "listing never executes a candidate");
    assert_eq!(
        fx.argv("report"),
        None,
        "listing never executes a candidate"
    );
}

#[test]
fn missing_and_reserved_names_give_clap_unknown_subcommand_error() {
    let fx = Fixture::new();
    let missing = fx.run(&["nosuch-ext-zz", "a"], "", true);
    assert_eq!(missing.status.code(), Some(2));
    assert!(
        text(&missing.stderr).contains("unrecognized subcommand 'nosuch-ext-zz'"),
        "{}",
        text(&missing.stderr)
    );
    assert!(missing.stdout.is_empty());

    let ordinary = fx.run(&["report", "a"], "", false);
    fx.ext("report", 0);
    let reserved = fx.run(&["report", "a"], "", true);
    assert_eq!(reserved.status.code(), Some(2));
    assert_eq!(
        text(&reserved.stderr),
        text(&ordinary.stderr),
        "a reserved name never falls through to its extension"
    );
    assert!(text(&reserved.stderr).contains("unrecognized subcommand 'report'"));
    assert_eq!(fx.argv("report"), None);

    let ext_run = fx.run(&["ext", "run", "nosuch-ext-zz"], "", true);
    assert_eq!(ext_run.status.code(), Some(1));
    assert!(text(&ext_run.stderr).contains("no corral-ext-nosuch-ext-zz executable on PATH"));
}
