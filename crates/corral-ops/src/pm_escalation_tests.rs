use super::*;
use crate::pm_next::{PmObservation, PmOption, pm_card};

fn routine() -> PmAction {
    pm_card(
        &PmObservation::AutomaticWait {
            task: "T-1".into(),
            condition: "external".into(),
        },
        "corral",
    )
}

#[test]
fn each_escalation_reason_and_routine_case() {
    use EscalationReason::*;
    let cases = [
        (None, 0, Some("wait"), false, vec![]),
        (
            Some(PmActionKind::Owner),
            0,
            Some("wait"),
            false,
            vec![OwnerOnly, UnvalidatedClass],
        ),
        (
            Some(PmActionKind::Question),
            0,
            Some("wait"),
            false,
            vec![OwnerOnly, UnvalidatedClass],
        ),
        (
            Some(PmActionKind::AgentQuestion),
            0,
            Some("wait"),
            false,
            vec![OwnerOnly, UnvalidatedClass],
        ),
        (
            Some(PmActionKind::OwnerQuestion),
            0,
            Some("wait"),
            false,
            vec![OwnerOnly, UnvalidatedClass],
        ),
        (
            Some(PmActionKind::Diagnosis),
            0,
            Some("wait"),
            false,
            vec![UnvalidatedClass],
        ),
        (None, 3, Some("wait"), false, vec![Repetition]),
        (None, 2, Some("wait"), false, vec![]),
        (None, 0, Some("wait"), true, vec![Requested]),
        (None, 0, Some("other"), false, vec![Disagreement]),
        (
            None,
            0,
            Some("escalate"),
            false,
            vec![Requested, Disagreement],
        ),
    ];
    for (kind, repeats, choice, requested_escalation, expected) in cases {
        let mut card = routine();
        if let Some(kind) = kind {
            card.kind = kind;
        }
        let actual = escalate(
            &card,
            &EscalationContext {
                repeats,
                choice,
                requested_escalation,
            },
        );
        assert_eq!(actual.required, !expected.is_empty());
        assert_eq!(actual.reasons, expected, "{:?}", card.kind);
    }
    let mut card = routine();
    card.recommended.clear();
    assert_eq!(
        escalate(&card, &EscalationContext::default()).reasons,
        vec![NoRecommendation]
    );
    for kind in [
        PmActionKind::Dispatch,
        PmActionKind::Gate,
        PmActionKind::Review,
        PmActionKind::Wait,
    ] {
        card = routine();
        card.kind = kind;
        assert!(!escalate(&card, &EscalationContext::default()).required);
    }
    card = routine();
    card.options.push(PmOption {
        id: "owner".into(),
        label: String::new(),
        command: String::new(),
        requires_owner: true,
    });
    assert!(
        !escalate(
            &card,
            &EscalationContext {
                choice: Some("wait"),
                ..Default::default()
            }
        )
        .required
    );
    assert_eq!(
        escalate(
            &card,
            &EscalationContext {
                choice: Some("owner"),
                ..Default::default()
            }
        )
        .reasons,
        vec![OwnerOnly, Disagreement]
    );
    card.options[0].requires_owner = true;
    assert_eq!(
        escalate(&card, &EscalationContext::default()).reasons,
        vec![OwnerOnly]
    );
}

#[test]
fn digest_excludes_prose_but_binds_ordered_closed_choices_and_recommendation() {
    let mut card = routine();
    let digest = shadow_digest(&card);
    assert_eq!(digest.len(), 64);
    card.why = "a new age".into();
    card.evidence.push("volatile".into());
    card.options[0].label = "new wording".into();
    card.options[0].command = "different executable path".into();
    assert_eq!(digest, shadow_digest(&card));
    card.task = Some("T-2".into());
    assert_ne!(digest, shadow_digest(&card));
    card = routine();
    card.options.push(PmOption {
        id: "other".into(),
        label: String::new(),
        command: String::new(),
        requires_owner: false,
    });
    let ordered = shadow_digest(&card);
    card.options.swap(0, 1);
    assert_ne!(ordered, shadow_digest(&card));
    let ordered = shadow_digest(&card);
    card.recommended = "other".into();
    assert_ne!(ordered, shadow_digest(&card));
    let recommended = shadow_digest(&card);
    card.kind = PmActionKind::Gate;
    assert_ne!(recommended, shadow_digest(&card));
}
