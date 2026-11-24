use super::*;

#[test]
fn commands_render_documented_flags_and_escape_shell_arguments() {
    let cases = [
        (
            Next::run(Op::LensDraft {
                project: "site".into(),
                kind: "state".into(),
                about: "What happens?".into(),
                ready: true,
            }),
            "corral lens draft site --kind state --about 'What happens?' --ready",
        ),
        (
            Next::run(Op::LensRepair {
                project: "site".into(),
                id: "my-lens".into(),
                ready: false,
            }),
            "corral lens repair site my-lens",
        ),
        (
            Next::force(Op::Dispatch {
                id: "T-042".into(),
                profile: Some("fast profile".into()),
                base: Some("-topic".into()),
            }),
            "corral dispatch T-042 --profile 'fast profile' --base=-topic --force",
        ),
        (
            Next::run(Op::New {
                task: NewTask {
                    title: "--literal title".into(),
                    status: Status::Ready,
                    project: Some("a project".into()),
                    priority: Priority::P1,
                    kind: Some("ui".into()),
                    size: Some("xl".into()),
                    base: Some("release/1".into()),
                    parent: Some("T-004".into()),
                    depends_on: vec!["T-001".into(), "T-002".into()],
                    ..NewTask::default()
                },
            }),
            "corral new -p 'a project' --ready --priority P1 --kind ui --size xl --base release/1 --parent T-004 --depends-on T-001,T-002 -- '--literal title'",
        ),
        (
            Next::force(Op::Approve {
                id: "T-042".into(),
                onto: Some("release/2".into()),
            }),
            "corral approve T-042 --onto release/2 --force",
        ),
        (
            Next::run(Op::Edit {
                id: "T-042".into(),
                edit: TaskEdit {
                    base: Some(None),
                    ..TaskEdit::default()
                },
            }),
            "corral edit T-042 --base ''",
        ),
        (
            Next::run(Op::Verdict {
                id: "T-042".into(),
                approve: false,
                notes: Some("it's red; $(echo retry)".into()),
            }),
            r"corral verdict T-042 changes --notes 'it'\''s red; $(echo retry)'",
        ),
        (
            Next::run(Op::Reply {
                id: "T-042".into(),
                text: "--help".into(),
            }),
            "corral reply -- T-042 --help",
        ),
        (
            Next::run(Op::Retry {
                id: "T-042".into(),
                profile: None,
                fresh: true,
                note: Some(String::new()),
            }),
            "corral retry T-042 --fresh --note ''",
        ),
        (
            Next::run(Op::ArtifactAdd {
                id: "T-042".into(),
                path: "reports/diff report.html".into(),
                kind: Some("diff".into()),
                title: Some("review notes".into()),
            }),
            "corral artifact add T-042 'reports/diff report.html' --kind diff --title 'review notes'",
        ),
        (
            Next::run(Op::UsageAdd {
                usage: Usage {
                    task: Some("T-042".into()),
                    run: Some("R1".into()),
                    at: jiff::Timestamp::UNIX_EPOCH,
                    profile: None,
                    harness: None,
                    model: "model".into(),
                    input: 12,
                    output: 0,
                    cache_read: 0,
                    cache_write: 2,
                    explicit_zero_tokens: [UsageToken::Output].into(),
                    usd: Some(0.25),
                    billing: Some("api".into()),
                },
            }),
            "corral usage add --model model --task T-042 --run R1 --input 12 --output 0 --cache-write 2 --usd 0.25 --billing api",
        ),
    ];

    for (next, command) in cases {
        assert_eq!(next.command(), command);
    }
}

#[test]
fn command_can_use_the_invoked_program_or_path() {
    let next = Next::run(Op::Sync);
    assert_eq!(next.command(), "corral sync");
    assert_eq!(next.command_with_program("corral-next"), "corral-next sync");
    assert_eq!(
        next.command_with_program("/path with spaces/corral-next"),
        "'/path with spaces/corral-next' sync"
    );
}

#[test]
fn edit_round_trip_distinguishes_unchanged_cleared_and_replaced_fields() {
    for selection in [None, Some(None), Some(Some("value".to_owned()))] {
        let op = Op::Edit {
            id: "T-042".into(),
            edit: TaskEdit {
                project: selection.clone(),
                kind: selection.clone(),
                size: selection.clone(),
                profile: selection.clone(),
                base: selection.clone(),
                parent: selection.clone(),
                ..TaskEdit::default()
            },
        };
        let wire = serde_json::to_value(&op).unwrap();
        let edit = &wire["edit"]["edit"];
        for key in ["project", "kind", "size", "profile", "base", "parent"] {
            match &selection {
                None => assert!(edit.get(key).is_none()),
                Some(value) => assert_eq!(edit.get(key), Some(&serde_json::json!(value))),
            }
        }
        assert_eq!(serde_json::from_value::<Op>(wire).unwrap(), op);
    }
}
