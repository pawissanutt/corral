//! Resume classification through the production notice path, with no provider calls.
use super::*;

#[test]
fn resume_fences_the_dead_generation_classifies_its_dangling_calls_and_relaunches_in_the_same_pane()
{
    // Given bun-reproof's dangling session (corral_gate and edit in flight) plus a later message
    // whose calls cover every other class, one of them with a result:
    let fx = f::fixture(false, 5);
    let mut host = HostProc::spawn(true);
    let bound = fx.bind(&fx.worker_target(), "w0:p1", &host);
    let control = f::bridge(
        &fx.ctx.store,
        &run_recipient(&fx),
        1,
        Some(f::CAP),
        f::healthy,
    );
    let calls = [
        ("c-progress", "corral_progress"),
        ("c-evidence", "corral_evidence"),
        ("c-done", "corral_done"),
        ("c-read", "read"),
        ("c-bash", "bash"),
        ("c-ls", "ls"),
        ("c-memory-put", "corral_memory_put"),
        ("c-memory-remove", "corral_memory_remove"),
    ];
    let session = f::dangling_fixture() + &f::session(&calls, &["c-ls"]);
    f::write_session(&bound, &session);
    let stored = [
        receipt(
            &bound,
            "c-progress",
            "progress",
            "committed",
            json!({"outcome": "saved"}),
        ),
        receipt(
            &bound,
            "c-evidence",
            "evidence",
            "rejected",
            json!({"code": "bad_request"}),
        ),
    ];
    let update = |t: &mut Task| {
        let state = core_agent::container_state_mut(t, &fx.worker()).unwrap();
        state.receipts.extend(stored.clone());
        Ok(())
    };
    fx.ctx.store.update(&fx.id, update).unwrap();
    std::fs::write(fx.app().join("tracked"), "changed\n").unwrap();
    let inbox = Inbox::new(&fx.ctx.store, run_recipient(&fx)).unwrap();
    let queued = inbox
        .post(corral_core::inbox::Post {
            kind: "owner_reply",
            urgency: corral_core::inbox::Urgency::Wake,
            author: "owner",
            body: "Also handle the empty input.",
            binding: MessageBinding::default(),
            subject: None,
            max_payload: corral_core::inbox::MAX_PAYLOAD,
        })
        .unwrap();
    host.kill();

    // When the owner resumes the run:
    let outcome = resume(&fx, &fx.worker(), false).unwrap();

    // Then generation 1 is fenced once, its control files are gone, and generation 2 opens in
    // the same pane with an armed launch command and nothing replayed:
    assert_eq!(
        (outcome["fenced"].as_u64(), outcome["generation"].as_u64()),
        (Some(1), Some(2))
    );
    let after = state(&fx, &fx.worker());
    let fence = after.generation(1).unwrap().fenced.clone().unwrap();
    assert_eq!(fence.reason, "resume");
    assert_eq!(
        (after.binding.generation, after.binding.health),
        (2, HostHealth::Unbound)
    );
    assert_eq!(after.generation(2).unwrap().pane, "w0:p1");
    assert_eq!(fx.events("agent_fenced").len(), 1);
    assert!(!control.dir.join("ctl.cap").exists() && !control.dir.join("host.sock").exists());
    let g2 = generation_dir(&fx.ctx.store, &run_recipient(&fx), 2).unwrap();
    assert!(g2.join("bootstrap.json").is_file());
    let runs = fx.calls("pane run");
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert!(runs[0].contains("agent launch --bootstrap") && runs[0].contains("corral-launch"));

    // And the one notice classifies each dangling call, oldest first:
    let notices = messages(&fx, &run_recipient(&fx), NOTICE_KIND);
    assert_eq!(notices.len(), 1);
    let subject = notices[0].subject.clone().unwrap();
    assert_eq!(subject["lost"], 1);
    assert_eq!(subject["generation"], 2);
    let expected = [
        ("corral_gate", "not_executed"),
        ("edit", "outcome_unknown"),
        ("corral_progress", "result"),
        ("corral_evidence", "rejected"),
        ("corral_done", "not_committed"),
        ("read", "interrupted_no_effect"),
        ("bash", "outcome_unknown"),
        ("corral_memory_put", "outcome_unknown"),
        ("corral_memory_remove", "outcome_unknown"),
    ];
    assert_eq!(
        classes(&subject["calls"]),
        expected.map(|(t, c)| (t.into(), c.into()))
    );
    let detail = |i: usize| subject["calls"][i]["detail"].as_str().unwrap_or_default();
    assert!(detail(1).contains("artifact"), "{}", detail(1));
    assert!(detail(4).contains("commit_all"), "{}", detail(4));
    assert!(
        detail(6).contains("processes it started may still run"),
        "{}",
        detail(6)
    );
    for i in [7, 8] {
        assert!(detail(i).contains("file effect unknown: re-read the entry"));
        assert!(detail(i).contains("recovery did not reapply"));
    }
    let gate_request =
        crate::agent::tool_request_id(SESSION, "call_corral_gate_1|fc_fake_1_call_corral_gate_1");
    assert_eq!(subject["calls"][0]["request_id"], json!(gate_request));

    // And the edit's evidence is a worktree artifact, and the Pi session was never rewritten:
    let task = fx.task();
    let title = format!(
        "Worktree after host loss ({} generation 1)",
        fx.worker().key()
    );
    let artifact = task.artifacts.iter().find(|a| a.title == title).unwrap();
    let text = String::from_utf8(crate::artifact_read(&fx.ctx, artifact).unwrap()).unwrap();
    assert!(text.contains("--porcelain=v2") && text.contains(".M") && text.contains("tracked"));
    assert_eq!(
        std::fs::read_to_string(f::session_file(&bound)).unwrap(),
        session
    );
    assert_eq!(fx.events("agent_resumed").len(), 1);
    let reconciled = fx.events("agent_call_reconciled");
    let reported: Vec<&str> = reconciled
        .iter()
        .filter_map(|e| e.detail["class"].as_str())
        .collect();
    assert_eq!(
        reported,
        [
            "not_executed",
            "result",
            "rejected",
            "not_committed",
            "outcome_unknown",
            "outcome_unknown"
        ]
    );

    // And the notice is published ahead of the message queued while the host was lost:
    let binding = corral_core::inbox::SessionBinding {
        session: SESSION.into(),
        endpoint: Some(g2.join("host.sock")),
        transcript: Some(f::session_file(&bound)),
    };
    let envelope = inbox
        .publish(&binding, "corral_pi", 65536, true)
        .unwrap()
        .unwrap();
    assert_eq!(envelope.members, [notices[0].seq, queued.seq]);
    let payload = inbox.payload(&envelope).unwrap();
    let notice_at = payload.find("recovery_notice").unwrap();
    assert!(notice_at < payload.find("Also handle the empty input.").unwrap());

    // And nothing was replayed: no gate ran.
    assert_eq!(gate_runs(&fx), 0);
}
