//! Setup for the private-server handoff PTY scenario (T-855 C4): a real store whose assistant
//! A-1 is planning, with r1-r3 published, a completed local review of r3 that went stale when r4
//! was published, and one scratch with a committed change. Generation 1 is a staged host that
//! never ran; the PTY scenario launches the production host as the resumed generation 2.
use super::*;
use crate::agent::test_support as ts;
use crate::agent::{event_request_id, tool_request_id};
use corral_core::Store;
use corral_core::agent::RequestIdentity;
use corral_core::assistant::{NewOwnerInput, OwnerInputSource};
use corral_core::assistant_workflow::WorkflowPhase;
use corral_core::blob_ref::BlobHash;
use serde_json::{Value, json};
use std::path::Path;

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

#[test]
#[ignore = "driven by scripts/qa-pi-handoff-live.py inside a private herdr server"]
fn handoff_live_setup() {
    let Ok(input) = std::env::var("CORRAL_HANDOFF_LIVE") else {
        return;
    };
    let input: Value = serde_json::from_str(&input).unwrap();
    assert_eq!(
        std::env::var("CORRAL_HERDR_SESSION").unwrap(),
        "corral-test"
    );
    let root = Path::new(input["root"].as_str().unwrap());
    let pane = input["pane"].as_str().unwrap();
    let mut store = Store::init(root).unwrap();
    store.config.agent.assets_dir = Some(input["assets"].as_str().unwrap().into());
    store.config.assistant.agent = Some("owner".into());
    store.config.profiles.insert(
        "owner".into(),
        serde_json::from_value(
            json!({"harness":"corral-pi", "model":"gpt-6.1-sol", "vendor":"openai"}),
        )
        .unwrap(),
    );
    store.config.projects.insert(
        "app".into(),
        serde_json::from_value(json!({"path":"app","base":"main"})).unwrap(),
    );
    store.config.resources.max_load_per_core = 1.0e6;
    store.save_config().unwrap();
    let ctx = Ctx::from_store(Store::open(root).unwrap());
    let (session, prepared, _) =
        open(&ctx, &root.join("app"), pane, &Overrides::default(), false).unwrap();
    let id = session.id.clone();
    let scratch = crate::assistant::scratch::create(&ctx, &id, "app", Some("notes"), None).unwrap();
    std::fs::write(scratch.path.join("cache.md"), "eviction notes\n").unwrap();
    git(&scratch.path, &["add", "cache.md"]);
    git(&scratch.path, &["commit", "-q", "-m", "cache notes"]);

    // Staged generation 1: the same envelopes a host sends, with a process that is not alive.
    let bootstrap = launch::consume_bootstrap(&prepared.bootstrap).unwrap();
    let host = ts::Host {
        identity: RequestIdentity {
            store: bootstrap.store.clone(),
            project: bootstrap.project.clone(),
            role: AgentRole::Assistant,
            task: None,
            run: None,
            review: None,
            callback: None,
            assistant: Some(id.clone()),
            session_id: "handoff-live-g1".into(),
            generation: bootstrap.generation,
            turn: None,
            parent: None,
        },
        host_cap: ts::secret("live-host", 1),
        ctl_cap: ts::secret("live-ctl", 1),
        nonce: bootstrap.bind_nonce.clone(),
        pane: pane.into(),
        sessions: bootstrap.session_dir.clone(),
    };
    let fx = ts::Fx {
        _tmp: tempfile::tempdir().unwrap(),
        ctx: Ctx::from_store(Store::open(root).unwrap()),
    };
    ts::committed(&ts::hello(&fx, &host));
    let generation = host.identity.generation;
    let send = |request: &str, op: &str, payload: Value| {
        let request = tool_request_id(&host.identity.session_id, request);
        ts::send(&fx, &ts::envelope(&host, op, &request, payload, false), op)
    };
    let expected = || {
        let c = ctx.store.get_assistant_container(&id).unwrap();
        let w = &c.workflow.as_ref().unwrap().current;
        json!({"workflow": w.id, "state_revision": w.state_revision,
            "authority_epoch": w.authority_epoch,
            "plan_revision": w.current_plan.as_ref().map(|p| p.revision)})
    };
    let publish = |n: &str, text: &str, extra: Value| {
        let e = expected();
        ts::committed(&send(
            &format!("stage-{n}"),
            "assistant_plan",
            json!({"action":"stage","expected":e,"offset":0,"chunk":text}),
        ));
        let mut payload = json!({"action":"publish","expected":e,"bytes":text.len(),
            "digest":BlobHash::of(text.as_bytes()),"stage":"draft"});
        for (key, value) in extra.as_object().unwrap() {
            payload[key] = value.clone();
        }
        ts::committed(&send(&format!("publish-{n}"), "assistant_plan", payload));
    };
    let config: corral_core::agent_capabilities::AgentCapabilities =
        serde_json::from_value(json!({"assistant":{"subagents":{"enabled":true}}})).unwrap();
    ctx.store
        .update_assistant_container(&id, |c| {
            let text = "Plan cache eviction; do not implement yet.";
            let entry = c.journal_owner_input(
                &id,
                NewOwnerInput {
                    generation,
                    turn: None,
                    source: OwnerInputSource::Pane,
                    sha256: BlobHash::of(text.as_bytes()).to_string(),
                    text: text.into(),
                    truncated: false,
                    command: None,
                    answers: None,
                    request: event_request_id(generation, 1),
                    binding: None,
                },
            );
            crate::assistant::workflow::prepare_entry(&ctx.store, &id, c, &entry.id)?;
            let w = &mut c.workflow.as_mut().unwrap().current;
            w.phase = WorkflowPhase::Planning;
            w.state_revision += 1;
            c.state.generations[0].capabilities = Some(config.resolve(AgentRole::Assistant, None));
            Ok(())
        })
        .unwrap();
    for n in 1..=3 {
        publish(
            &n.to_string(),
            &format!("# Cache eviction r{n}\n"),
            json!({}),
        );
    }
    let text = "Review this plan, without implementing.";
    let owner = ts::committed(&ts::send(
        &fx,
        &ts::envelope(
            &host,
            "event",
            &event_request_id(generation, 50),
            json!({"kind":"owner_input","source":"pane","text":text,
                "sha256":BlobHash::of(text.as_bytes())}),
            false,
        ),
        "event",
    ))
    .result["input"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = ts::committed(&send(
        "start-review",
        "assistant_plan_review",
        json!({"action":"start","expected":expected(),"input":owner}),
    ))
    .result;
    let event = |seq: u64, action: &str, extra: Value| {
        let mut payload = json!({"kind":"auxiliary","action":action,"usage_kind":"explore",
            "request":r["invocation"],
            "plan_review":{"workflow":r["workflow"],"review":r["review"],"digest":r["digest"]}});
        for (key, value) in extra.as_object().unwrap() {
            payload[key] = value.clone();
        }
        ts::committed(&ts::send(
            &fx,
            &ts::envelope(
                &host,
                "event",
                &event_request_id(generation, seq),
                payload,
                false,
            ),
            "event",
        ))
        .result
    };
    event(101, "review", json!({"review_event":{"action":"claim"}}));
    event(
        102,
        "prepare",
        json!({"model":"openai-codex/offline-fixture"}),
    );
    event(103, "started", json!({}));
    // r4 is ready with a declared scope, so a current-revision Implement here is reachable.
    let head = String::from_utf8(
        std::process::Command::new("git")
            .arg("-C")
            .arg(root.join("app"))
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_owned();
    publish(
        "4",
        "# Cache eviction r4\nChanged eviction order.\n",
        json!({"stage":"ready",
            "execution_scope":{"project":"app","paths":["cache.md"],"actions":["change eviction"]},
            "assumptions":{"project":"app","source_head":head,"base_head":head,
                "files":[{"path":"cache.md","sha256":null}]}}),
    );
    let findings = "No blocking findings; advisory only.\n";
    event(
        104,
        "review",
        json!({"review_event":{"action":"stage","offset":0,"chunk":findings}}),
    );
    event(105, "finish", json!({"outcome":"completed"}));
    let done = event(
        106,
        "review",
        json!({"review_event":{"action":"settle","lifecycle":"completed",
            "summary":"Advisory only","bytes":findings.len(),
            "digest":BlobHash::of(findings.as_bytes())}}),
    );
    assert_eq!(
        (
            &done["revision"],
            &done["current_revision"],
            &done["freshness"]
        ),
        (&json!(3), &json!(4), &json!("stale"))
    );

    // The PTY host is the resumed generation 2 in the same private pane.
    let (fenced, prepared) = resume(&ctx, &id, pane, &Overrides::default()).unwrap();
    assert_eq!(fenced, 1);
    let mut bootstrap = launch::consume_bootstrap(&prepared.bootstrap).unwrap();
    bootstrap.corral = input["corral"].as_str().unwrap().into();
    let dir = prepared.bootstrap.parent().unwrap();
    let ready = launch::check(&bootstrap, dir, prepared.assets.as_deref()).unwrap();
    let secrets = launch::Secrets::generate().unwrap();
    launch::write_ctl_cap(dir, &secrets.ctl_cap).unwrap();
    let message = root.join("handoff-launch.json");
    std::fs::write(
        &message,
        launch::launch_message(&bootstrap, &ready, &secrets).unwrap(),
    )
    .unwrap();
    println!(
        "HANDOFF_LIVE {}",
        json!({"session": id, "launch": message, "scratch": scratch.path,
            "generation": bootstrap.generation, "asset_sha256": bootstrap.asset_sha256})
    );
}
