use clap::Parser;

use super::*;

#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    pi: PiArgs,
}

fn parse(argv: &[&str]) -> Result<PiArgs, clap::Error> {
    Cli::try_parse_from(std::iter::once("pi").chain(argv.iter().copied())).map(|cli| cli.pi)
}

fn build(here: bool, name: Option<&str>, resume: Option<&str>) -> Invocation {
    Invocation::Build {
        here,
        name: name.map(Into::into),
        resume: resume.map(Into::into),
    }
}

#[test]
fn the_invocation_split_follows_flags_workspace_and_terminal() {
    use Invocation::*;
    // (argv, in a workspace, on a terminal, what opens or the refusal code)
    type Case<'a> = (&'a [&'a str], bool, bool, Result<Invocation, &'a str>);
    let cases: &[Case] = &[
        (&[], true, true, Ok(Launcher)),
        (
            &["--model", "m", "--effort", "high"],
            true,
            true,
            Ok(Launcher),
        ),
        (&[], true, false, Ok(Assistant { over_cap: false })),
        (
            &["--assistant"],
            true,
            true,
            Ok(Assistant { over_cap: false }),
        ),
        (
            &["--assistant"],
            true,
            false,
            Ok(Assistant { over_cap: false }),
        ),
        (
            &["--assistant", "--over-cap"],
            true,
            true,
            Ok(Assistant { over_cap: true }),
        ),
        (
            &["--over-cap"],
            true,
            true,
            Ok(Assistant { over_cap: true }),
        ),
        (
            &["--over-cap"],
            true,
            false,
            Ok(Assistant { over_cap: true }),
        ),
        (&["--pm"], true, true, Ok(Pm)),
        (&["--pm", "-c"], true, true, Ok(Pm)),
        (&["--pm", "--continue"], true, false, Ok(Pm)),
        (
            &["--task", "T-42"],
            true,
            true,
            Ok(Assistant { over_cap: false }),
        ),
        (&["--task", "T-42"], false, true, Err("not_in_workspace")),
        (&["--resume", "A-1"], true, true, Ok(Resume("A-1".into()))),
        (&["--resume", "A-3"], true, false, Ok(Resume("A-3".into()))),
        (&["--build"], true, true, Ok(build(false, None, None))),
        (&["--build"], true, false, Ok(build(false, None, None))),
        (
            &["--build", "--here"],
            true,
            true,
            Ok(build(true, None, None)),
        ),
        (
            &["--build", "--name", "x"],
            true,
            true,
            Ok(build(false, Some("x"), None)),
        ),
        (
            &["--build", "--model", "m"],
            true,
            true,
            Ok(build(false, None, None)),
        ),
        (&["--build"], false, true, Ok(build(false, None, None))),
        (
            &["--build", "--here", "--name", "x"],
            false,
            false,
            Ok(build(true, Some("x"), None)),
        ),
        (
            &["--resume", "B-abc123"],
            true,
            true,
            Ok(build(false, None, Some("B-abc123"))),
        ),
        (
            &["--resume", "B-abc123"],
            false,
            true,
            Ok(build(false, None, Some("B-abc123"))),
        ),
        (
            &["--build", "--resume", "B-abc123"],
            true,
            true,
            Ok(build(false, None, Some("B-abc123"))),
        ),
        (
            &["--build", "--resume", "A-3"],
            true,
            true,
            Err("usage_error"),
        ),
        (&["--bare"], true, true, Ok(Bare { notice: false })),
        (&["--bare"], false, false, Ok(Bare { notice: false })),
        (
            &["--bare", "--here", "--name", "local"],
            true,
            true,
            Ok(Bare { notice: false }),
        ),
        (&[], false, true, Ok(Bare { notice: true })),
        (&[], false, false, Ok(Bare { notice: true })),
        (&["--model", "m"], false, true, Ok(Bare { notice: true })),
        (&["--pm"], false, true, Err("not_in_workspace")),
        (&["--assistant"], false, true, Err("not_in_workspace")),
        (&["--resume", "A-1"], false, true, Err("not_in_workspace")),
        (&["--over-cap"], false, true, Err("not_in_workspace")),
    ];
    for (argv, in_workspace, terminal, expected) in cases {
        let args = parse(argv).unwrap();
        let got = invocation(&args, *in_workspace, *terminal);
        match expected {
            Ok(expected) => assert_eq!(&got.unwrap(), expected, "{argv:?} {terminal}"),
            Err(code) => {
                let error = got.unwrap_err().to_string();
                assert!(error.contains(code), "{argv:?}: {error}");
            }
        }
    }
}

#[test]
fn launcher_bare_selection_dispatches_without_opening_a_bound_session() {
    let (next, _) = chosen(BARE_ROW, 0).unwrap();
    assert_eq!(launcher_dispatch(next), LauncherDispatch::Bare);
}

#[test]
fn launcher_rows_open_what_their_flags_open() {
    // Build's first option is a new worktree (`--build`), its second the checkout (`--here`).
    let options = build_entry().options;
    assert_eq!(options.len(), 2);
    assert!(options[BUILD_HERE].starts_with("here"));
    assert_eq!(
        chosen(BUILD_ROW, 0).unwrap(),
        (build(false, None, None), false)
    );
    assert_eq!(
        chosen(BUILD_ROW, BUILD_HERE).unwrap(),
        (build(true, None, None), false)
    );
    assert_eq!(
        chosen(ASSISTANT_ROW, 0).unwrap(),
        (Invocation::Assistant { over_cap: false }, false)
    );
    // The PM row says it continues the previous conversation: `--pm -c`.
    assert_eq!(chosen(PM_ROW, 0).unwrap(), (Invocation::Pm, true));
    assert_eq!(
        chosen("A-7", 0).unwrap(),
        (Invocation::Resume("A-7".into()), false)
    );
    // A build Resume row is `--resume B-<id>`.
    let session = BuildSessionSummary {
        id: "B-abc123".into(),
        name: "checkout-copy".into(),
        repo_root: "/work/mitiplay".into(),
        worktree: "/work/mitiplay/.worktrees/build-checkout-copy".into(),
        here: false,
        bare: false,
        project: None,
        updated: "2026-11-12T10:00:00Z".into(),
        exited: Some("ship".into()),
    };
    let now: jiff::Timestamp = "2026-11-12T10:23:00Z".parse().unwrap();
    let row = build_resume_entry(&session, now);
    assert_eq!(row.name, "build");
    assert_eq!(row.detail, "checkout-copy · mitiplay · 23m ago · shipped");
    assert_eq!(
        chosen(&row.id, 0).unwrap(),
        (build(false, None, Some("B-abc123")), false)
    );
}

#[test]
fn launcher_offers_bare_and_marks_resumable_bare_sessions() {
    // Bare is a start row directly after Build and opens `--bare`.
    let rows = start_entries();
    assert_eq!(rows[0].id, BUILD_ROW);
    assert_eq!(rows[1].id, BARE_ROW);
    assert_eq!(rows[1].name, "Bare");
    assert!(matches!(rows[1].section, Section::Start));
    assert_eq!(rows.len(), 4);
    assert_eq!(
        chosen(BARE_ROW, 0).unwrap(),
        (Invocation::Bare { notice: false }, false)
    );
    // A recorded bare session's row says bare and resumes by id; a build one still says build.
    let session = |bare| BuildSessionSummary {
        id: "B-bare01".into(),
        name: "scratch".into(),
        repo_root: "/work/mitiplay".into(),
        worktree: "/work/mitiplay/.worktrees/build-scratch".into(),
        here: false,
        bare,
        project: None,
        updated: "2026-11-12T10:00:00Z".into(),
        exited: None,
    };
    let now: jiff::Timestamp = "2026-11-12T10:23:00Z".parse().unwrap();
    let bare = build_resume_entry(&session(true), now);
    assert_eq!(bare.name, "bare");
    assert!(matches!(bare.section, Section::Resume));
    assert_eq!(
        chosen(&bare.id, 0).unwrap(),
        (build(false, None, Some("B-bare01")), false)
    );
    assert_eq!(build_resume_entry(&session(false), now).name, "build");
}

#[test]
fn conflicting_modes_are_usage_errors() {
    for argv in [
        &["--bare", "--pm"][..],
        &["--bare", "--task", "T-42"],
        &["--pm", "--task", "T-42"],
        &["--resume", "A-1", "--task", "T-42"],
        &["--bare", "--resume", "A-1"],
        &["--pm", "--resume", "A-1"],
        &["--pm", "--over-cap"],
        &["--resume", "A-1", "--over-cap"],
        &["--assistant", "--bare"],
        &["--assistant", "--pm"],
        &["--assistant", "--resume", "A-1"],
        &["-c"],
        &["--bare", "-c"],
        &["--resume", "A-1", "-c"],
        &["--over-cap", "-c"],
        &["--task", "T-42", "-c"],
        &["--build", "--bare"],
        &["--build", "--pm"],
        &["--build", "--assistant"],
        &["--build", "--task", "T-42"],
        &["--build", "--over-cap"],
        &["--build", "-c"],
        &["--build", "--pm", "-c"],
        &["--here"],
        &["--name", "x"],
        &["--resume", "B-abc123", "--here"],
        &["--resume", "B-abc123", "--name", "x"],
        &["--build", "--here", "--resume", "B-abc123"],
        &["--build", "--name", "x", "--resume", "B-abc123"],
    ] {
        assert!(parse(argv).is_err(), "{argv:?}");
    }
}

#[test]
fn pm_continuation_is_explicit() {
    assert!(!parse(&["--pm"]).unwrap().continue_session);
    for flag in ["-c", "--continue"] {
        assert!(parse(&["--pm", flag]).unwrap().continue_session);
    }
}
