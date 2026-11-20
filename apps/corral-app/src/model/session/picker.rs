//! The model and effort pickers of a session (`/model`, Ctrl+Shift+M, the composer's model pill;
//! `Alt+E` cycles the effort). The model list is the host's catalog (`DetailRef::SessionModels`):
//! the workspace's favorites first, then the rest, each group the current model first, then the
//! default, then the host's order; each row names its provider
//! as the host reports it, so a vendor's subscription and API providers (`anthropic`,
//! `anthropic-api`) stay separate choices and are never merged by model name. Typing filters
//! (every word must match the model, its name or its provider); `All | Scoped` narrows to the
//! session's scoped models where the host has any. Choosing sends `session.model`; the composer
//! follows the host's live state, and a refused change keeps the model and says why as a notice.
//! A row's star (or `Alt+S` on the highlighted row) marks or unmarks the model as a favorite
//! through `session.action` (`ModelFavorite`), keeping the list open and the session's model; the
//! host keeps favorites for the workspace and re-announces its catalog, so the star follows the
//! catalog, never this device. The effort list is the current model's levels.

use corral_protocol::detail::{DetailBody, DetailGet, DetailRef, ModelChoice, SessionModels};
use corral_protocol::session_host::{SessionAction, SessionActionRequest, SessionModel};
use corral_protocol::session_state::SessionState;

use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;
use crate::model::session::{Action, Can, Cx, Host, State};
use crate::model::session::{Pending, ReadState};

/// Which list is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    Model,
    Effort,
}

/// The pickers' and the `/` menu's state on this device, per session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerUi {
    pub open: Option<PickerKind>,
    pub filter: String,
    /// Only the session's scoped models.
    pub scoped: bool,
    /// The highlighted row of the open list.
    pub at: usize,
    /// The model the highlight stays on while a favorite change reorders the list.
    pub follow: Option<String>,
    /// The highlighted row of the `/` menu.
    pub slash_at: usize,
    /// The draft the `/` menu was closed on (Esc keeps the text); it opens again once the draft
    /// changes.
    pub slash_closed: Option<String>,
}

impl PickerUi {
    pub const EMPTY: Self = Self {
        open: None,
        filter: String::new(),
        scoped: false,
        at: 0,
        follow: None,
        slash_at: 0,
        slash_closed: None,
    };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerAction {
    Open {
        session: Id,
        kind: PickerKind,
    },
    Close(Id),
    Filter {
        session: Id,
        text: String,
    },
    Scoped {
        session: Id,
        scoped: bool,
    },
    /// Move the highlight by `delta` rows.
    Step {
        session: Id,
        delta: i8,
    },
    /// Choose the highlighted row.
    Choose(Id),
    /// Use this model for the session.
    Pick {
        session: Id,
        model: String,
    },
    /// Use this effort with the current model.
    Effort {
        session: Id,
        effort: String,
    },
    /// The current model's next effort level.
    CycleEffort(Id),
    /// The next (or, with `-1`, the previous) model of the list, scoped models first.
    NextModel {
        session: Id,
        delta: i8,
    },
    /// Mark (or unmark) this model as a favorite of the workspace; the list stays open.
    Favorite {
        session: Id,
        model: String,
        favorite: bool,
    },
}

/// The open picker as it shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Picker {
    pub kind: PickerKind,
    /// `Model for this session`, `Effort for anthropic/claude-sonnet-4-5`.
    pub title: String,
    pub filter: String,
    /// `3 / 41` while filtering.
    pub count: Option<String>,
    /// `All | Scoped`, where the host has scoped models.
    pub scope: Option<ScopeSwitch>,
    /// The catalog's refresh state: `Refreshing model catalogs…`, `Catalogs refreshed 12:04`.
    pub catalog: Option<(Hue, String)>,
    pub rows: Vec<PickRow>,
    /// The highlighted row.
    pub at: Option<usize>,
    /// The highlighted row's `name · provider/model`.
    pub detail: Option<String>,
    /// Why the list is empty, or what is missing.
    pub note: Option<String>,
    pub read: ReadState,
    /// The host cannot change the model now: rows stay listed and do not run.
    pub unavailable: Option<String>,
    /// The host cannot change favorites now: the stars stay listed and do not run.
    pub favorites: Option<String>,
    pub close: Action,
    /// The action typing into the filter runs, without its text.
    pub session: Id,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeSwitch {
    pub scoped: bool,
    pub all: Action,
    pub only: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickRow {
    /// `pick-<n>`.
    pub id: String,
    /// The model's name (its id when the host names none), or the effort level.
    pub label: String,
    /// `<provider>/<model>`; empty for an effort row.
    pub model: String,
    /// As the host reports it: `anthropic`, `anthropic-api`.
    pub provider: Option<String>,
    /// `200k context · effort low–high`; an effort's description.
    pub meta: String,
    /// `current`, `default`, `→ next`.
    pub marks: Vec<&'static str>,
    pub current: bool,
    /// None while the host cannot change it now.
    pub action: Option<Action>,
    /// Whether the model is a favorite of the workspace; none for an effort row.
    pub favorite: Option<bool>,
    /// Marks or unmarks it (`pick-<n>-favorite`); none while the host cannot.
    pub star: Option<Action>,
}

/// One choice of a list, before the page decides what pressing it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Choice {
    Model(ModelChoice),
    Effort(String),
}

fn models<'f>(feed: &'f Feed, session: &str) -> Option<&'f SessionModels> {
    match feed.detail(&target(session))?.body()? {
        DetailBody::SessionModels(models) => Some(models),
        _ => None,
    }
}

fn target(session: &str) -> DetailRef {
    DetailRef::SessionModels {
        id: session.to_owned(),
    }
}

fn live_state<'f>(feed: &'f Feed, session: &str) -> Option<&'f SessionState> {
    feed.live.get(session)?.state.as_ref()
}

/// The session's model: the host's live one, else the catalog's current.
fn current_model(feed: &Feed, session: &str) -> Option<String> {
    live_state(feed, session)
        .and_then(|state| state.model.clone())
        .or_else(|| models(feed, session).and_then(|models| models.current.clone()))
}

/// Whether every word of `filter` is found in the model's id, name or provider.
fn matches(model: &ModelChoice, filter: &str) -> bool {
    let haystack = format!(
        "{} {} {}",
        model.id,
        model.name.as_deref().unwrap_or_default(),
        model.provider
    )
    .to_lowercase();
    filter
        .to_lowercase()
        .split_whitespace()
        .all(|word| haystack.contains(word))
}

/// The catalog in the picker's order: favorites, then the rest, each the current model, the
/// default, then the host's order.
fn ordered<'m>(models: &'m SessionModels, current: Option<&str>) -> Vec<&'m ModelChoice> {
    let rank = |model: &ModelChoice| {
        let place = if Some(model.id.as_str()) == current {
            0
        } else if Some(&model.id) == models.default.as_ref() {
            1
        } else {
            2
        };
        (!model.favorite, place)
    };
    let mut ordered: Vec<&ModelChoice> = models.models.iter().collect();
    ordered.sort_by_key(|model| rank(model));
    ordered
}

/// The choices the open list (or `kind`) offers now, in its order, filtered.
pub(crate) fn choices(feed: &Feed, session: &str, ui: &PickerUi, kind: PickerKind) -> Vec<Choice> {
    let current = current_model(feed, session);
    match kind {
        PickerKind::Model => {
            let Some(models) = models(feed, session) else {
                return Vec::new();
            };
            let scoped = ui.scoped && models.models.iter().any(|model| model.scoped);
            ordered(models, current.as_deref())
                .into_iter()
                .filter(|model| !scoped || model.scoped)
                .filter(|model| matches(model, &ui.filter))
                .map(|model| Choice::Model(model.clone()))
                .collect()
        }
        PickerKind::Effort => levels(feed, session, current.as_deref())
            .into_iter()
            .filter(|level| level.contains(ui.filter.trim()))
            .map(Choice::Effort)
            .collect(),
    }
}

/// The current model's effort levels: the host's live ones, else the catalog's.
fn levels(feed: &Feed, session: &str, current: Option<&str>) -> Vec<String> {
    live_state(feed, session)
        .map(|state| state.effort_levels.clone())
        .filter(|levels| !levels.is_empty())
        .or_else(|| {
            models(feed, session)?
                .models
                .iter()
                .find(|model| Some(model.id.as_str()) == current)
                .map(|model| model.effort_levels.clone())
        })
        .unwrap_or_default()
}

/// `200k`, `1M`.
fn window(tokens: u64) -> String {
    if tokens >= 1_000_000 && tokens % 1_000_000 == 0 {
        format!("{}M", tokens / 1_000_000)
    } else if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

/// What an effort level means, in the terminal UI's words.
fn effort_words(level: &str) -> &'static str {
    match level {
        "off" => "no thinking",
        "minimal" => "the least thinking",
        "low" => "a little thinking",
        "medium" => "balanced thinking",
        "high" => "more thinking",
        "xhigh" => "the most thinking",
        _ => "",
    }
}

fn model_meta(model: &ModelChoice) -> String {
    let mut parts = Vec::new();
    match model.context_window {
        Some(tokens) => parts.push(format!("{} context", window(tokens))),
        None => parts.push("context window not reported".to_owned()),
    }
    match (model.effort_levels.first(), model.effort_levels.last()) {
        (Some(first), Some(last)) if first != last => parts.push(format!("effort {first}–{last}")),
        (Some(only), _) => parts.push(format!("effort {only}")),
        _ => parts.push("no thinking".to_owned()),
    }
    if let Some(note) = &model.note {
        parts.push(note.clone());
    }
    parts.join(" · ")
}

/// Why the session cannot change its model now, if it cannot.
pub(crate) fn unavailable(cx: &Cx<'_>) -> Option<String> {
    match cx.can("model") {
        Can::Absent => return Some("this session does not change its model".into()),
        Can::Unavailable(why) => return Some(why),
        Can::Yes | Can::Unknown => {}
    }
    match cx.host {
        Host::NotRunning | Host::Ended => Some("the host is not running".into()),
        Host::Unreachable => Some("corrald cannot reach the host".into()),
        Host::Running | Host::Unknown => None,
    }
}

/// Why the session's host cannot mark favorites now, if it cannot.
fn favorites_unavailable(cx: &Cx<'_>) -> Option<String> {
    match cx.can("model_favorite") {
        Can::Absent => return Some("this session's host does not keep favorites".into()),
        Can::Unavailable(why) => return Some(why),
        Can::Yes | Can::Unknown => {}
    }
    match cx.host {
        Host::NotRunning | Host::Ended => Some("the host is not running".into()),
        Host::Unreachable => Some("corrald cannot reach the host".into()),
        Host::Running | Host::Unknown => None,
    }
}

fn row_of(cx: &Cx<'_>, at: usize, choice: &Choice, blocked: bool, stars: bool) -> PickRow {
    let session = cx.id.clone();
    let current = cx.model();
    match choice {
        Choice::Model(model) => {
            let is_current = current.as_deref() == Some(model.id.as_str());
            let mut marks = Vec::new();
            if is_current {
                marks.push("current");
            }
            let models = models(cx.feed, cx.id.as_str());
            if models.and_then(|models| models.default.as_deref()) == Some(model.id.as_str()) {
                marks.push("default");
            }
            if cx.state.and_then(|state| state.pending_model.as_deref()) == Some(model.id.as_str())
            {
                marks.push("next");
            }
            PickRow {
                id: format!("pick-{at}"),
                label: model.name.clone().unwrap_or_else(|| model.id.clone()),
                model: model.id.clone(),
                provider: Some(model.provider.clone()),
                meta: model_meta(model),
                marks,
                current: is_current,
                star: stars.then(|| {
                    Action::Picker(PickerAction::Favorite {
                        session: session.clone(),
                        model: model.id.clone(),
                        favorite: !model.favorite,
                    })
                }),
                favorite: Some(model.favorite),
                action: (!blocked).then(|| {
                    if is_current {
                        Action::Picker(PickerAction::Close(session))
                    } else {
                        Action::Picker(PickerAction::Pick {
                            session,
                            model: model.id.clone(),
                        })
                    }
                }),
            }
        }
        Choice::Effort(level) => {
            let is_current = cx.effort().as_deref() == Some(level.as_str());
            PickRow {
                id: format!("pick-{at}"),
                label: level.clone(),
                model: String::new(),
                provider: None,
                meta: effort_words(level).to_owned(),
                marks: if is_current {
                    vec!["current"]
                } else {
                    Vec::new()
                },
                current: is_current,
                favorite: None,
                star: None,
                action: (!blocked).then(|| {
                    Action::Picker(PickerAction::Effort {
                        session,
                        effort: level.clone(),
                    })
                }),
            }
        }
    }
}

/// The catalog's refresh state, as the host reports it.
fn catalog(cx: &Cx<'_>, models: &SessionModels) -> Option<(Hue, String)> {
    if models.refreshing {
        return Some((Hue::Neutral, "Refreshing model catalogs…".into()));
    }
    let at = models.refreshed_at.as_deref()?;
    let clock = crate::model::agents::clock(Some(at), cx.feed)?;
    Some((Hue::Neutral, format!("Model catalogs refreshed {clock}")))
}

/// The highlighted row of `shown`: the followed model's while it is listed, else `at`.
fn highlight(shown: &[Choice], ui: &PickerUi) -> usize {
    ui.follow
        .as_deref()
        .and_then(|follow| {
            shown
                .iter()
                .position(|choice| matches!(choice, Choice::Model(model) if model.id == follow))
        })
        .unwrap_or(ui.at)
        .min(shown.len().saturating_sub(1))
}

/// The open picker of the session `cx` reads; none while no list is open.
pub fn picker(cx: &Cx<'_>) -> Option<Picker> {
    let ui = &cx.ui.picker;
    let kind = ui.open?;
    let session = cx.id.clone();
    let blocked = unavailable(cx);
    let favorites = (kind == PickerKind::Model)
        .then(|| favorites_unavailable(cx))
        .flatten();
    let all = choices(
        cx.feed,
        cx.id.as_str(),
        &PickerUi {
            filter: String::new(),
            ..ui.clone()
        },
        kind,
    );
    let shown = choices(cx.feed, cx.id.as_str(), ui, kind);
    let rows: Vec<PickRow> = shown
        .iter()
        .enumerate()
        .map(|(at, choice)| row_of(cx, at, choice, blocked.is_some(), favorites.is_none()))
        .collect();
    let at = (!rows.is_empty()).then(|| highlight(&shown, ui));
    let detail = at.and_then(|at| rows.get(at)).and_then(|row| {
        row.provider
            .as_ref()
            .map(|_| format!("{} · {}", row.label, row.model))
    });
    let served = models(cx.feed, cx.id.as_str());
    let filtering = !ui.filter.trim().is_empty();
    let note = match kind {
        PickerKind::Model if served.is_some_and(|models| models.models.is_empty()) => Some(
            "Only providers you have signed in to are listed. Sign in from a terminal with corral-pi login --provider <id>."
                .to_owned(),
        ),
        PickerKind::Effort if all.is_empty() => Some(match cx.model() {
            Some(model) => format!("{model} has no effort levels."),
            None => "The host has not reported the session's model yet.".to_owned(),
        }),
        _ if filtering && rows.is_empty() => Some(format!("Nothing matches “{}”.", ui.filter.trim())),
        _ => None,
    };
    let title = match kind {
        PickerKind::Model => "Model for this session".to_owned(),
        PickerKind::Effort => match cx.model() {
            Some(model) => format!("Effort for {model}"),
            None => "Effort".to_owned(),
        },
    };
    Some(Picker {
        kind,
        title,
        filter: ui.filter.clone(),
        count: filtering.then(|| format!("{} / {}", rows.len(), all.len())),
        scope: served
            .filter(|models| kind == PickerKind::Model && models.models.iter().any(|m| m.scoped))
            .map(|_| ScopeSwitch {
                scoped: ui.scoped,
                all: Action::Picker(PickerAction::Scoped {
                    session: session.clone(),
                    scoped: false,
                }),
                only: Action::Picker(PickerAction::Scoped {
                    session: session.clone(),
                    scoped: true,
                }),
            }),
        catalog: served
            .filter(|_| kind == PickerKind::Model)
            .and_then(|models| catalog(cx, models)),
        read: ReadState::of(cx.feed.detail(&target(cx.id.as_str())), cx.feed),
        rows,
        at,
        detail,
        note,
        unavailable: blocked,
        favorites,
        close: Action::Picker(PickerAction::Close(session.clone())),
        session,
    })
}

/// The catalog read the open model list needs: once while nothing is read and the host can
/// serve it. Opening the list rereads it.
pub(crate) fn fetches(cx: &Cx<'_>) -> Option<Action> {
    let open = cx.ui.picker.open?;
    let current = cx.model();
    let wanted =
        open == PickerKind::Model || levels(cx.feed, cx.id.as_str(), current.as_deref()).is_empty();
    let servable = !matches!(cx.can("model"), Can::Absent) && cx.host == Host::Running;
    (wanted && servable && cx.feed.detail(&target(cx.id.as_str())).is_none())
        .then(|| Action::Refresh(target(cx.id.as_str())))
}

/// Revalidates the catalog with the etag of the page shown.
fn reread(feed: &Feed, session: &str) -> Effect {
    let target = target(session);
    Effect::Detail(DetailGet {
        etag: feed
            .detail(&target)
            .and_then(|cache| cache.pages.first())
            .map(|page| page.etag.clone()),
        target,
        page: None,
    })
}

/// Sends `session.model` under a new key; a refusal is told as a notice when it settles.
fn send(
    state: &mut State,
    session: &Id,
    model: String,
    effort: Option<String>,
    what: &'static str,
) -> Vec<Effect> {
    let key = state.ids.next_id();
    let ui = state.ui_mut(session);
    ui.picker.open = None;
    ui.dock
        .pending
        .insert(key.clone(), Pending::Action { what });
    vec![Effect::SessionModel {
        key,
        request: SessionModel {
            id: session.to_string(),
            model,
            effort,
        },
    }]
}

/// Uses `effort` with the session's current model.
fn effort(state: &mut State, session: &Id, effort: String, feed: &Feed) -> Vec<Effect> {
    match current_model(feed, session.as_str()) {
        Some(model) => send(state, session, model, Some(effort), "Effort change"),
        None => {
            state.ui_mut(session).notices.push(
                Hue::Held,
                "The host has not reported the session's model yet; the effort is unchanged",
                feed.now,
            );
            Vec::new()
        }
    }
}

fn open(state: &mut State, session: &Id, kind: PickerKind, feed: &Feed) -> Vec<Effect> {
    let ui = state.ui_mut(session);
    ui.picker.open = Some(kind);
    ui.picker.filter.clear();
    ui.picker.at = 0;
    ui.picker.follow = None;
    let reads = kind == PickerKind::Model
        || levels(
            feed,
            session.as_str(),
            current_model(feed, session.as_str()).as_deref(),
        )
        .is_empty();
    let loading = feed
        .detail(&target(session.as_str()))
        .is_some_and(|cache| cache.loading);
    if reads && !loading && state_serves(feed, session.as_str()) {
        vec![reread(feed, session.as_str())]
    } else {
        Vec::new()
    }
}

/// Whether the host may be asked for its catalog: it declares model changes, or nothing yet.
fn state_serves(feed: &Feed, session: &str) -> bool {
    live_state(feed, session).is_none_or(|state| {
        state.can.is_empty() || state.can.iter().any(|capability| capability.id == "model")
    })
}

pub(crate) fn act(state: &mut State, action: PickerAction, feed: &Feed) -> Vec<Effect> {
    match action {
        PickerAction::Open { session, kind } => open(state, &session, kind, feed),
        PickerAction::Close(session) => {
            state.ui_mut(&session).picker.open = None;
            Vec::new()
        }
        PickerAction::Filter { session, text } => {
            let ui = state.ui_mut(&session);
            ui.picker.filter = text;
            ui.picker.at = 0;
            ui.picker.follow = None;
            Vec::new()
        }
        PickerAction::Scoped { session, scoped } => {
            let ui = state.ui_mut(&session);
            ui.picker.scoped = scoped;
            ui.picker.at = 0;
            ui.picker.follow = None;
            Vec::new()
        }
        PickerAction::Step { session, delta } => {
            let ui = state.ui(session.as_str()).picker.clone();
            let shown = ui
                .open
                .map_or_else(Vec::new, |kind| choices(feed, session.as_str(), &ui, kind));
            let last = shown.len().saturating_sub(1);
            let at = highlight(&shown, &ui)
                .saturating_add_signed(isize::from(delta))
                .min(last);
            // The highlight keeps its model, so a catalog reordered by a favorite change
            // arriving later never moves it onto another model.
            let follow = match shown.get(at) {
                Some(Choice::Model(model)) => Some(model.id.clone()),
                _ => None,
            };
            let picker = &mut state.ui_mut(&session).picker;
            picker.at = at;
            picker.follow = follow;
            Vec::new()
        }
        PickerAction::Choose(session) => {
            let ui = state.ui(session.as_str()).picker.clone();
            let Some(kind) = ui.open else {
                return Vec::new();
            };
            let shown = choices(feed, session.as_str(), &ui, kind);
            let Some(choice) = shown.get(highlight(&shown, &ui)).cloned() else {
                return Vec::new();
            };
            match choice {
                Choice::Model(model)
                    if current_model(feed, session.as_str()).as_deref()
                        == Some(model.id.as_str()) =>
                {
                    state.ui_mut(&session).picker.open = None;
                    Vec::new()
                }
                Choice::Model(model) => send(state, &session, model.id, None, "Model change"),
                Choice::Effort(level) => effort(state, &session, level, feed),
            }
        }
        PickerAction::Pick { session, model } => send(state, &session, model, None, "Model change"),
        PickerAction::Effort {
            session,
            effort: level,
        } => effort(state, &session, level, feed),
        PickerAction::CycleEffort(session) => {
            let current = current_model(feed, session.as_str());
            let levels = levels(feed, session.as_str(), current.as_deref());
            if levels.is_empty() {
                state.ui_mut(&session).notices.push(
                    Hue::Neutral,
                    "This model has no effort levels",
                    feed.now,
                );
                return Vec::new();
            }
            let now = live_state(feed, session.as_str()).and_then(|state| state.effort.clone());
            let next = now
                .and_then(|now| levels.iter().position(|level| *level == now))
                .map_or(0, |at| (at + 1) % levels.len());
            match levels.get(next).cloned() {
                Some(level) => effort(state, &session, level, feed),
                None => Vec::new(),
            }
        }
        PickerAction::NextModel { session, delta } => {
            let Some(models) = models(feed, session.as_str()) else {
                return open(state, &session, PickerKind::Model, feed);
            };
            let current = current_model(feed, session.as_str());
            let scoped = models.models.iter().any(|model| model.scoped);
            let cycle: Vec<&ModelChoice> = models
                .models
                .iter()
                .filter(|model| !scoped || model.scoped)
                .collect();
            if cycle.is_empty() {
                return open(state, &session, PickerKind::Model, feed);
            }
            let len = cycle.len();
            let at = current
                .and_then(|current| cycle.iter().position(|model| model.id == current))
                .map_or(0, |at| {
                    (at + len).saturating_add_signed(isize::from(delta)) % len
                });
            match cycle.get(at).map(|model| model.id.clone()) {
                Some(model) => send(state, &session, model, None, "Model change"),
                None => Vec::new(),
            }
        }
        PickerAction::Favorite {
            session,
            model,
            favorite,
        } => favorite_model(state, &session, model, favorite, feed),
    }
}

/// Sends `ModelFavorite` under a new key with the list left open; the star changes when the
/// host re-announces its catalog. The highlight stays on its model while the list reorders.
fn favorite_model(
    state: &mut State,
    session: &Id,
    model: String,
    favorite: bool,
    feed: &Feed,
) -> Vec<Effect> {
    let ui = state.ui(session.as_str()).picker.clone();
    let follow = ui.open.and_then(|kind| {
        let shown = choices(feed, session.as_str(), &ui, kind);
        match shown.get(highlight(&shown, &ui)) {
            Some(Choice::Model(highlighted)) => Some(highlighted.id.clone()),
            _ => None,
        }
    });
    let key = state.ids.next_id();
    let ui = state.ui_mut(session);
    ui.picker.follow = follow;
    let what = if favorite { "Favorite" } else { "Unfavorite" };
    ui.dock
        .pending
        .insert(key.clone(), Pending::Action { what });
    vec![Effect::SessionAction {
        key,
        request: SessionActionRequest {
            id: session.to_string(),
            action: SessionAction::ModelFavorite { model, favorite },
        },
    }]
}
