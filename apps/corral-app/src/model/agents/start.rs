//! Starting a session from Sessions: the New menu (a new assistant session, or the New build
//! sheet) and the sheet itself: the project, a name the host may choose, a new worktree or the
//! project's checkout, the model and the effort. The choices offered are what `corral.toml`
//! names (the configured build route, `[build.profiles]` and the models profiles name), plus a
//! typed `<provider>/<model>`; the host decides whether a start can run, and its refusal comes
//! back with its two lines on the refusal sheet, the sheet keeping every field.

use std::collections::BTreeSet;

use corral_protocol::build::BuildStart;
use corral_protocol::rows::AssistantRow;
use serde_json::Value;

use crate::model::act::RefusalSheet;
use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::id::IdSupply;
use crate::model::projects::ProjectFilter;
use crate::model::{Id, Live};

use super::Action;

/// Where the build works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Place {
    /// A new branch in its own worktree; the checkout is untouched.
    #[default]
    Worktree,
    /// The project's checkout, on its current branch.
    Checkout,
}

impl Place {
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Worktree => "worktree",
            Self::Checkout => "checkout",
        }
    }
}

/// The model the build starts on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ModelPick {
    /// The configured build route.
    #[default]
    Route,
    /// A `[build.profiles]` entry; its model and effort apply unless set here.
    Profile(String),
    /// A `<provider>/<model>`, chosen from those `corral.toml` names or typed.
    Model(String),
}

/// The sheet's fields and the start in flight.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Start {
    pub open: bool,
    /// The chosen project; none until one is pressed (the default then applies).
    pub project: Option<String>,
    pub name: String,
    pub place: Place,
    pub model: ModelPick,
    pub effort: Option<String>,
    /// The `build.start` in flight.
    pub sending: Option<Id>,
    /// Why the last start failed without a typed refusal.
    pub failed: Option<String>,
}

/// What the New menu and the sheet do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartAction {
    Open,
    Close,
    Project(String),
    Name(String),
    Place(Place),
    Model(ModelPick),
    Effort(Option<String>),
    Submit,
}

/// The New menu's entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewPress {
    /// `assistant.session` new, then its page (the assistant's own action).
    Assistant,
    /// The New build sheet.
    Build,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMenu {
    /// `btn-new`.
    pub id: &'static str,
    pub label: &'static str,
    pub items: Vec<NewItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewItem {
    /// `btn-new-assistant`, `btn-new-build`.
    pub id: &'static str,
    pub label: &'static str,
    /// The terminal and desktop key (`N` opens the build sheet).
    pub key: Option<&'static str>,
    pub press: NewPress,
}

pub fn new_menu() -> NewMenu {
    NewMenu {
        id: "btn-new",
        label: "New",
        items: vec![
            NewItem {
                id: "btn-new-assistant",
                label: "New assistant session",
                key: None,
                press: NewPress::Assistant,
            },
            NewItem {
                id: "btn-new-build",
                label: "New build session…",
                key: Some("N"),
                press: NewPress::Build,
            },
        ],
    }
}

/// One pressable choice of a field (`seg-<group>-<value>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    pub label: String,
    pub detail: Option<String>,
    pub selected: bool,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectField {
    pub label: &'static str,
    pub choices: Vec<Choice>,
    /// Said when the workspace has no project to build in.
    pub missing: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameField {
    /// `field-build-name`.
    pub id: &'static str,
    pub label: &'static str,
    pub value: String,
    pub hint: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceField {
    pub label: &'static str,
    pub choices: Vec<Choice>,
    /// What the chosen place means for this project.
    pub explain: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelField {
    pub label: &'static str,
    /// `field-build-model`: a typed `<provider>/<model>`.
    pub id: &'static str,
    pub typed: String,
    pub choices: Vec<Choice>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffortField {
    pub label: &'static str,
    pub choices: Vec<Choice>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartButton {
    /// `btn-start-build`.
    pub id: &'static str,
    /// `Start build`, `Starting…` while it waits.
    pub label: &'static str,
    /// Dimmed with this reason.
    pub unavailable: Option<String>,
    pub action: Action,
}

/// The New build sheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewBuildForm {
    pub title: &'static str,
    pub project: ProjectField,
    pub name: NameField,
    pub place: PlaceField,
    pub model: ModelField,
    /// Absent when `corral.toml` names no effort level.
    pub effort: Option<EffortField>,
    pub start: StartButton,
    pub cancel: Action,
    pub error: Option<String>,
}

const NO_PROJECT: &str = "Builds need a project. Add one in Settings › Configuration.";

/// What the sheet says when corrald gave no answer in time: the build may exist already, so
/// the line points at Agents rather than reading as a failure to try again.
const NO_ANSWER: &str = "corrald did not answer in time. The build may still have started; look for it in Agents before starting another one.";

fn start(action: StartAction) -> Action {
    Action::Start(action)
}

/// The project a start uses: the one pressed, else the Agents filter's when it names one, else
/// the only project.
fn project(start: &Start, feed: &Feed, filter: &ProjectFilter) -> Option<String> {
    let known = |id: &str| feed.projects.contains_key(id);
    if let Some(id) = start.project.as_deref().filter(|id| known(id)) {
        return Some(id.to_owned());
    }
    if let ProjectFilter::Project(id) = filter.resolved(feed) {
        return Some(id.to_string());
    }
    let mut ids = feed.projects.keys();
    match (ids.next(), ids.next()) {
        (Some(only), None) => Some(only.to_string()),
        _ => None,
    }
}

fn config(feed: &Feed) -> Option<&serde_json::Map<String, Value>> {
    feed.settings.as_ref().map(|settings| &settings.config)
}

fn text<'a>(table: &'a Value, key: &str) -> Option<&'a str> {
    table.get(key).and_then(Value::as_str)
}

/// `[build.profiles]` names with their model and effort.
fn profiles(feed: &Feed) -> Vec<(String, Option<String>, Option<String>)> {
    config(feed)
        .and_then(|config| config.get("build"))
        .and_then(|build| build.get("profiles"))
        .and_then(Value::as_object)
        .map(|profiles| {
            profiles
                .iter()
                .map(|(name, profile)| {
                    (
                        name.clone(),
                        text(profile, "model").map(str::to_owned),
                        text(profile, "effort").map(str::to_owned),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every `<provider>/<model>` `corral.toml` names in its profiles.
fn models(feed: &Feed) -> BTreeSet<String> {
    let mut models = BTreeSet::new();
    let tables = config(feed)
        .and_then(|config| config.get("profiles"))
        .and_then(Value::as_object);
    for profile in tables.into_iter().flat_map(|tables| tables.values()) {
        if let Some(model) = text(profile, "model").filter(|model| model.contains('/')) {
            models.insert(model.to_owned());
        }
    }
    for (_, model, _) in profiles(feed) {
        models.extend(model.filter(|model| model.contains('/')));
    }
    models
}

/// The effort levels `corral.toml` names, in the usual order, then any others.
fn efforts(feed: &Feed) -> Vec<String> {
    let mut named: BTreeSet<String> = BTreeSet::new();
    let tables = config(feed)
        .and_then(|config| config.get("profiles"))
        .and_then(Value::as_object);
    for profile in tables.into_iter().flat_map(|tables| tables.values()) {
        named.extend(text(profile, "effort").map(str::to_owned));
    }
    for (_, _, effort) in profiles(feed) {
        named.extend(effort);
    }
    let order = ["minimal", "low", "medium", "high", "xhigh", "max"];
    let mut levels: Vec<String> = order
        .iter()
        .filter(|level| named.contains(**level))
        .map(|level| (*level).to_owned())
        .collect();
    levels.extend(
        named
            .into_iter()
            .filter(|level| !order.contains(&level.as_str())),
    );
    levels
}

fn choice(id: String, label: impl Into<String>, selected: bool, action: StartAction) -> Choice {
    Choice {
        id,
        label: label.into(),
        detail: None,
        selected,
        action: start(action),
    }
}

/// The sheet while it is open.
pub fn form(state: &Start, feed: &Feed, filter: &ProjectFilter) -> Option<NewBuildForm> {
    if !state.open {
        return None;
    }
    let chosen = project(state, feed, filter);
    let projects = ProjectField {
        label: "Project",
        choices: feed
            .projects
            .keys()
            .map(|id| {
                choice(
                    format!("seg-build-project-{id}"),
                    id.to_string(),
                    chosen.as_deref() == Some(id.as_str()),
                    StartAction::Project(id.to_string()),
                )
            })
            .collect(),
        missing: feed.projects.is_empty().then(|| NO_PROJECT.to_owned()),
    };
    let path = chosen
        .as_deref()
        .and_then(|id| feed.projects.get(id))
        .and_then(|row| row.effective.as_ref())
        .map(|settings| settings.path.display().to_string());
    let place = PlaceField {
        label: "Where",
        choices: [
            (Place::Worktree, "New worktree"),
            (Place::Checkout, "In the checkout"),
        ]
        .into_iter()
        .map(|(place, label)| {
            choice(
                format!("seg-build-place-{}", place.slug()),
                label,
                state.place == place,
                StartAction::Place(place),
            )
        })
        .collect(),
        explain: match state.place {
            Place::Worktree => {
                "work on a new branch in its own worktree; the checkout is untouched".into()
            }
            Place::Checkout => match path {
                Some(path) => format!("work directly in {path} on its current branch"),
                None => "work directly in the project's checkout on its current branch".into(),
            },
        },
    };
    let mut model_choices = vec![choice(
        "seg-build-model-default".into(),
        "Project default",
        state.model == ModelPick::Route,
        StartAction::Model(ModelPick::Route),
    )];
    model_choices.extend(profiles(feed).into_iter().map(|(name, model, _)| {
        let mut found = choice(
            format!("seg-build-model-profile-{name}"),
            format!("{name} profile"),
            state.model == ModelPick::Profile(name.clone()),
            StartAction::Model(ModelPick::Profile(name)),
        );
        found.detail = model;
        found
    }));
    model_choices.extend(models(feed).into_iter().map(|model| {
        choice(
            format!("seg-build-model-{model}"),
            model.clone(),
            state.model == ModelPick::Model(model.clone()),
            StartAction::Model(ModelPick::Model(model)),
        )
    }));
    let levels = efforts(feed);
    let effort = (!levels.is_empty()).then(|| EffortField {
        label: "Effort",
        choices: std::iter::once(choice(
            "seg-build-effort-default".into(),
            "default",
            state.effort.is_none(),
            StartAction::Effort(None),
        ))
        .chain(levels.into_iter().map(|level| {
            choice(
                format!("seg-build-effort-{level}"),
                level.clone(),
                state.effort.as_deref() == Some(level.as_str()),
                StartAction::Effort(Some(level)),
            )
        }))
        .collect(),
    });
    let unavailable = if feed.projects.is_empty() {
        Some(NO_PROJECT.to_owned())
    } else if chosen.is_none() {
        Some("Choose the project to build in.".to_owned())
    } else {
        None
    };
    Some(NewBuildForm {
        title: "New build session",
        project: projects,
        name: NameField {
            id: "field-build-name",
            label: "Name",
            value: state.name.clone(),
            hint: "Leave it empty and the host names the build.",
        },
        place,
        model: ModelField {
            label: "Model",
            id: "field-build-model",
            typed: match &state.model {
                ModelPick::Model(model) => model.clone(),
                _ => String::new(),
            },
            choices: model_choices,
        },
        effort,
        start: StartButton {
            id: "btn-start-build",
            label: if state.sending.is_some() {
                "Starting…"
            } else {
                "Start build"
            },
            unavailable,
            action: start(StartAction::Submit),
        },
        cancel: start(StartAction::Close),
        error: state.failed.clone(),
    })
}

impl Start {
    /// Applies a sheet action; a start goes out once, under a new key, as `build.start`.
    pub fn act(
        &mut self,
        action: StartAction,
        ids: &mut IdSupply,
        feed: &Feed,
        filter: &ProjectFilter,
    ) -> Vec<Effect> {
        match action {
            StartAction::Open => self.open = true,
            StartAction::Close => {
                if self.sending.is_none() {
                    *self = Self::default();
                }
            }
            StartAction::Project(id) => self.project = Some(id),
            StartAction::Name(name) => self.name = name,
            StartAction::Place(place) => self.place = place,
            StartAction::Model(model) => self.model = model,
            StartAction::Effort(effort) => self.effort = effort,
            StartAction::Submit => {
                let Some(project) = project(self, feed, filter) else {
                    return Vec::new();
                };
                if self.sending.is_some() {
                    return Vec::new();
                }
                let key = ids.next_id();
                self.sending = Some(key.clone());
                self.failed = None;
                let (profile, model) = match &self.model {
                    ModelPick::Route => (None, None),
                    ModelPick::Profile(name) => (Some(name.clone()), None),
                    ModelPick::Model(model) => (None, Some(model.trim().to_owned())),
                };
                let name = self.name.trim();
                return vec![Effect::BuildStart {
                    key,
                    start: BuildStart {
                        project: Some(project),
                        name: (!name.is_empty()).then(|| name.to_owned()),
                        here: self.place == Place::Checkout,
                        model: model.filter(|model| !model.is_empty()),
                        effort: self.effort.clone(),
                        fallback: Vec::new(),
                        profile,
                    },
                }];
            }
        }
        Vec::new()
    }

    /// Follows the start in flight: a started build opens its page and the sheet closes; a
    /// refusal opens the refusal sheet with the host's lines and the fields stay.
    pub fn settle(&mut self, feed: &Feed) -> Vec<Effect> {
        let Some(key) = self.sending.clone() else {
            return Vec::new();
        };
        let Some(operation) = feed
            .operations
            .get(key.as_str())
            .filter(|operation| !operation.loading)
        else {
            return Vec::new();
        };
        self.sending = None;
        if let Some(refusal) = &operation.refusal {
            return vec![Effect::Refused(RefusalSheet {
                title: "Start build".into(),
                refusal: refusal.clone(),
            })];
        }
        if let Some(error) = &operation.error {
            self.failed = Some(if error.code == "deadline_expired" {
                NO_ANSWER.to_owned()
            } else {
                format!("{} ({})", error.message, error.code)
            });
            return Vec::new();
        }
        match operation.decode::<AssistantRow>() {
            Some(row) => {
                *self = Self::default();
                vec![Effect::OpenLive(Live::Session(Id::from(row.id.as_str())))]
            }
            None => {
                self.failed = Some("corrald's answer could not be read.".into());
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::daemon::{ClientError, Operation};

    /// A sheet whose start settled with `code`, the fields typed before it was sent.
    fn settled_with(code: &str) -> Start {
        let mut start = Start {
            open: true,
            name: "login-fix".into(),
            model: ModelPick::Model("anthropic/opus".into()),
            sending: Some(Id::from("op-1")),
            ..Start::default()
        };
        let mut feed = Feed::default();
        feed.operations.insert(
            Id::from("op-1"),
            Operation {
                method: "build.start".into(),
                loading: false,
                value: None,
                error: Some(ClientError {
                    code: code.into(),
                    message: "daemon request failed".into(),
                }),
                refusal: None,
            },
        );
        assert!(start.settle(&feed).is_empty());
        start
    }

    #[test]
    fn a_start_that_got_no_answer_says_the_build_may_exist() {
        let expired = settled_with("deadline_expired");
        let refused = settled_with("unavailable");
        assert!(expired.sending.is_none());
        assert!(expired.open);
        assert_eq!(expired.name, "login-fix");
        assert_eq!(expired.model, ModelPick::Model("anthropic/opus".into()));
        let line = expired.failed.expect("an expired start leaves a line");
        assert!(!line.contains("deadline_expired"));
        assert_ne!(Some(&line), refused.failed.as_ref());
    }
}
