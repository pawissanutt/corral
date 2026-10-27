use corral_core::{Store, records::RecordArea};
use corral_ops::{Ctx, PmObservation, pm_card, pm_present, pm_resolve, pm_workset};
use serde_json::{Value, json};

#[test]
fn decision_presentation_and_answer_survive_restart_without_legacy_authority() {
    // Given a persisted workspace choice, alongside a conflicting legacy answer.
    let temp = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(temp.path()).unwrap());
    let mut action = pm_card(
        &PmObservation::Owner {
            task: "workspace".into(),
            reason: "Choose the workspace scope".into(),
        },
        "corral",
    );
    action.task = None;
    let decision = json!({
        "id":"decision-fixture", "task":"workspace", "run":null,
        "source":"workspace", "request_digest":"fixture", "action":action,
        "presentation":null
    });
    let area = RecordArea::<Value>::new("pm-decisions");
    area.save(
        &ctx.store,
        "state",
        &json!({
            "records":{"decision-fixture":{"decision":decision,"answer":null}}
        }),
    )
    .unwrap();
    assert!(
        !ctx.store
            .dir()
            .join("state")
            .join("pm-decisions.json")
            .exists()
    );
    let legacy = ctx.store.dir().join("state").join("pm-decisions.json");
    let obsolete = json!({
        "version":1,
        "records":{"decision-fixture":{"decision":decision,"answer":"obsolete answer"}}
    });
    std::fs::write(&legacy, serde_json::to_vec(&obsolete).unwrap()).unwrap();
    let at = "2026-10-18T12:00:00Z".parse().unwrap();
    // When the owner presents the choice and a restarted process resolves it.
    pm_present(&ctx, "decision-fixture", "owner:receipt", true, at).unwrap();
    let restarted = Ctx::from_store(Store::open_unobserved(temp.path()).unwrap());
    let workset = pm_workset(&restarted).unwrap();
    let pending = workset
        .decisions
        .iter()
        .find(|d| d.id == "decision-fixture")
        .unwrap();
    let presentation = pending.presentation.as_ref().unwrap();
    assert_eq!(presentation.delivery, "owner:receipt");
    assert!(presentation.confirmed);
    assert_eq!(presentation.at, at);
    pm_resolve(&restarted, "decision-fixture", "retain scope").unwrap();
    // Then another restart reads the same answer and excludes the resolved choice.
    let fresh = Ctx::from_store(Store::open_unobserved(temp.path()).unwrap());
    let row = fresh
        .store
        .record_row("pm-decisions", "state")
        .unwrap()
        .unwrap();
    assert_eq!(row.doc["doc_version"], 1);
    let saved = area.load(&fresh.store, "state").unwrap().unwrap();
    assert_eq!(
        saved["records"]["decision-fixture"]["answer"],
        "retain scope"
    );
    assert!(
        !pm_workset(&fresh)
            .unwrap()
            .decisions
            .iter()
            .any(|d| d.id == "decision-fixture")
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&legacy).unwrap()).unwrap(),
        obsolete
    );
}

#[test]
fn legacy_decisions_do_not_supply_missing_canonical_state() {
    // Given only an invalid legacy decision file.
    let temp = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(temp.path()).unwrap());
    std::fs::write(
        ctx.store.dir().join("state").join("pm-decisions.json"),
        "invalid legacy",
    )
    .unwrap();
    // When the PM workset is read after restart.
    let fresh = Ctx::from_store(Store::open_unobserved(temp.path()).unwrap());
    let workset = pm_workset(&fresh).unwrap();
    // Then legacy state neither causes a decoding failure nor contributes decisions.
    assert!(workset.decisions.is_empty());
    assert!(
        fresh
            .store
            .record_row("pm-decisions", "state")
            .unwrap()
            .is_none()
    );
}
