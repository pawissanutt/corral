use super::*;
use corral_engine::{
    config::automation::{AgentSpec, Rule},
    events::NewEvent,
};
use corral_types::Actor;

struct Fixture {
    dir: tempfile::TempDir,
    store: Store,
    config: Config,
}

impl Fixture {
    fn new(cron: Option<&str>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("test.db")).unwrap();
        let mut config = Config::default();
        config.automation.agents.insert(
            "agent".into(),
            AgentSpec {
                profile: "pi".into(),
                instructions: "private policy".into(),
            },
        );
        config.automation.rules.insert(
            "rule".into(),
            Rule {
                enabled: true,
                agent: "agent".into(),
                event: cron.is_none().then(|| "blocked".into()),
                cron: cron.map(str::to_owned),
            },
        );
        Self { dir, store, config }
    }

    fn tick(
        &self,
        minute: i64,
        mut send: impl FnMut(&str, &str, Option<&str>, &str, &str) -> Delivery,
    ) -> bool {
        pass(
            self.dir.path(),
            &self.store,
            &self.config,
            Timestamp::from_second(minute * 60).unwrap(),
            &mut send,
        )
        .unwrap()
    }

    fn record(&self) -> Record {
        read(&self.dir.path().join("automation/rule.json"))
            .unwrap()
            .unwrap()
    }

    fn event(&self, kind: EventKind) -> u64 {
        self.store
            .transaction(|store| {
                store.append_event_at(
                    &NewEvent {
                        task: None,
                        run: None,
                        kind,
                        actor: Actor::Agent,
                        detail: json!({"message": "🦀\\\"".repeat(1024)}),
                    },
                    Timestamp::from_second(0).unwrap(),
                )
            })
            .unwrap()
            .seq
    }
}

fn forbidden(_: &str, _: &str, _: Option<&str>, _: &str, _: &str) -> Delivery {
    panic!("unexpected send")
}

#[test]
fn claim_precedes_delivery_and_restart_never_retries() {
    let f = Fixture::new(None);
    f.event(EventKind::Blocked);
    assert!(!f.tick(0, forbidden));
    let seq = f.event(EventKind::Blocked);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        f.tick(1, |rule, profile, prior, _, text| {
            assert_eq!((rule, profile, prior), ("rule", "pi", None));
            assert_eq!(f.record().claim, Some(format!("event:{seq}")));
            assert_eq!(f.record().cursor, seq);
            assert!(!text.contains("private policy"));
            assert!(text.len() <= 2048);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(text).unwrap()["seq"],
                seq
            );
            std::panic::panic_any(42_u8);
        })
    }));
    assert_eq!(result.unwrap_err().downcast_ref::<u8>(), Some(&42));
    assert!(!f.tick(2, forbidden));
    assert_eq!(
        health(f.dir.path(), &f.config).unwrap()["rule"]
            .outcome
            .as_deref(),
        Some("delivery unknown")
    );
}

#[test]
fn failed_attempt_and_skips_survive_rollback() {
    let f = Fixture::new(None);
    assert!(!f.tick(0, forbidden));
    f.event(EventKind::Created);
    let first = f.event(EventKind::Blocked);
    let second = f.event(EventKind::Blocked);
    assert!(f.tick(5, |_, _, _, _, _| Delivery {
        outcome: "failed".into(),
        error: Some("offline".into()),
        chat: None
    }));
    assert!(!f.tick(5, forbidden));
    let r = f.record();
    assert_eq!(r.claim, Some(format!("event:{first}")));
    assert_eq!(r.last_skip, Some(format!("event:{second}")));
    f.event(EventKind::Blocked);
    assert!(!f.tick(4, forbidden));
    let r = f.record();
    assert_eq!(r.skipped, 2);
    assert_eq!(r.health.error.as_deref(), Some("offline"));
    assert!(!f.tick(6, forbidden));
}

#[test]
fn cron_misses_downtime_and_rebaselines_on_changes() {
    let mut f = Fixture::new(Some("*/2 * * * *"));
    assert!(!f.tick(0, forbidden));
    assert!(!f.tick(3, forbidden));
    assert!(f.tick(4, |_, _, _, _, text| {
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(text).unwrap()["utc_minute"],
            4
        );
        Delivery {
            outcome: "sent".into(),
            chat: Some("1".into()),
            error: None,
        }
    }));
    assert!(!f.tick(4, forbidden));
    assert!(!f.tick(2, forbidden));
    f.config.automation.rules.get_mut("rule").unwrap().enabled = false;
    assert!(!f.tick(5, forbidden));
    assert!(!f.tick(6, forbidden));
    f.config.automation.rules.get_mut("rule").unwrap().enabled = true;
    assert!(!f.tick(8, forbidden));
    assert!(!f.tick(8, forbidden));
    f.config
        .automation
        .agents
        .get_mut("agent")
        .unwrap()
        .instructions
        .push('!');
    assert!(!f.tick(10, forbidden));
    assert_eq!(f.record().claim.as_deref(), Some("cron:4"));
    f.config.automation.rules.clear();
    assert!(f.tick(11, forbidden));
    assert!(!f.dir.path().join("automation/rule.json").exists());
}

#[test]
fn event_pages_advance_in_sequence_order() {
    let f = Fixture::new(None);
    assert!(!f.tick(0, forbidden));
    f.store
        .transaction(|store| {
            for _ in 0..128 {
                store.append_event(&NewEvent {
                    task: None,
                    run: None,
                    kind: EventKind::Created,
                    actor: Actor::Owner,
                    detail: json!(null),
                })?;
            }
            Ok(())
        })
        .unwrap();
    let blocked = f.event(EventKind::Blocked);
    assert!(!f.tick(1, forbidden));
    assert_eq!(f.record().cursor, blocked - 1);
    assert!(f.tick(1, |_, _, _, _, _| Delivery {
        outcome: "skipped".into(),
        error: Some("busy".into()),
        chat: None
    }));
    assert_eq!(f.record().health.outcome.as_deref(), Some("skipped"));
}
