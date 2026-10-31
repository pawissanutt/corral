use std::path::{Path, PathBuf};

use corral_core::Store;
use corral_core::agent::write_permissions::{
    GrantState, Invocation, NewPermission, OperationKind, PermissionChoice, PermissionPath,
    WriteOperation, WriteTool,
};
use corral_core::agent::{ContainerRef, RequestReceipt, paths};
use corral_core::assistant::AssistantContainer;
use jiff::Timestamp;
use serde_json::{Value, json};

use super::{CLOCK, HOME};
use crate::Ctx;
use crate::agent::admission::{BindTarget, fence_generation};
use crate::agent::call::CallResponse;
use crate::agent::receipts::FAILPOINT;
use crate::agent::test_support::{self as ts, Fx, Host};
use crate::agent::{event_request_id, tool_request_id};
use crate::assistant::permissions;

const OP: &str = "write_permission";

struct S {
    fx: Fx,
    host: Host,
    id: String,
    home: PathBuf,
    proj: PathBuf,
}

fn bind(fx: &Fx, id: &str, pane: &str, session: &str) -> Host {
    let target = BindTarget::Assistant { session: id.into() };
    let host = ts::bind(fx, &target, pane, session);
    ts::committed(&ts::hello(fx, &host));
    host
}

/// A-1 bound in generation 1, with a private home and project directory under the store's
/// temporary root, and this thread's HOME pointed at that home.
fn session() -> S {
    let fx = ts::fx();
    let root = std::fs::canonicalize(fx._tmp.path()).unwrap();
    let (home, proj) = (root.join("home/u"), root.join("work/proj"));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(proj.join("src")).unwrap();
    HOME.with_borrow_mut(|slot| *slot = Some(home.clone()));
    let id = fx
        .ctx
        .store
        .create_assistant("pi", None, Some("a-pane"), None)
        .unwrap()
        .id;
    let host = bind(&fx, &id, "a-pane", "s-a1");
    S {
        fx,
        host,
        id,
        home,
        proj,
    }
}

fn at(time: &str) -> Timestamp {
    format!("2026-10-30T{time}:00Z").parse().unwrap()
}

fn pair(path: &Path) -> Value {
    json!({"shown": path, "canonical": path})
}

fn request(host: &Host, generation: u32, n: u32) -> String {
    tool_request_id(
        &host.identity.session_id,
        &format!("wp:{generation}:{n:032x}"),
    )
}

fn payload(generation: u32, n: u32, choice: &str, target: &Path, scope: Option<Value>) -> Value {
    let mut payload = json!({
        "action": "decide",
        "invocation": {"generation": generation, "id": format!("{n:032x}"), "tool_call": "call-1"},
        "tool": "write", "operation": {"kind": "new_file"}, "target": pair(target),
        "digest": "d".repeat(64), "preview_sha256": "e".repeat(64), "choice": choice,
    });
    if let Some(scope) = scope {
        payload["scope"] = scope;
    }
    payload
}

fn input(host: &Host, n: u32, choice: &str, target: &Path, scope: Option<&Path>) -> Vec<u8> {
    let generation = host.identity.generation;
    let payload = payload(generation, n, choice, target, scope.map(pair));
    ts::envelope(host, OP, &request(host, generation, n), payload, false)
}

fn send(fx: &Fx, host: &Host, request: &str, payload: Value) -> CallResponse {
    ts::send(fx, &ts::envelope(host, OP, request, payload, false), OP)
}

fn decide(s: &S, n: u32, choice: &str, target: &Path, scope: Option<&Path>) -> RequestReceipt {
    ts::committed(&ts::send(
        &s.fx,
        &input(&s.host, n, choice, target, scope),
        OP,
    ))
}

fn check(fx: &Fx, host: &Host, target: &Path) -> CallResponse {
    let request = tool_request_id(&host.identity.session_id, "check");
    let payload = json!({"action": "check", "tool": "write", "target": pair(target)});
    send(fx, host, &request, payload)
}

/// The grant a check found, `None` when it found none, or the refusal code.
fn granted(fx: &Fx, host: &Host, target: &Path) -> Result<Option<String>, &'static str> {
    let response = check(fx, host, target);
    match ts::code(&response) {
        Some(code) => Err(code),
        None => Ok(response.receipt().unwrap().result["grant"]
            .as_str()
            .map(str::to_owned)),
    }
}

fn container(s: &S) -> AssistantContainer {
    s.fx.ctx.store.get_assistant_container(&s.id).unwrap()
}

fn lines(store: &Store, kind: &str, key: &str) -> Vec<Value> {
    store
        .events()
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == kind && event.detail["key"] == key)
        .map(|event| event.detail)
        .collect()
}

fn fence(s: &S, generation: u32) {
    let reference = ContainerRef::Assistant {
        session: s.id.clone(),
    };
    assert!(fence_generation(&s.fx.ctx.store, &reference, generation, "resume").unwrap());
}

#[test]
fn a_new_skill_scope_resolves_without_creation_and_keeps_symlink_and_directory_checks() {
    let s = session();
    let scope = s.home.join(".agents/skills/example");
    let target = scope.join("SKILL.md");
    assert!(!scope.exists());
    let receipt = decide(&s, 1, "hour", &target, Some(&scope));
    assert_eq!(receipt.result["authorized"], true);
    assert!(
        !scope.exists(),
        "authorization must not create the directory"
    );
    assert_eq!(
        granted(&s.fx, &s.host, &target).unwrap(),
        Some("G-1".into())
    );
    std::fs::create_dir_all(&scope).unwrap();
    std::fs::write(&target, "# Example\n").unwrap();
    assert_eq!(
        granted(&s.fx, &s.host, &target).unwrap(),
        Some("G-1".into())
    );
    assert_eq!(
        granted(&s.fx, &s.host, &scope.join(".git/config")),
        Err("fixed_rule")
    );
    // The nearest existing ancestor must be a directory; dangling symlinks stay refused.
    assert_eq!(permissions::resolve_dir(&target.join("child")), None);
    let dangling = s.home.join("dangling");
    std::os::unix::fs::symlink(s.home.join("absent"), &dangling).unwrap();
    assert_eq!(permissions::resolve_dir(&dangling.join("child")), None);
    // A symlinked ancestor resolves to its actual destination, including absent descendants.
    let link = s.home.join("linked-skills");
    std::os::unix::fs::symlink(&scope, &link).unwrap();
    assert_eq!(
        permissions::resolve_dir(&link.join("new/deep")),
        Some(scope.join("new/deep"))
    );
    // Replacing the granted root with a link does not widen the recorded grant.
    std::fs::remove_file(&target).unwrap();
    std::fs::remove_dir(&scope).unwrap();
    std::os::unix::fs::symlink(&s.proj, &scope).unwrap();
    assert_eq!(granted(&s.fx, &s.host, &target).unwrap(), None);
}

#[test]
fn a_once_decision_replays_only_to_its_live_generation_and_is_consumed() {
    // Given A-1 generation 1 decided Allow once.
    let s = session();
    let target = s.proj.join("a.txt");
    let original = input(&s.host, 1, "once", &target, None);
    let first = ts::committed(&ts::send(&s.fx, &original, OP));
    assert_eq!(first.result["authorized"], true);
    assert_eq!(first.result["lifetime"], "once");
    // When the live generation resends it, then the first receipt replays and nothing is added.
    match ts::send(&s.fx, &original, OP) {
        CallResponse::Receipt {
            receipt,
            replayed: true,
        } => assert_eq!(receipt, first),
        other => panic!("expected a replay, got {other:?}"),
    }
    assert_eq!(container(&s).owner_inputs.len(), 1);
    // When a resume fenced generation 1, then its resend is refused with no result.
    fence(&s, 1);
    let fenced = ts::send(&s.fx, &original, OP);
    assert_eq!(ts::code(&fenced), Some("agent_fenced"));
    assert!(fenced.receipt().is_none());
    // And generation 2 can neither send generation 1's payload nor an underived request ID.
    let second = bind(&s.fx, &s.id, "a-pane", "s-a2");
    let cross = send(
        &s.fx,
        &second,
        &request(&second, 1, 1),
        payload(1, 1, "once", &target, None),
    );
    assert_eq!(ts::code(&cross), Some("bad_request"));
    let underived = tool_request_id(&second.identity.session_id, "call-1");
    let refused = send(
        &s.fx,
        &second,
        &underived,
        payload(2, 2, "once", &target, None),
    );
    assert_eq!(ts::code(&refused), Some("bad_request"));
    // Then the once record is consumed and no check finds a grant.
    let c = container(&s);
    assert_eq!(c.owner_inputs.len(), 1);
    assert_eq!(c.write_grants.len(), 1);
    assert_eq!(
        c.write_grants[0].state(Timestamp::now(), false),
        GrantState::Consumed
    );
    assert_eq!(granted(&s.fx, &second, &target), Ok(None));
}

#[derive(Clone, Copy, Debug)]
enum After {
    Fence,
    End,
    Nothing,
}

/// A duplicate of A-1's first once decide, held after its New admission while a scoped thread
/// with no hook commits the identical original and then does `after`.
fn held_duplicate(after: After) -> (S, String, CallResponse) {
    let s = session();
    let target = s.proj.join("a.txt");
    let original = input(&s.host, 1, "once", &target, None);
    let (store, id, sent) = (s.fx.ctx.store.clone(), s.id.clone(), original.clone());
    let mut armed = true;
    FAILPOINT.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |point: &str| {
            if point != "write_permission_admitted_before_commit" || !std::mem::take(&mut armed) {
                return;
            }
            let (store, id, sent) = (store.clone(), id.clone(), sent.clone());
            std::thread::scope(|scope| {
                scope
                    .spawn(move || {
                        let ctx = Ctx::from_store(store.clone());
                        ts::committed(&crate::agent::call(&ctx, OP, &sent).unwrap());
                        let reference = ContainerRef::Assistant {
                            session: id.clone(),
                        };
                        match after {
                            After::Fence => {
                                fence_generation(&store, &reference, 1, "resume").unwrap();
                            }
                            After::End => {
                                store.end_assistant(&id).unwrap();
                            }
                            After::Nothing => {}
                        }
                    })
                    .join()
                    .unwrap();
            });
        }));
    });
    let duplicate = ts::send(&s.fx, &original, OP);
    FAILPOINT.with(|hook| hook.borrow_mut().take());
    let request = request(&s.host, 1, 1);
    (s, request, duplicate)
}

#[test]
fn a_duplicate_held_before_commit_gets_no_authorization_after_a_fence_or_an_end() {
    for (after, code) in [(After::Fence, "agent_fenced"), (After::End, "run_ended")] {
        let (s, request, duplicate) = held_duplicate(after);
        // Then the duplicate is refused with no receipt, and only the original is recorded.
        assert_eq!(ts::code(&duplicate), Some(code), "{after:?}");
        assert!(duplicate.receipt().is_none());
        let c = container(&s);
        let stored = c.state.receipts.iter().filter(|r| r.request_id == request);
        assert_eq!(stored.count(), 1, "{after:?}");
        assert_eq!(c.owner_inputs.len(), 1);
        assert_eq!(c.write_grants.len(), 1);
        assert!(c.write_grants[0].consumed_at.is_some());
    }
}

#[test]
fn a_duplicate_held_before_commit_replays_the_original_while_the_generation_is_live() {
    let (s, request, duplicate) = held_duplicate(After::Nothing);
    match duplicate {
        CallResponse::Receipt {
            receipt,
            replayed: true,
        } => {
            assert_eq!(receipt.request_id, request);
            assert_eq!(receipt.result["authorized"], true);
        }
        other => panic!("expected the original receipt, got {other:?}"),
    }
    assert_eq!(container(&s).owner_inputs.len(), 1);
}

#[test]
fn an_admission_replay_after_the_session_ended_is_refused() {
    let s = session();
    let original = input(&s.host, 1, "once", &s.proj.join("a.txt"), None);
    ts::committed(&ts::send(&s.fx, &original, OP));
    s.fx.ctx.store.end_assistant(&s.id).unwrap();
    let refused = ts::send(&s.fx, &original, OP);
    assert_eq!(ts::code(&refused), Some("run_ended"));
    assert!(refused.receipt().is_none());
}

/// Runs decide `run` (or, with `None`, a drain of A-1's outbox) on a new thread whose
/// failpoint hook panics at `point`; true when it crashed.
fn crashed_at(s: &S, point: &'static str, run: Option<Vec<u8>>) -> bool {
    let (store, home, session) = (s.fx.ctx.store.clone(), s.home.clone(), s.id.clone());
    std::thread::spawn(move || {
        HOME.with_borrow_mut(|slot| *slot = Some(home));
        FAILPOINT.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |at: &str| {
                if at == point {
                    panic!("crash at {point}");
                }
            }));
        });
        match run {
            Some(input) => {
                crate::agent::call(&Ctx::from_store(store), OP, &input).unwrap();
            }
            None => {
                permissions::drain_permission_audit(&store, &session).unwrap();
            }
        }
    })
    .join()
    .is_err()
}

#[derive(Clone, Copy, Debug)]
enum Drainer {
    List,
    Decide,
    Revoke,
    End,
}

#[test]
fn a_decision_committed_before_its_audit_line_is_logged_by_the_next_drain() {
    for drainer in [
        Drainer::List,
        Drainer::Decide,
        Drainer::Revoke,
        Drainer::End,
    ] {
        // Given a decide that crashed after its commit, before its audit drain.
        let s = session();
        let store = &s.fx.ctx.store;
        let decided = input(&s.host, 1, "hour", &s.proj.join("a.txt"), Some(&s.proj));
        let key = request(&s.host, 1, 1);
        assert!(crashed_at(
            &s,
            "write_permission_committed_before_audit",
            Some(decided)
        ));
        // Then the entry is queued and the event log has no line for it.
        let queued = container(&s).permission_audit;
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].key, key);
        assert!(lines(store, "assistant_write_permission", &key).is_empty());
        // When the next list, decide, revoke or end runs.
        match drainer {
            Drainer::List => {
                fence(&s, 1);
                let second = bind(&s.fx, &s.id, "a-pane", "s-a2");
                let request = tool_request_id(&second.identity.session_id, "list");
                let listed = send(&s.fx, &second, &request, json!({"action": "list"}));
                assert_eq!(listed.receipt().unwrap().result["grants"][0]["id"], "G-1");
            }
            Drainer::Decide => {
                decide(&s, 2, "deny", &s.proj.join("b.txt"), None);
            }
            Drainer::Revoke => {
                permissions::revoke(store, &s.id, Some("G-1"), "owner").unwrap();
            }
            Drainer::End => {
                crate::agent::ops::assistant::end(&s.fx.ctx, &s.id).unwrap();
            }
        }
        // Then one line carries the full record under the request ID, and the entry is cleared.
        let logged = lines(store, "assistant_write_permission", &key);
        assert_eq!(logged.len(), 1, "{drainer:?}");
        let record = container(&s).owner_inputs[0].permission.clone().unwrap();
        assert_eq!(
            logged[0]["decision"],
            serde_json::to_value(&record).unwrap()
        );
        assert_eq!(logged[0]["input"], format!("{}/g1/i1", s.id));
        assert!(
            container(&s)
                .permission_audit
                .iter()
                .all(|entry| entry.key != key)
        );
    }
}

#[test]
fn a_drain_crashed_between_log_and_clear_repeats_the_line_with_the_same_key() {
    let s = session();
    let store = &s.fx.ctx.store;
    let decided = input(&s.host, 1, "deny", &s.proj.join("a.txt"), None);
    let key = request(&s.host, 1, 1);
    assert!(crashed_at(
        &s,
        "write_permission_committed_before_audit",
        Some(decided)
    ));
    assert!(crashed_at(&s, "permission_audit_logged_before_clear", None));
    assert_eq!(container(&s).permission_audit.len(), 1);
    assert_eq!(lines(store, "assistant_write_permission", &key).len(), 1);
    assert_eq!(
        permissions::drain_permission_audit(store, &s.id).unwrap(),
        1
    );
    let logged = lines(store, "assistant_write_permission", &key);
    assert_eq!(logged.len(), 2);
    assert_eq!(logged[0], logged[1]);
    assert!(container(&s).permission_audit.is_empty());
}

#[test]
fn an_hour_grant_allows_at_10_59_and_not_at_11_00_or_09_59() {
    let s = session();
    CLOCK.set(Some(at("10:00")));
    let made = decide(&s, 1, "hour", &s.proj.join("a.txt"), Some(&s.proj));
    assert_eq!(made.result["grant"], "G-1");
    assert_eq!(made.result["expires_at"], "2026-10-30T11:00:00Z");
    let target = s.proj.join("src/lib.rs");
    for (time, expected) in [
        ("10:59", Some("G-1".to_owned())),
        ("11:00", None),
        ("09:59", None),
    ] {
        CLOCK.set(Some(at(time)));
        assert_eq!(granted(&s.fx, &s.host, &target), Ok(expected), "{time}");
    }
}

#[test]
fn a_session_grant_serves_only_its_session_live_generation() {
    // Given A-1 generation 1's session grant on the project.
    let s = session();
    decide(&s, 1, "session", &s.proj.join("a.txt"), Some(&s.proj));
    let target = s.proj.join("src/lib.rs");
    // Then A-2 sees no grant.
    let other =
        s.fx.ctx
            .store
            .create_assistant("pi", None, Some("b-pane"), None)
            .unwrap();
    let a2 = bind(&s.fx, &other.id, "b-pane", "s-b1");
    assert_eq!(granted(&s.fx, &a2, &target), Ok(None));
    // When a resume fences generation 1, then it is refused while generation 2 is allowed.
    fence(&s, 1);
    let second = bind(&s.fx, &s.id, "a-pane", "s-a2");
    assert_eq!(granted(&s.fx, &s.host, &target), Err("agent_fenced"));
    assert_eq!(granted(&s.fx, &second, &target), Ok(Some("G-1".into())));
    // When the session ends, then no check is answered and the listing shows the grant ended.
    crate::agent::ops::assistant::end(&s.fx.ctx, &s.id).unwrap();
    assert_eq!(granted(&s.fx, &second, &target), Err("run_ended"));
    let home = corral_core::agent::write_permissions::Home::new(&s.home);
    let listing = permissions::list(&s.fx.ctx.store, &s.id, Timestamp::now(), &home).unwrap();
    assert_eq!(listing.grants[0].state, GrantState::Ended);
}

#[test]
fn an_owner_revocation_ends_the_grant_and_another_assistant_cannot_revoke() {
    let s = session();
    decide(&s, 1, "hour", &s.proj.join("a.txt"), Some(&s.proj));
    let target = s.proj.join("b.txt");
    let store = &s.fx.ctx.store;
    let before = container(&s);
    assert!(permissions::revoke(store, &s.id, Some("G-1"), "A-2").is_err());
    assert_eq!(container(&s), before, "a foreign revoke changes nothing");
    let revoked = permissions::revoke(store, &s.id, Some("G-1"), "owner").unwrap();
    assert_eq!(
        (revoked.revoked, revoked.warning),
        (vec!["G-1".to_owned()], None)
    );
    assert_eq!(granted(&s.fx, &s.host, &target), Ok(None));
    let grant = container(&s).write_grants[0].clone();
    assert!(grant.revoked_at.is_some());
    assert_eq!(grant.revoked_by.as_deref(), Some("owner"));
    let line = lines(store, "assistant_write_permission_revoked", "G-1:revoked");
    assert_eq!(line.len(), 1);
    assert_eq!(
        (&line[0]["grant"], &line[0]["by"]),
        (&json!("G-1"), &json!("owner"))
    );
}

#[test]
fn broad_mismatched_and_protected_scopes_are_refused_and_a_relinked_scope_stops_matching() {
    let s = session();
    let in_home = s.home.join("x");
    let refusals = [
        ("/home/u/x", Some(PathBuf::from("/home")), "bad_request"),
        (
            in_home.to_str().unwrap(),
            Some(s.home.clone()),
            "bad_request",
        ),
        (".git/config", None, "fixed_rule"),
        (".corral/corral.toml", None, "fixed_rule"),
        (".worktrees/T-9/x", None, "fixed_rule"),
    ];
    for (n, (target, scope, code)) in (1..).zip(refusals) {
        let target = s.proj.join(target);
        let choice = if scope.is_some() { "hour" } else { "once" };
        let refused = ts::send(
            &s.fx,
            &input(&s.host, n, choice, &target, scope.as_deref()),
            OP,
        );
        assert_eq!(ts::code(&refused), Some(code), "{}", target.display());
    }
    // A canonical scope that differs from corral's own resolution of the shown scope.
    let mismatch = json!({"shown": s.proj, "canonical": s.proj.join("src")});
    let refused = send(
        &s.fx,
        &s.host,
        &request(&s.host, 1, 9),
        payload(1, 9, "hour", &s.proj.join("src/a.rs"), Some(mismatch)),
    );
    assert_eq!(ts::code(&refused), Some("bad_request"));
    assert!(container(&s).owner_inputs.is_empty(), "nothing journaled");
    // Given a session grant whose shown scope is a symlink to the project.
    let link = s.proj.parent().unwrap().join("link");
    std::os::unix::fs::symlink(&s.proj, &link).unwrap();
    let scope = json!({"shown": link, "canonical": s.proj});
    let made = send(
        &s.fx,
        &s.host,
        &request(&s.host, 1, 10),
        payload(1, 10, "session", &s.proj.join("a.txt"), Some(scope)),
    );
    assert_eq!(ts::committed(&made).result["grant"], "G-1");
    let target = s.proj.join("src/lib.rs");
    assert_eq!(granted(&s.fx, &s.host, &target), Ok(Some("G-1".into())));
    // When the link is pointed at another directory, then the grant no longer matches.
    let other = s.proj.parent().unwrap().join("other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&other, &link).unwrap();
    assert_eq!(granted(&s.fx, &s.host, &target), Ok(None));
}

#[test]
fn hour_and_deny_decisions_journal_structured_records_that_their_events_repeat() {
    let s = session();
    CLOCK.set(Some(at("10:00")));
    decide(&s, 1, "hour", &s.proj.join("a.txt"), Some(&s.proj));
    decide(&s, 2, "deny", &s.proj.join("b.txt"), None);
    let c = container(&s);
    let hour = c.owner_inputs[0].permission.clone().unwrap();
    let canonical = s.proj.to_str().unwrap();
    assert_eq!(
        serde_json::to_value(&hour).unwrap(),
        json!({
            "tool": "write", "operation": {"kind": "new_file"},
            "target": pair(&s.proj.join("a.txt")), "digest": "d".repeat(64),
            "invocation": {"generation": 1, "id": format!("{:032x}", 1), "tool_call": "call-1"},
            "choice": "hour", "lifetime": "hour",
            "scope": {"shown": canonical, "canonical": canonical},
            "granted_at": "2026-10-30T10:00:00Z", "expires_at": "2026-10-30T11:00:00Z",
            "grant": "G-1",
        })
    );
    let deny = c.owner_inputs[1].permission.clone().unwrap();
    assert_eq!(deny.choice, PermissionChoice::Deny);
    assert!(deny.lifetime.is_none() && deny.scope.is_none() && deny.grant.is_none());
    for (n, record) in [(1, &hour), (2, &deny)] {
        let logged = lines(
            &s.fx.ctx.store,
            "assistant_write_permission",
            &request(&s.host, 1, n),
        );
        assert_eq!(logged.len(), 1);
        assert_eq!(logged[0]["decision"], serde_json::to_value(record).unwrap());
        assert_eq!(
            (&logged[0]["session"], &logged[0]["generation"]),
            (&json!(s.id), &json!(1))
        );
    }
}

#[test]
fn the_event_path_refuses_a_permission_source() {
    let s = session();
    let payload = json!({"kind": "owner_input", "source": "permission", "text": "y",
        "sha256": paths::sha256_hex(b"y")});
    let input = ts::envelope(&s.host, "event", &event_request_id(1, 1), payload, false);
    assert_eq!(
        ts::code(&ts::send(&s.fx, &input, "event")),
        Some("bad_request")
    );
    assert!(container(&s).owner_inputs.is_empty());
}

fn seeded(n: u32, choice: PermissionChoice, scope: &Path) -> NewPermission {
    let scope = scope.to_str().unwrap();
    NewPermission {
        generation: 1,
        turn: None,
        request: format!("rq-{n:032x}"),
        tool: WriteTool::Write,
        operation: WriteOperation {
            kind: OperationKind::NewFile,
            replacements: None,
        },
        target: PermissionPath {
            shown: format!("{scope}/a.txt"),
            canonical: format!("{scope}/a.txt"),
        },
        digest: "d".repeat(64),
        invocation: Invocation {
            generation: 1,
            id: format!("{n:032x}"),
            tool_call: "call".into(),
        },
        choice,
        scope: Some(PermissionPath {
            shown: scope.into(),
            canonical: scope.into(),
        }),
    }
}

#[test]
fn sixty_four_active_grants_refuse_a_new_one_and_an_expired_record_makes_room() {
    // Given 64 active grant records, the first an hour grant made at 08:00.
    let s = session();
    let store = &s.fx.ctx.store;
    CLOCK.set(Some(at("08:30")));
    store
        .update_assistant_container(&s.id, |c| {
            c.record_permission(
                "A-1",
                seeded(100, PermissionChoice::Hour, &s.proj),
                at("08:00"),
            )
            .unwrap();
            for n in 101..164 {
                c.record_permission(
                    "A-1",
                    seeded(n, PermissionChoice::Session, &s.proj),
                    at("08:00"),
                )
                .unwrap();
            }
            Ok(())
        })
        .unwrap();
    let before = container(&s);
    // When another session or hour decide arrives, then it is refused and nothing changes.
    for (n, choice) in [(1, "session"), (2, "hour")] {
        let refused = ts::send(
            &s.fx,
            &input(&s.host, n, choice, &s.proj.join("a.txt"), Some(&s.proj)),
            OP,
        );
        assert_eq!(ts::code(&refused), Some("grants_full"));
        let after = container(&s);
        assert_eq!(
            (after.owner_inputs, after.write_grants),
            (before.owner_inputs.clone(), before.write_grants.clone())
        );
    }
    // When G-1 has expired, then a new grant evicts it and gets a never-used ID.
    CLOCK.set(Some(at("10:00")));
    let made = decide(&s, 3, "hour", &s.proj.join("a.txt"), Some(&s.proj));
    assert_eq!(made.result["grant"], "G-65");
    let grants = container(&s).write_grants;
    assert_eq!(grants.len(), 64);
    assert!(grants.iter().all(|grant| grant.id != "G-1"));
    // And the evicted grant's decision is still in the event log.
    let evicted = lines(
        store,
        "assistant_write_permission",
        &format!("rq-{:032x}", 100),
    );
    assert_eq!(evicted[0]["decision"]["grant"], "G-1");
}
