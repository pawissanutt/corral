use std::path::{Path, PathBuf};

use jiff::Timestamp;

use super::*;
use crate::agent::{AgentBinding, AgentRole, AgentRunState, HostHealth};
use crate::assistant::{NewOwnerInput, OWNER_INPUTS_MAX, OwnerInputSource};

fn home() -> Home {
    Home {
        lexical: Some("/home/u".into()),
        canonical: Some("/data/users/u".into()),
    }
}

fn at(time: &str) -> Timestamp {
    format!("2026-10-30T{time}:00Z").parse().unwrap()
}

fn container() -> AssistantContainer {
    AssistantContainer {
        version: 1,
        pane: None,
        herdr_socket: None,
        workspace: PathBuf::from("/w"),
        state: AgentRunState::new(AgentBinding {
            role: AgentRole::Assistant,
            store: "st".into(),
            project: String::new(),
            generation: 1,
            health: HostHealth::Unbound,
            callback: None,
            closing: false,
        }),
        owner_inputs: Vec::new(),
        write_grants: Vec::new(),
        write_grants_issued: 0,
        permission_audit: Vec::new(),
        workflow: None,
    }
}

fn pair(path: &str) -> PermissionPath {
    PermissionPath {
        shown: path.into(),
        canonical: path.into(),
    }
}

fn decision(choice: PermissionChoice, scope: Option<&str>, n: u32) -> NewPermission {
    NewPermission {
        generation: 1,
        turn: Some(3),
        request: format!("rq-{n:032x}"),
        tool: WriteTool::Write,
        operation: WriteOperation {
            kind: OperationKind::NewFile,
            replacements: None,
        },
        target: pair("/srv/proj/a.txt"),
        digest: "d".repeat(64),
        invocation: Invocation {
            generation: 1,
            id: format!("{n:032x}"),
            tool_call: format!("call-{n}"),
        },
        choice,
        scope: scope.map(pair),
    }
}

fn same(path: &Path) -> Option<PathBuf> {
    Some(path.to_path_buf())
}

fn pane_input(n: usize) -> NewOwnerInput {
    NewOwnerInput {
        generation: 1,
        turn: None,
        source: OwnerInputSource::Pane,
        sha256: "a".repeat(64),
        text: format!("pane {n}"),
        truncated: false,
        command: None,
        answers: None,
        request: format!("ev-1-{n}"),
    }
}

#[test]
fn settings_refuse_broad_or_malformed_allow_entries_and_accept_broad_deny() {
    // Given HOME /home/u and every allow entry the scope limit or the syntax refuses.
    let home = home();
    for entry in [
        "/",
        "~",
        "~/.",
        "~//",
        "/home",
        "/etc",
        "/home/u",
        "/home/u/",
        "relative/dir",
        "~/a/../b",
        "~/skills/*",
    ] {
        let settings = WritePermissionSettings {
            allow: vec![entry.into()],
            deny: Vec::new(),
        };
        // Then each is refused, naming the field and the entry.
        let error = settings.validate(&home).unwrap_err();
        assert!(
            error.starts_with(&format!("write_permissions.allow entry {entry:?} ")),
            "{entry}: {error}"
        );
    }
    let many = WritePermissionSettings {
        allow: (0..65).map(|i| format!("/srv/p{i}")).collect(),
        deny: Vec::new(),
    };
    assert_eq!(
        many.validate(&home).unwrap_err(),
        "write_permissions.allow lists more than 64 entries"
    );
    let dotted = WritePermissionSettings {
        allow: Vec::new(),
        deny: vec!["~/a/../b".into()],
    };
    assert_eq!(
        dotted.validate(&home).unwrap_err(),
        "write_permissions.deny entry \"~/a/../b\" must not contain '..'"
    );
    // And broad deny entries and narrow allow entries load.
    let fine = WritePermissionSettings {
        allow: vec!["~/.agents/skills".into(), "/home/u/proj".into()],
        deny: vec!["/".into(), "~".into()],
    };
    fine.validate(&home).unwrap();
}

#[test]
fn the_catalog_settings_function_records_both_lists_and_refuses_bad_tables() {
    let table =
        |value: serde_json::Value| -> CapabilitySettings { serde_json::from_value(value).unwrap() };
    let effective = write_permission_settings(&table(json!({"deny": ["/"]}))).unwrap();
    assert_eq!(
        serde_json::to_value(&effective).unwrap(),
        json!({"allow": [], "deny": ["/"]})
    );
    assert!(write_permission_settings(&table(json!({"allow": ["/"]}))).is_err());
    assert!(write_permission_settings(&table(json!({"ask": []}))).is_err());
}

#[test]
fn the_scope_limit_refuses_root_top_level_and_home_ancestors_in_either_form() {
    // The case table the host's TypeScript mirror repeats (paths.ts).
    let home = home();
    let cases: [(&str, Option<&str>); 12] = [
        ("/", Some("is the root directory")),
        ("/etc", Some("is a top-level directory")),
        ("/home", Some("is a top-level directory")),
        (
            "/home/u",
            Some("is the home directory or an ancestor of it"),
        ),
        (
            "/home/u/",
            Some("is the home directory or an ancestor of it"),
        ),
        (
            "/data/users",
            Some("is the home directory or an ancestor of it"),
        ),
        (
            "/data/users/u",
            Some("is the home directory or an ancestor of it"),
        ),
        ("/home/u/proj", None),
        ("/data/users/u/proj", None),
        ("/opt/lib", None),
        ("relative/dir", Some("is not an absolute path")),
        ("/srv/../home", Some("must not contain '..'")),
    ];
    for (path, expected) in cases {
        assert_eq!(
            scope_limit(Path::new(path), &home).err().as_deref(),
            expected,
            "{path}"
        );
    }
    assert_eq!(
        expand("~//", &home).unwrap(),
        PathBuf::from("/home/u"),
        "repeated and trailing separators go"
    );
    assert_eq!(
        protected_component(Path::new("/p/.worktrees/T-9/x")),
        Some(".worktrees")
    );
    assert_eq!(protected_component(Path::new("/p/.gitignore")), None);
}

#[test]
fn an_hour_grant_survives_a_backward_clock_jump_but_expires_at_its_deadline() {
    // Given an hour grant on /srv/proj made at 10:00.
    let mut c = container();
    let (input, grant) = c
        .record_permission(
            "A-1",
            decision(PermissionChoice::Hour, Some("/srv/proj"), 1),
            at("10:00"),
        )
        .unwrap();
    let grant = grant.unwrap();
    assert_eq!(grant.expires_at, Some(at("11:00")));
    assert_eq!(input.permission.unwrap().expires_at, Some(at("11:00")));
    let target = Path::new("/srv/proj/sub/b.txt");
    // A backward jump remains active; the deadline still expires the grant.
    for (time, allowed, state) in [
        ("10:59", true, GrantState::Active),
        ("11:00", false, GrantState::Expired),
        ("09:59", true, GrantState::Active),
    ] {
        let found = check(&c.write_grants, target, at(time), &home(), same);
        assert_eq!(found.is_some(), allowed, "{time}");
        assert_eq!(c.write_grants[0].state(at(time), false), state, "{time}");
    }
    assert!(
        c.revoke_write_grants("A-1", Some("G-1"), "owner", at("11:00"), false)
            .unwrap_err()
            .contains("G-1 is expired")
    );
    assert_eq!(
        c.revoke_write_grants("A-1", Some("G-1"), "owner", at("09:59"), false)
            .unwrap(),
        ["G-1"]
    );
    assert!(
        check(
            &c.write_grants,
            Path::new("/srv/other"),
            at("10:30"),
            &home(),
            same
        )
        .is_none()
    );
}

#[test]
fn check_skips_once_revoked_and_relinked_grants() {
    let mut c = container();
    let now = at("10:00");
    let (_, once) = c
        .record_permission("A-1", decision(PermissionChoice::Once, None, 1), now)
        .unwrap();
    let once = once.unwrap();
    assert_eq!(
        (once.consumed_at, once.scope.clone()),
        (Some(now), pair("/srv/proj/a.txt"))
    );
    let target = Path::new("/srv/proj/a.txt");
    assert!(
        check(&c.write_grants, target, now, &home(), same).is_none(),
        "once is spent"
    );
    c.record_permission(
        "A-1",
        decision(PermissionChoice::Session, Some("/srv/proj"), 2),
        now,
    )
    .unwrap();
    assert_eq!(
        check(&c.write_grants, target, now, &home(), same)
            .unwrap()
            .id,
        "G-2"
    );
    // A shown scope that now resolves elsewhere no longer matches.
    let relinked = |_: &Path| Some(PathBuf::from("/srv/elsewhere"));
    assert!(check(&c.write_grants, target, now, &home(), relinked).is_none());
    // Revoking an inactive grant is refused; revoking the active one queues its audit line.
    assert!(
        c.revoke_write_grants("A-1", Some("G-1"), "owner", now, false)
            .unwrap_err()
            .contains("G-1 is consumed")
    );
    assert_eq!(
        c.revoke_write_grants("A-1", None, "owner", now, false)
            .unwrap(),
        ["G-2"]
    );
    assert!(check(&c.write_grants, target, now, &home(), same).is_none());
    let audit = c.permission_audit.last().unwrap();
    assert_eq!(audit.key, "G-2:revoked");
    assert_eq!(audit.detail["by"], "owner");
    assert_eq!(c.write_grants[1].state(now, true), GrantState::Revoked);
}

#[test]
fn a_full_store_refuses_an_allow_and_evicts_only_inactive_records_with_fresh_ids() {
    // Given 64 active session grants.
    let mut c = container();
    let now = at("10:00");
    for n in 0..64 {
        c.record_permission(
            "A-1",
            decision(PermissionChoice::Session, Some("/srv/proj"), n),
            now,
        )
        .unwrap();
    }
    let before = c.clone();
    // Then another hour or once decision is refused with nothing changed, while deny still
    // journals because it makes no grant.
    for choice in [PermissionChoice::Hour, PermissionChoice::Once] {
        let scope = (choice == PermissionChoice::Hour).then_some("/srv/proj");
        assert_eq!(
            c.record_permission("A-1", decision(choice, scope, 99), now)
                .unwrap_err(),
            GrantsFull
        );
        assert_eq!(c, before);
    }
    c.record_permission("A-1", decision(PermissionChoice::Deny, None, 98), now)
        .unwrap();
    assert_eq!(c.write_grants, before.write_grants);
    // When one record is inactive, it is evicted and the new grant gets a never-used ID.
    c.revoke_write_grants("A-1", Some("G-1"), "owner", now, false)
        .unwrap();
    let (_, grant) = c
        .record_permission(
            "A-1",
            decision(PermissionChoice::Hour, Some("/srv/proj"), 100),
            now,
        )
        .unwrap();
    assert_eq!(grant.unwrap().id, "G-65");
    assert_eq!(c.write_grants.len(), WRITE_GRANTS_MAX);
    assert!(c.write_grants.iter().all(|g| g.id != "G-1"));
    // And the evicted grant's decision stays queued for the event log.
    assert!(
        c.permission_audit
            .iter()
            .any(|a| a.detail["decision"]["grant"] == "G-1")
    );
}

#[test]
fn a_permission_entry_drops_the_oldest_permission_entry_never_a_pane_input() {
    // Given a full journal of 32 pane inputs interleaved with 32 permission entries.
    let mut c = container();
    let now = at("10:00");
    for n in 0..32 {
        c.journal_owner_input("A-1", pane_input(n));
        c.record_permission("A-1", decision(PermissionChoice::Deny, None, n as u32), now)
            .unwrap();
    }
    assert_eq!(c.owner_inputs.len(), OWNER_INPUTS_MAX);
    let oldest = c.owner_inputs[1].id.clone();
    // When a 33rd permission entry is journaled.
    let (entry, _) = c
        .record_permission("A-1", decision(PermissionChoice::Deny, None, 32), now)
        .unwrap();
    // Then the oldest permission entry goes and every pane input stays.
    let count = |c: &AssistantContainer, source| {
        c.owner_inputs.iter().filter(|e| e.source == source).count()
    };
    assert_eq!(count(&c, OwnerInputSource::Pane), 32);
    assert_eq!(count(&c, OwnerInputSource::Permission), 32);
    assert!(c.owner_input(&oldest).is_none());
    assert_eq!(entry.id, "A-1/g1/i65");
    // And a pane input still evicts oldest-first, as before.
    c.journal_owner_input("A-1", pane_input(99));
    assert_eq!(c.owner_inputs.len(), OWNER_INPUTS_MAX);
    assert_eq!(count(&c, OwnerInputSource::Pane), 32);
}

#[test]
fn the_first_permission_in_a_full_pane_journal_preserves_every_pane_input() {
    for choice in [
        PermissionChoice::Once,
        PermissionChoice::Hour,
        PermissionChoice::Session,
        PermissionChoice::Deny,
    ] {
        // Given a full journal with no permission entry available for eviction.
        let mut c = container();
        for n in 0..OWNER_INPUTS_MAX {
            c.journal_owner_input("A-1", pane_input(n));
        }
        let pane_inputs = c.owner_inputs.clone();
        let scope = matches!(choice, PermissionChoice::Hour | PermissionChoice::Session)
            .then_some("/srv/proj");
        // When the owner makes the first permission decision.
        let (entry, _) = c
            .record_permission("A-1", decision(choice, scope, 1), at("10:00"))
            .unwrap();
        // Then every pane input and the new decision remain, with its durable audit record.
        assert_eq!(
            &c.owner_inputs[..OWNER_INPUTS_MAX],
            pane_inputs,
            "{choice:?}"
        );
        assert_eq!(c.owner_inputs.last(), Some(&entry));
        assert_eq!(
            c.permission_audit[0].detail["decision"],
            serde_json::to_value(entry.permission.unwrap()).unwrap()
        );
    }
}

#[test]
fn a_permission_overflow_stays_bounded_and_ordinary_eviction_stays_oldest_first() {
    for source in [
        OwnerInputSource::Pane,
        OwnerInputSource::Dialog,
        OwnerInputSource::Confirmation,
    ] {
        // Given a full citable journal followed by a permission decision.
        let mut c = container();
        for n in 0..OWNER_INPUTS_MAX {
            c.journal_owner_input("A-1", pane_input(n));
        }
        c.record_permission(
            "A-1",
            decision(PermissionChoice::Deny, None, 1),
            at("10:00"),
        )
        .unwrap();
        let first_permission = c.owner_inputs.last().unwrap().id.clone();
        // When another permission arrives, it replaces only the previous permission.
        let (entry, _) = c
            .record_permission(
                "A-1",
                decision(PermissionChoice::Deny, None, 2),
                at("10:00"),
            )
            .unwrap();
        assert_eq!(c.owner_inputs.len(), OWNER_INPUTS_MAX + 1);
        assert_eq!(c.owner_inputs[0].id, "A-1/g1/i1");
        assert!(c.owner_input(&first_permission).is_none());
        // Ordinary inputs still trim oldest-first to the ordinary journal limit.
        let mut input = pane_input(99);
        input.source = source;
        let newest = c.journal_owner_input("A-1", input);
        assert_eq!(c.owner_inputs.len(), OWNER_INPUTS_MAX);
        assert_eq!(c.owner_inputs[0].id, "A-1/g1/i3");
        assert_eq!(c.owner_input(&entry.id), Some(&entry));
        assert_eq!(c.owner_inputs.last(), Some(&newest));
    }
}

#[test]
fn a_deny_records_no_scope_lifetime_or_grant_and_the_summary_is_bounded() {
    let mut c = container();
    let mut long = decision(PermissionChoice::Deny, None, 1);
    long.target = pair(&format!("/srv/{}", "x".repeat(2000)));
    let (entry, grant) = c.record_permission("A-1", long, at("10:00")).unwrap();
    assert!(grant.is_none());
    let record = entry.permission.clone().unwrap();
    assert_eq!(record.choice, PermissionChoice::Deny);
    assert!(record.scope.is_none() && record.lifetime.is_none() && record.grant.is_none());
    assert!(entry.truncated && entry.text.len() <= PERMISSION_TEXT_MAX);
    assert_eq!(entry.source, OwnerInputSource::Permission);
    // An entry and a container written before this change still parse.
    let mut old = serde_json::to_value(&entry).unwrap();
    old.as_object_mut().unwrap().remove("permission");
    let parsed: crate::assistant::OwnerInput = serde_json::from_value(old).unwrap();
    assert!(parsed.permission.is_none());
    let bare = serde_json::to_value(container()).unwrap();
    assert!(bare.get("write_grants").is_none() && bare.get("permission_audit").is_none());
    assert_eq!(
        serde_json::from_value::<AssistantContainer>(bare).unwrap(),
        container()
    );
}

#[test]
fn an_allow_entry_resolving_to_a_broad_directory_is_ignored() {
    let home = home();
    let link = |target: &'static str| move |_: &Path| Some(PathBuf::from(target));
    assert_eq!(
        resolve_allow("~/link", &home, link("/home/u/real")).unwrap(),
        PathBuf::from("/home/u/real")
    );
    assert_eq!(
        resolve_allow("~/link", &home, link("/")).unwrap_err(),
        "resolves to /, which is the root directory"
    );
    assert_eq!(
        resolve_allow("~/gone", &home, |_: &Path| None).unwrap_err(),
        "does not resolve to an existing directory"
    );
    assert!(resolve_allow("~", &home, same).is_err());
}
