//! Settings › Autopilot & grants: the operating-mode commands and the finite autopilot grant.
//! Every change is an owner-only canonical `mode.*` command confirmed on this device against
//! corrald's exact preview; a grant names its scope explicitly (corral never grants "all").

use corral_protocol::cmd::Command;
use corral_protocol::cmd::mode::ModeCommand;
use corral_protocol::rows::{OperatingMode, SettingsSnapshot};

use super::live::grant_on;
use crate::model::Act;
use crate::model::hue::Hue;
use crate::model::id::IdSupply;

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

    pub fn power(&self, power: Power) -> bool {
        self.powers[power as usize]
    }

    /// Why the grant cannot be previewed yet; None when it can.
    pub fn problem(&self) -> Option<&'static str> {
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

    /// The grant as a confirmed canonical command: corrald previews the exact scope, and the
    /// confirmation carries its automation preview identity.
    pub fn act(&self, ids: &mut IdSupply) -> Option<Act> {
        if self.problem().is_some() {
            return None;
        }
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
        Some(Act::new(ids, "Grant autopilot", Command::Mode(command)).confirmed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_without_scope_is_never_built() {
        let mut form = GrantForm::default();
        form.powers[Power::Dispatch as usize] = true;
        assert!(form.act(&mut IdSupply::new("grant-test")).is_none());
        form.set(Field::Tasks, "T-1, T-2".into());
        let act = form.act(&mut IdSupply::new("grant-test")).unwrap();
        let Command::Mode(ModeCommand::Autopilot { tasks, confirm, .. }) = act.command else {
            panic!("autopilot command expected");
        };
        assert_eq!(tasks, ["T-1", "T-2"]);
        assert!(confirm.is_empty());
    }
}
