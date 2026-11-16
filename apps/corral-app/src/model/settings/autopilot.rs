//! Settings › Autopilot & grants: the operating-mode commands and the finite autopilot grant.
//! Every change is an owner-only canonical `mode.*` command confirmed on this device against
//! corrald's exact preview; a grant names its scope explicitly (corral never grants "all").

use corral_protocol::cmd::Command;
use corral_protocol::cmd::mode::ModeCommand;
use corral_protocol::rows::{OperatingMode, SettingsSnapshot};

use super::live::grant_on;
use crate::model::daemon::Feed;
use crate::model::hue::Hue;
use crate::model::id::IdSupply;
use crate::model::projects::known_projects;
use crate::model::{Act, Id};

/// One operating-mode change the owner may make from here. corrald's `mode off` is only an alias
/// of pause, so it is not offered separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModeVerb {
    Pause,
    Resume,
    TurboOn,
    TurboOff,
    Assist,
}

impl ModeVerb {
    pub fn label(self) -> &'static str {
        match self {
            ModeVerb::Pause => "Pause automation",
            ModeVerb::Resume => "Resume automation",
            ModeVerb::TurboOn => "Turn turbo on",
            ModeVerb::TurboOff => "Turn turbo off",
            ModeVerb::Assist => "Return to Assist",
        }
    }

    /// What the change does, in one line under the buttons.
    pub fn meaning(self) -> &'static str {
        match self {
            ModeVerb::Pause => {
                "pauses the Autopilot grant's new actions; running agents continue and dispatch.paused is unchanged"
            }
            ModeVerb::Resume => {
                "resumes the existing Autopilot grant without widening it; dispatch.paused is unchanged"
            }
            ModeVerb::TurboOn => "lands, accepts plans and repairs without owner stops",
            ModeVerb::TurboOff => "brings back owner stops and round limits",
            ModeVerb::Assist => {
                "withdraws the Autopilot grant; running work continues and every new action is yours"
            }
        }
    }

    pub fn hue(self) -> Hue {
        match self {
            ModeVerb::TurboOn => Hue::Held,
            ModeVerb::Assist | ModeVerb::Pause => Hue::Fail,
            ModeVerb::Resume | ModeVerb::TurboOff => Hue::Neutral,
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            ModeVerb::Pause => "pause",
            ModeVerb::Resume => "resume",
            ModeVerb::TurboOn => "turbo-on",
            ModeVerb::TurboOff => "turbo-off",
            ModeVerb::Assist => "assist",
        }
    }

    /// The confirmed canonical command.
    pub fn act(self, ids: &mut IdSupply) -> Act {
        let command = match self {
            ModeVerb::Pause => ModeCommand::Pause {},
            ModeVerb::Resume => ModeCommand::Resume {},
            ModeVerb::TurboOn => ModeCommand::Turbo { off: false },
            ModeVerb::TurboOff => ModeCommand::Turbo { off: true },
            ModeVerb::Assist => ModeCommand::Assist {},
        };
        Act::new(ids, self.label(), Command::Mode(command)).confirmed()
    }
}

/// The changes that apply to the current mode and grants. Assist has nothing to switch; a grant
/// is written below. Pause and turbo only exist under an Autopilot grant. A mode this app does
/// not know may still be narrowed back to Assist, the one change whose meaning is fixed.
pub fn verbs(settings: &SettingsSnapshot) -> Vec<ModeVerb> {
    match settings.mode {
        OperatingMode::Assist => Vec::new(),
        OperatingMode::Autopilot => vec![
            if grant_on(settings, "automation.paused") {
                ModeVerb::Resume
            } else {
                ModeVerb::Pause
            },
            if grant_on(settings, "automation.turbo") {
                ModeVerb::TurboOff
            } else {
                ModeVerb::TurboOn
            },
            ModeVerb::Assist,
        ],
        OperatingMode::Other => vec![ModeVerb::Assist],
    }
}

/// What autopilot may do under a grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Power {
    Dispatch,
    Accept,
    PlanAccept,
    AggregateAccept,
}

impl Power {
    pub const ALL: [Power; 4] = [
        Power::Dispatch,
        Power::Accept,
        Power::PlanAccept,
        Power::AggregateAccept,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Power::Dispatch => "Dispatch ready tasks",
            Power::Accept => "Accept reviewed work",
            Power::PlanAccept => "Accept plans",
            Power::AggregateAccept => "Accept aggregate results",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Power::Dispatch => "dispatch",
            Power::Accept => "accept",
            Power::PlanAccept => "plan-accept",
            Power::AggregateAccept => "aggregate-accept",
        }
    }
}

/// A text field of the grant form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    Projects,
    Tasks,
    Plans,
    Exclusions,
    ExpiresDays,
    MaxNew,
}

/// Project scope input: known configuration keys, or text only when no list is available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectPicker {
    Known(Vec<ProjectOption>),
    FreeText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectOption {
    pub id: Id,
    pub selected: bool,
}

/// The grant being written, before corrald previews it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GrantForm {
    pub projects: String,
    pub tasks: String,
    pub plans: String,
    pub exclusions: String,
    pub expires_days: String,
    pub max_new: String,
    pub include_new: bool,
    pub powers: [bool; 4],
}

fn list(text: &str) -> Vec<String> {
    text.split([',', ' ', '\n'])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

fn count(text: &str) -> Result<Option<u32>, ()> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    text.parse().map(Some).map_err(|_| ())
}

impl GrantForm {
    pub fn field(&self, field: Field) -> &str {
        match field {
            Field::Projects => &self.projects,
            Field::Tasks => &self.tasks,
            Field::Plans => &self.plans,
            Field::Exclusions => &self.exclusions,
            Field::ExpiresDays => &self.expires_days,
            Field::MaxNew => &self.max_new,
        }
    }

    pub fn set(&mut self, field: Field, value: String) {
        *match field {
            Field::Projects => &mut self.projects,
            Field::Tasks => &mut self.tasks,
            Field::Plans => &mut self.plans,
            Field::Exclusions => &mut self.exclusions,
            Field::ExpiresDays => &mut self.expires_days,
            Field::MaxNew => &mut self.max_new,
        } = value;
    }

    pub fn project_picker(&self, feed: &Feed) -> ProjectPicker {
        let known = known_projects(feed);
        if known.is_empty() {
            return ProjectPicker::FreeText;
        }
        let selected = list(&self.projects);
        ProjectPicker::Known(
            known
                .into_iter()
                .map(|id| ProjectOption {
                    selected: selected.iter().any(|project| project == id.as_str()),
                    id,
                })
                .collect(),
        )
    }

    /// Toggle only a known project; retire stale text scopes when switching to the picker.
    pub fn toggle_project(&mut self, feed: &Feed, project: Id) {
        let known = known_projects(feed);
        if !known.contains(&project) {
            return;
        }
        let mut selected = list(&self.projects);
        selected.retain(|project| known.iter().any(|id| id.as_str() == project));
        if selected.iter().any(|id| id == project.as_str()) {
            selected.retain(|id| id != project.as_str());
        } else {
            selected.push(project.to_string());
        }
        selected.sort_unstable();
        selected.dedup();
        self.projects = selected.join(", ");
    }

    /// Feed-aware validation prevents a stale free-text scope bypassing the known list.
    pub fn problem_for(&self, feed: &Feed) -> Option<&'static str> {
        if let Some(problem) = self.problem() {
            return Some(problem);
        }
        let known = known_projects(feed);
        if !known.is_empty()
            && list(&self.projects)
                .iter()
                .any(|project| !known.iter().any(|id| id.as_str() == project))
        {
            return Some("Choose projects from the current project list.");
        }
        None
    }

    pub fn act_for(&self, feed: &Feed, ids: &mut IdSupply) -> Option<Act> {
        if self.problem_for(feed).is_some() {
            return None;
        }
        Some(self.grant(ids))
    }

    pub fn power(&self, power: Power) -> bool {
        self.powers[power as usize]
    }

    /// Why the grant cannot be previewed yet, before the project list is consulted.
    fn problem(&self) -> Option<&'static str> {
        if list(&self.projects).is_empty()
            && list(&self.tasks).is_empty()
            && list(&self.plans).is_empty()
        {
            return Some("Name at least one project, task or plan; corral never grants all work.");
        }
        if !self.powers.iter().any(|on| *on) {
            return Some("Choose at least one thing autopilot may do.");
        }
        if count(&self.expires_days).is_err() || count(&self.max_new).is_err() {
            return Some("Expiry and the new-task limit are whole numbers.");
        }
        None
    }

    fn grant(&self, ids: &mut IdSupply) -> Act {
        let command = ModeCommand::Autopilot {
            projects: list(&self.projects),
            include_new: self.include_new,
            expires_after_days: count(&self.expires_days).ok().flatten(),
            max_new_tasks: count(&self.max_new).ok().flatten(),
            tasks: list(&self.tasks),
            exclusions: list(&self.exclusions),
            plan_scopes: list(&self.plans),
            dispatch: self.power(Power::Dispatch),
            accept: self.power(Power::Accept),
            plan_accept: self.power(Power::PlanAccept),
            aggregate_accept: self.power(Power::AggregateAccept),
            confirm: String::new(),
            json: false,
        };
        Act::new(ids, "Grant autopilot", Command::Mode(command)).confirmed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_without_scope_is_never_built() {
        let mut form = GrantForm::default();
        form.powers[Power::Dispatch as usize] = true;
        let feed = Feed::default();
        assert!(
            form.act_for(&feed, &mut IdSupply::new("grant-test"))
                .is_none()
        );
        form.set(Field::Tasks, "T-1, T-2".into());
        let act = form
            .act_for(&feed, &mut IdSupply::new("grant-test"))
            .unwrap();
        let Command::Mode(ModeCommand::Autopilot { tasks, confirm, .. }) = act.command else {
            panic!("autopilot command expected");
        };
        assert_eq!(tasks, ["T-1", "T-2"]);
        assert!(confirm.is_empty());
    }

    #[test]
    fn known_project_picker_limits_grants_and_keeps_empty_projects_selectable() {
        use corral_protocol::rows::ProjectRow;

        let mut feed = Feed::default();
        feed.capabilities
            .insert(corral_protocol::PROJECTS_V1.into());
        for id in ["corral", "mitiplay"] {
            feed.projects.insert(
                id.into(),
                ProjectRow {
                    id: id.into(),
                    path: id.into(),
                    open: 0,
                    running: 0,
                    review: 0,
                    needs_me: 0,
                    in_pm_scope: false,
                    autopilot: false,
                    rev: 1,
                },
            );
        }
        let mut form = GrantForm {
            projects: "unknown".into(),
            ..GrantForm::default()
        };
        form.powers[Power::Dispatch as usize] = true;
        assert!(
            form.act_for(&feed, &mut IdSupply::new("grant-test"))
                .is_none()
        );
        form.toggle_project(&feed, "mitiplay".into());
        form.toggle_project(&feed, "corral".into());
        assert_eq!(form.projects, "corral, mitiplay");
        let ProjectPicker::Known(options) = form.project_picker(&feed) else {
            panic!("known project picker expected");
        };
        assert_eq!(options.len(), 2);
        assert!(options.iter().all(|option| option.selected));
        let act = form
            .act_for(&feed, &mut IdSupply::new("grant-test"))
            .unwrap();
        let Command::Mode(ModeCommand::Autopilot { projects, .. }) = act.command else {
            panic!("autopilot command expected");
        };
        assert_eq!(projects, ["corral", "mitiplay"]);
        form.toggle_project(&feed, "mitiplay".into());
        assert_eq!(form.projects, "corral");

        feed.projects.clear();
        assert_eq!(form.project_picker(&feed), ProjectPicker::FreeText);
        form.set(Field::Projects, "external-project".into());
        assert!(
            form.act_for(&feed, &mut IdSupply::new("grant-test"))
                .is_some()
        );
    }
}
