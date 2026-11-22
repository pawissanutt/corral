use corral_protocol::detail::{DetailBody, DetailRef, ModelChoice, SessionModels};
use corral_protocol::session_host::SessionAction;
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
        favorite: false,
        note: None,
    }
}

/// A session on the subscription model whose catalog also serves the same model by API.
fn feed_with_catalog(id: &Id) -> Feed {
    feed_with(
        id,
        vec![
            model(SUBSCRIPTION, "anthropic"),
            model(API, "anthropic-api"),
        ],
    )
}

/// A session on the subscription model with `models` as its catalog, whose host keeps
/// favorites.
fn feed_with(id: &Id, models: Vec<ModelChoice>) -> Feed {
    let mut feed = Feed::default();
    let mut live = LiveSession::default();
    live.replace_state(Some(Box::new(SessionState {
        model: Some(SUBSCRIPTION.into()),
        can: ["model", "model_favorite"]
            .into_iter()
            .map(|id| Capability {
                id: id.into(),
                unavailable: None,
            })
            .collect(),
        ..SessionState::default()
    })));
    feed.live.insert(id.clone(), live);
    let target = DetailRef::SessionModels { id: id.to_string() };
    feed.details.insert(
        detail_key(&target),
        DetailCache {
            transcript_seams: Default::default(),
            target,
            pages: Vec::new(),
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::SessionModels(SessionModels {
                current: Some(SUBSCRIPTION.into()),
                default: Some(SUBSCRIPTION.into()),
                effort: Some("medium".into()),
                models,
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

/// A favorite listed under the rest while filtering is lost in a long catalog, and a star that
/// also chose the model or closed the list would switch the session's model when the owner only
/// meant to mark it.
#[test]
fn favorites_lead_the_filtered_list_and_starring_keeps_the_model_and_the_list() {
    let id = Id::from("B-k3x9");
    let favorite = ModelChoice {
        favorite: true,
        ..model(API, "anthropic-api")
    };
    let feed = feed_with(&id, vec![model(SUBSCRIPTION, "anthropic"), favorite]);
    let facts = Facts {
        role: SessionRole::Build,
        task: None,
    };
    let mut ui = Ui::default();
    ui.picker.open = Some(PickerKind::Model);
    ui.picker.filter = "sonnet".into();
    let cx = Cx::new(&feed, &ui, &id, daemon_env(&feed, Form::Desktop), &facts);
    let picker = model_picker(&cx).expect("the model list is open");
    let listed: Vec<(&str, Option<bool>)> = picker
        .rows
        .iter()
        .map(|row| (row.model.as_str(), row.favorite))
        .collect();
    assert_eq!(listed, [(API, Some(true)), (SUBSCRIPTION, Some(false))]);
    assert!(
        picker.rows[1].action.is_some(),
        "a non-favorite stays selectable"
    );

    let star = picker.rows[1]
        .star
        .clone()
        .expect("the host keeps favorites");
    let mut state = State::default();
    state.sessions.insert(id.clone(), ui.clone());
    let effects = state.act(star, &feed);
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::SessionModel { .. }))
    );
    let marked: Vec<(String, bool)> = effects
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::SessionAction { request, .. } => match request.action {
                SessionAction::ModelFavorite { model, favorite } => Some((model, favorite)),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(marked, [(SUBSCRIPTION.to_owned(), true)]);
    assert_eq!(state.ui(id.as_str()).picker.open, Some(PickerKind::Model));
}
