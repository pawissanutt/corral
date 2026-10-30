use super::*;
use crate::attention::handling_tests::fixture::{Fx, at};
use crate::owner_notice::{CRASH, CrashPoint};

#[test]
fn actual_provider_reset_wait_retains_source_outcomes_without_resending() {
    for point in [
        None,
        Some(CrashPoint::AfterClaim),
        Some(CrashPoint::BeforeNotify),
        Some(CrashPoint::AfterNotify),
        Some(CrashPoint::AfterOutcomeSave),
        Some(CrashPoint::AfterAppend),
        Some(CrashPoint::AfterSourceClear),
    ] {
        let mut fx = Fx::new();
        fx.ctx.store.config.profiles.insert("fixture".into(), serde_json::from_value(json!({
            "harness":"omo","model":"fixture/model",
            "metadata":{
                "display_name":"Fixture","purpose":"Fixture","instructions":"Fixture",
                "guidance":corral_core::SelectionGuidance::default(),
                "eligibility":{"enabled":true,"automatic":true,"roles":["implementation_review"],"projects":[]},
                "revision":"fixture-r1","evidence":"fixture",
                "candidates":[{"id":"single","harness":"omo","connection":"fixture","provider":"fixture",
                    "model":"fixture/model","args":[],"evidence":"fixture","cost":corral_core::CandidateCost::Subscription}],
                "fallback":{"automatic":true,"approved_providers":["fixture"],"max_switches":1,
                    "allow_unknown_subscription_cost":true,"approval":"fixture"}
            }
        })).unwrap());
        let task = fx.questions(0);
        let mut run = task.current_run().unwrap().clone();
        run.harness = Some("omo".into());
        run.model = Some("fixture/model".into());
        let failed = Failed {
            task: &task.id,
            run: &run,
            role: ProfileRole::ImplementationReview,
            episode: "fixture-wait",
            exclude_provider: None,
            class: corral_core::AvailabilityClass::QuotaExhausted {
                account: false,
                reset_at: Some(at(10000)),
            },
            message: "quota exhausted",
            since: at(0),
            retries: 0,
        };
        CRASH.with(|crash| crash.set(point));
        let result = assess(&fx.ctx, &failed, at(0));
        assert_eq!(result.is_err(), point.is_some(), "{point:?}");
        if let Ok(assessment) = result {
            assert!(matches!(assessment, Assessment::Wait {until} if until == at(10000)));
        }
        let calls = fx.calls();
        assert!(matches!(
            assess(&fx.ctx, &failed, at(1)).unwrap(),
            Assessment::Wait { .. }
        ));
        crate::owner_notice::drain(&fx.reopen()).unwrap();
        assert_eq!(fx.calls(), calls);
        assert_eq!(fx.logical("notification_claimed").len(), 1);
        assert!(
            fx.ctx
                .store
                .get(&task.id)
                .unwrap()
                .attention
                .source_notices
                .is_empty()
        );
        assert_eq!(
            load(&fx.ctx, "fixture-wait")
                .unwrap()
                .unwrap()
                .notice
                .unwrap()
                .producer,
            "provider_wait"
        );
    }
}
