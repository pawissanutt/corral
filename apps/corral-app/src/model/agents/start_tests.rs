use super::*;

#[test]
fn submitting_code_sends_only_the_trusted_code_start_fields() {
    let mut feed = crate::model::preview::feed(0, String::new(), String::new(), None);
    let project = feed.projects.keys().next().unwrap().to_string();
    let mut state = Start::default();
    let mut ids = IdSupply::default();
    let filter = ProjectFilter::default();
    for action in [
        StartAction::OpenCode,
        StartAction::Project(project.clone()),
        StartAction::Name(" Check the docs ".into()),
        StartAction::Model(ModelPick::Model("provider/model".into())),
        StartAction::Effort(Some("low".into())),
    ] {
        assert!(state.act(action, &mut ids, &feed, &filter).is_empty());
    }
    let effects = state.act(StartAction::Submit, &mut ids, &feed, &filter);
    let [effect @ Effect::CodeStart { key, start }] = effects.as_slice() else {
        panic!("expected a Code start");
    };
    assert_eq!(
        start,
        &CodeStart {
            project: project.clone(),
            name: Some("Check the docs".into()),
            model: Some("provider/model".into()),
            effort: Some("low".into()),
        }
    );
    crate::model::preview::effect(&mut feed, effect).unwrap();
    let reply = feed
        .operations
        .get(key.as_str())
        .unwrap()
        .decode::<AssistantRow>()
        .unwrap();
    let build = reply.build.unwrap();
    assert_eq!(build.mode, corral_protocol::build::BuildMode::Code);
    assert_eq!(build.worktree, build.repo_root);
    assert!(form(&state, &feed, &filter).unwrap().warning.is_some());
}
