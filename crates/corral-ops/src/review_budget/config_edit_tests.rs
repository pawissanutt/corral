use super::*;

fn edit(fx: &Fixture, change: impl FnOnce(&mut corral_core::Config)) {
    let mut other = Store::open(fx.ctx.store.root()).unwrap();
    change(&mut other.config);
    other.save_config().unwrap();
}

fn rule(value: serde_json::Value) -> corral_core::RoutingRule {
    serde_json::from_value(value).unwrap()
}

type Edit = fn(&mut corral_core::Config);

#[test]
fn reservation_rejects_only_edits_that_change_this_tasks_reviewer() {
    let edits: [(&str, bool, Edit); 4] = [
        ("implementer routing rule", true, |c| {
            c.routing
                .rules
                .push(rule(json!({"project": "app", "profile": "small"})))
        }),
        ("unmatched reviewer rule", true, |c| {
            c.routing
                .reviewer_rules
                .push(rule(json!({"size": ["xs", "s"], "profile": "small"})))
        }),
        ("matching reviewer rule", false, |c| {
            c.routing
                .reviewer_rules
                .push(rule(json!({"project": "app", "profile": "small"})))
        }),
        ("gate command", false, |c| {
            c.review.gate = Some(vec!["false".into()])
        }),
    ];
    for (label, admitted, change) in edits {
        // Given a launcher holding the config snapshot it prepared the gate under.
        let mut fx = Fixture::new(1);
        fx.ctx.store.config.review.reviewer = Some("rev".into());
        fx.ctx.store.config.agents.insert(
            "small".into(),
            AgentCmd {
                command: "true".into(),
            },
        );
        fx.ctx.store.save_config().unwrap();
        let id = fx.task();
        // When another store edits config before the reservation commits.
        edit(&fx, change);
        let result = ReviewLaunch::reserve(&fx.ctx, &id, Kind::Gate);
        // Then only an edit that changes this task's verification inputs invalidates it.
        match (admitted, result) {
            (true, Ok(launch)) => launch.commit().unwrap(),
            (false, Err(error)) => assert!(
                error
                    .to_string()
                    .contains("review configuration changed before reservation"),
                "{label}: {error:#}"
            ),
            (admitted, result) => panic!("{label}: admitted={admitted} ok={}", result.is_ok()),
        }
    }
}
