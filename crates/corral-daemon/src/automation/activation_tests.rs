use super::*;
use corral_engine::events::NewEvent;
use corral_types::Actor;

const ENABLED: &str = r#"
[automation.agents.triage]
profile = "pi"
instructions = "Summarize what is blocking the task."

[automation.rules.on-blocked]
agent = "triage"
event = "blocked"
enabled = true
"#;

fn digest(text: &str) -> [u8; 32] {
    Sha256::digest(text.as_bytes()).into()
}

#[test]
fn unrelated_save_preserves_pending_blocks_without_a_live_manifest() {
    for previously_app_saved in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("test.db")).unwrap();
        let lock = fence(dir.path()).unwrap();
        let path = dir.path().join("corral.toml");
        let worker_path = lock.dir.join("on-blocked.json");
        let config = Config::parse(ENABLED).unwrap();
        let now = Timestamp::from_second(60).unwrap();
        let later = Timestamp::from_second(120).unwrap();
        let baseline = if previously_app_saved {
            stage(&lock, digest(""), digest(ENABLED), &config, 0, now).unwrap();
            load(&lock, digest(ENABLED)).unwrap()
        } else {
            None
        };
        pass(
            &lock,
            &store,
            &config,
            baseline.as_ref(),
            || now,
            &mut |_, _, _, _, _| panic!("unexpected block before baseline"),
        )
        .unwrap();
        let observed = record(&worker_path).unwrap().unwrap();
        assert_eq!(observed.generation, u64::from(previously_app_saved));

        let current = format!("# hand edit\n{ENABLED}");
        fs::write(&path, &current).unwrap();
        assert!(load(&lock, digest(&current)).unwrap().is_none());
        let pending = store
            .transaction(|store| {
                store.append_event_at(
                    &NewEvent {
                        task: None,
                        run: None,
                        kind: EventKind::Blocked,
                        actor: Actor::Agent,
                        detail: json!({"message": "waiting"}),
                    },
                    later,
                )
            })
            .unwrap()
            .seq;
        let candidate = format!("# app save\n{current}");
        stage(
            &lock,
            digest(&current),
            digest(&candidate),
            &config,
            pending,
            later,
        )
        .unwrap();
        fs::write(&path, &candidate).unwrap();
        let manifest = load(&lock, digest(&candidate)).unwrap().unwrap();
        let carried = &manifest.rules["on-blocked"];
        assert_eq!(carried.generation, observed.generation);
        assert_eq!(carried.cursor, observed.cursor);
        assert_eq!(carried.minute, observed.minute);
        let mut sent = Vec::new();
        pass(
            &lock,
            &store,
            &config,
            Some(&manifest),
            || later,
            &mut |_, _, _, _, text| {
                sent.push(
                    serde_json::from_str::<serde_json::Value>(text).unwrap()["seq"]
                        .as_u64()
                        .unwrap(),
                );
                Delivery {
                    outcome: "sent".into(),
                    error: None,
                    chat: None,
                }
            },
        )
        .unwrap();
        assert_eq!(sent, [pending]);

        // A hand disable not yet observed by the worker must still make the
        // next app enable establish a new baseline rather than reuse its record.
        let disabled = ENABLED.replace("enabled = true", "enabled = false");
        fs::write(&path, &disabled).unwrap();
        stage(
            &lock,
            digest(&disabled),
            digest(ENABLED),
            &config,
            pending + 1,
            later,
        )
        .unwrap();
        fs::write(&path, ENABLED).unwrap();
        let enabled = load(&lock, digest(ENABLED)).unwrap().unwrap();
        assert!(enabled.rules["on-blocked"].generation > observed.generation);
        assert_eq!(enabled.rules["on-blocked"].cursor, pending + 1);
    }
}
