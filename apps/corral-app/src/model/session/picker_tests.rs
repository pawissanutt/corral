use corral_protocol::detail::{DetailBody, DetailRef, ModelChoice, SessionModels};
use corral_protocol::session_state::{Capability, SessionState};

use super::{Action, Cx, Facts, PickerAction, PickerKind, SessionRole, State, Ui, model_picker};
use crate::model::Id;
use crate::model::daemon::{DetailCache, Feed, detail_key};
use crate::model::effect::Effect;
use crate::model::health::daemon_env;
use crate::model::nav::Form;
use crate::model::session::transcript::LiveSession;

const SUBSCRIPTION: &str = "anthropic/claude-sonnet-4-5";
const API: &str = "anthropic-api/claude-sonnet-4-5";

fn model(id: &str, provider: &str) -> ModelChoice {
    ModelChoice {
        id: id.into(),
        provider: provider.into(),
        name: Some("Claude Sonnet 4.5".into()),
        context_window: Some(200_000),
        effort_levels: vec!["low".into(), "medium".into(), "high".into()],
        scoped: false,
        note: None,
    }
}

/// A session on the subscription model whose catalog also serves the same model by API.
fn feed_with_catalog(id: &Id) -> Feed {
    let mut feed = Feed::default();
    let mut live = LiveSession::default();
    live.replace_state(Some(Box::new(SessionState {
        model: Some(SUBSCRIPTION.into()),
        can: vec![Capability {
            id: "model".into(),
            unavailable: None,
        }],
        ..SessionState::default()
    })));
    feed.live.insert(id.clone(), live);
    let target = DetailRef::SessionModels { id: id.to_string() };
    feed.details.insert(
        detail_key(&target),
        DetailCache {
            target,
            pages: Vec::new(),
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::SessionModels(SessionModels {
                current: Some(SUBSCRIPTION.into()),
                default: Some(SUBSCRIPTION.into()),
                effort: Some("medium".into()),
                models: vec![
                    model(SUBSCRIPTION, "anthropic"),
                    model(API, "anthropic-api"),
                ],
                refreshed_at: None,
                refreshing: false,
            })),
        },
    );
    feed
}

/// The subscription (`anthropic`) and API (`anthropic-api`) providers of one vendor are
/// distinct providers. Merging them by model name, or showing them without their provider,
/// would let the owner switch billing routes without seeing it.
#[test]
fn subscription_and_api_models_of_one_vendor_are_separate_choices() {
    let id = Id::from("B-k3x9");
    let feed = feed_with_catalog(&id);
    let facts = Facts {
        role: SessionRole::Build,
        task: None,
    };
    let mut ui = Ui::default();
    ui.picker.open = Some(PickerKind::Model);
    let cx = Cx::new(&feed, &ui, &id, daemon_env(&feed, Form::Desktop), &facts);
    let picker = model_picker(&cx).expect("the model list is open");

    let providers: Vec<Option<&str>> = picker
        .rows
        .iter()
        .map(|row| row.provider.as_deref())
        .collect();
    assert_eq!(providers, [Some("anthropic"), Some("anthropic-api")]);
    assert!(picker.rows[0].current && !picker.rows[1].current);
    let pick_api = picker.rows[1]
        .action
        .clone()
        .expect("the API model can be chosen");

    let mut state = State::default();
    state.sessions.insert(id.clone(), ui.clone());
    let sent: Vec<String> = state
        .act(pick_api, &feed)
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::SessionModel { request, .. } => Some(request.model),
            _ => None,
        })
        .collect();
    assert_eq!(sent, [API]);

    // Filtering by the provider finds the API route alone.
    state.act(
        Action::Picker(PickerAction::Open {
            session: id.clone(),
            kind: PickerKind::Model,
        }),
        &feed,
    );
    state.act(
        Action::Picker(PickerAction::Filter {
            session: id.clone(),
            text: "api".into(),
        }),
        &feed,
    );
    let ui = state.ui(id.as_str()).clone();
    let cx = Cx::new(&feed, &ui, &id, daemon_env(&feed, Form::Desktop), &facts);
    let filtered = model_picker(&cx).expect("the model list is open");
    let models: Vec<&str> = filtered.rows.iter().map(|row| row.model.as_str()).collect();
    assert_eq!(models, [API]);
}
