//! Configured profiles as the owner manages them in a daemon session: each profile's harness,
//! model and effort, where it comes from, what still uses it and whether corrald would let it
//! go; and the profile being added, always as a copy of one corrald already has configured.
//! Adding and removing are owner-only, hash-guarded `config.profile-*` commands that corrald
//! previews for a fresh confirmation on this device; it re-checks names and uses on commit.

use corral_protocol::cmd::Command;
use corral_protocol::cmd::config::ConfigCommand;
use corral_protocol::rows::{ProfileChoice, SettingsSnapshot};

use crate::model::Act;

/// One configured profile, with what the owner may do to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedProfile {
    pub name: String,
    /// Harness, model and effort as configured: "corral-pi · gpt-6.1-sol · high".
    pub setup: String,
    /// The model as configured, the draft an edit starts from; empty when none is set.
    pub model: String,
    /// Where the profile is configured, when corrald says.
    pub origin: Option<&'static str>,
    /// Why it cannot be removed now, or what uses it; None when nothing needs saying.
    pub held: Option<String>,
    /// The removal corrald would accept, worded for what it removes; None when refused.
    pub remove: Option<&'static str>,
}

fn setup(choice: &ProfileChoice) -> String {
    let mut parts = vec![choice.harness.as_str()];
    parts.push(choice.model.as_deref().unwrap_or("no model set"));
    if let Some(effort) = &choice.effort {
        parts.push(effort);
    }
    parts.join(" · ")
}

/// Every configured profile corrald reports, in its order.
pub fn managed(settings: &SettingsSnapshot) -> Vec<ManagedProfile> {
    settings
        .profiles
        .iter()
        .map(|choice| ManagedProfile {
            name: choice.name.clone(),
            setup: setup(choice),
            model: choice.model.clone().unwrap_or_default(),
            origin: match (choice.workspace, choice.inherited) {
                (true, false) => Some("this workspace"),
                (true, true) => Some("this workspace, over an inherited profile"),
                (false, true) => Some("inherited · not in this workspace's configuration"),
                (false, false) => None,
            },
            held: match (&choice.remove_refusal, choice.references.is_empty()) {
                (Some(refusal), _) => Some(format!("Can't remove: {refusal}")),
                (None, false) => Some(format!("Used by {}", choice.references.join(", "))),
                (None, true) => None,
            },
            remove: choice
                .remove_refusal
                .is_none()
                .then_some(if choice.inherited {
                    "Remove override"
                } else {
                    "Remove"
                }),
        })
        .collect()
}

/// The profile a new one copies by default: the assistant profile, else the first configured.
pub fn default_source(settings: &SettingsSnapshot) -> String {
    settings
        .assistant_profile
        .as_ref()
        .filter(|name| settings.profiles.iter().any(|p| &p.name == *name))
        .or_else(|| settings.profiles.first().map(|p| &p.name))
        .cloned()
        .unwrap_or_default()
}

/// A text field of the profile being added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileField {
    Name,
    Model,
    Effort,
}

impl ProfileField {
    pub const fn slug(self) -> &'static str {
        match self {
            ProfileField::Name => "name",
            ProfileField::Model => "model",
            ProfileField::Effort => "effort",
        }
    }
}

/// The profile being added: the configured profile it copies, its new name, and the model and
/// effort it changes (blank keeps the copied one).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProfileForm {
    pub from: String,
    pub name: String,
    pub model: String,
    pub effort: String,
}

fn optional(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

impl ProfileForm {
    pub fn field(&self, field: ProfileField) -> &str {
        match field {
            ProfileField::Name => &self.name,
            ProfileField::Model => &self.model,
            ProfileField::Effort => &self.effort,
        }
    }

    pub fn set(&mut self, field: ProfileField, value: String) {
        match field {
            ProfileField::Name => self.name = value,
            ProfileField::Model => self.model = value,
            ProfileField::Effort => self.effort = value,
        }
    }

    /// Why the form cannot be sent yet, as far as this device can tell; corrald checks again.
    pub fn problem(&self, settings: &SettingsSnapshot) -> Option<String> {
        let name = self.name.trim();
        if !settings.profiles.iter().any(|p| p.name == self.from) {
            return Some("Choose a configured profile to copy.".into());
        }
        if name.is_empty() {
            return Some("Name the new profile.".into());
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Some("A profile name uses letters, digits, '-' and '_' only.".into());
        }
        if settings.profiles.iter().any(|p| p.name == name) {
            return Some(format!("A profile named {name} already exists."));
        }
        None
    }

    /// The hash-guarded, Confirm-class add, when the form can be sent.
    pub fn act(&self, settings: &SettingsSnapshot) -> Option<Act> {
        if self.problem(settings).is_some() {
            return None;
        }
        let name = self.name.trim().to_owned();
        let title = format!("Add profile {name}, a copy of {}", self.from);
        Some(
            Act::new(
                title,
                Command::Config(ConfigCommand::ProfileAdd {
                    name,
                    from: self.from.clone(),
                    model: optional(&self.model),
                    effort: optional(&self.effort),
                    expected_hash: settings.config_hash.clone(),
                }),
            )
            .confirmed(),
        )
    }
}

/// The hash-guarded, Confirm-class removal of `profile`'s workspace entry.
pub fn remove_act(settings: &SettingsSnapshot, profile: &str) -> Act {
    let inherited = settings
        .profiles
        .iter()
        .any(|p| p.name == profile && p.inherited);
    let title = if inherited {
        format!("Remove this workspace's override of {profile}")
    } else {
        format!("Remove profile {profile}")
    };
    Act::new(
        title,
        Command::Config(ConfigCommand::ProfileRemove {
            name: profile.to_owned(),
            expected_hash: settings.config_hash.clone(),
        }),
    )
    .confirmed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_protocol::rows::OperatingMode;

    fn choice(
        name: &str,
        workspace: bool,
        inherited: bool,
        refusal: Option<&str>,
    ) -> ProfileChoice {
        ProfileChoice {
            name: name.into(),
            harness: "corral-pi".into(),
            model: Some("gpt-6.1-sol".into()),
            effort: None,
            workspace,
            inherited,
            references: refusal.map(|_| vec!["pm.agent".into()]).unwrap_or_default(),
            remove_refusal: refusal.map(str::to_owned),
        }
    }

    fn settings() -> SettingsSnapshot {
        SettingsSnapshot {
            id: "settings".into(),
            workspace: "w".into(),
            store: "s".into(),
            host: "h".into(),
            mode: OperatingMode::Assist,
            grants: vec![],
            assistant_models: vec![],
            profiles: vec![
                choice("pi-sol", true, false, Some("pm.agent uses it")),
                choice("pi-luna", true, false, None),
                choice("pi-base", true, true, None),
            ],
            assistant_profile: Some("pi-luna".into()),
            summary_profile: None,
            listener: None,
            config_hash: "hash-1".into(),
            rev: 1,
        }
    }

    #[test]
    fn profile_add_copies_a_configured_profile_under_the_read_hash() {
        let settings = settings();
        let mut form = ProfileForm {
            from: default_source(&settings),
            ..ProfileForm::default()
        };
        assert_eq!(form.from, "pi-luna");
        assert!(form.act(&settings).is_none());
        form.set(ProfileField::Name, "pi-sol".into());
        assert!(form.problem(&settings).unwrap().contains("already exists"));
        form.set(ProfileField::Name, "bad name".into());
        assert!(form.act(&settings).is_none());
        form.set(ProfileField::Name, " pi-fast ".into());
        form.set(ProfileField::Effort, "low".into());
        let act = form.act(&settings).unwrap();
        assert!(act.confirm);
        assert_eq!(
            act.command,
            Command::Config(ConfigCommand::ProfileAdd {
                name: "pi-fast".into(),
                from: "pi-luna".into(),
                model: None,
                effort: Some("low".into()),
                expected_hash: "hash-1".into(),
            })
        );
    }

    #[test]
    fn removal_follows_corrald_refusal_and_inheritance() {
        let settings = settings();
        let rows = managed(&settings);
        assert_eq!(rows[0].remove, None);
        assert_eq!(
            rows[0].held.as_deref(),
            Some("Can't remove: pm.agent uses it")
        );
        assert_eq!(rows[1].remove, Some("Remove"));
        assert_eq!(rows[2].remove, Some("Remove override"));
        let act = remove_act(&settings, "pi-base");
        assert!(act.confirm && act.title.contains("override"));
    }
}
