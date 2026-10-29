//! Resolved settings for the worker's optional argv hooks and the bash allow/ask/deny rules
//! (T-572). Rust validates and resolves them once per generation; the host only parses them.
//! Hooks are trusted same-user programs from Corral configuration, not a containment boundary.
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{CapabilitySettings, CapabilitySnapshot};

pub const HOOKS_PER_PHASE_MAX: usize = 16;
pub const BASH_RULES_MAX: usize = 128;
const ARGV_MAX: usize = 64;
const ARG_MAX_BYTES: usize = 4096;
const TOOLS_MAX: usize = 64;
const PATTERN_MAX_BYTES: usize = 1024;
const HOOK_TIMEOUT_MAX_MS: u32 = 600_000;

/// One configured hook: a program and its arguments, run without a shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookCommand {
    pub name: String,
    pub argv: Vec<String>,
    /// Pre/post-tool hooks run around these local tools; empty means every eligible tool.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
}

/// `[agent.capabilities.worker.hooks]`: pre-tool, post-tool and turn-end hooks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HookSettings {
    pub timeout_ms: u32,
    pub pre_tool: Vec<HookCommand>,
    pub post_tool: Vec<HookCommand>,
    pub turn_end: Vec<HookCommand>,
}

impl Default for HookSettings {
    fn default() -> Self {
        Self {
            timeout_ms: 60_000,
            pre_tool: Vec::new(),
            post_tool: Vec::new(),
            turn_end: Vec::new(),
        }
    }
}

fn hook_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

fn tool_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.as_bytes()[0].is_ascii_lowercase()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

impl HookSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.timeout_ms == 0 || self.timeout_ms > HOOK_TIMEOUT_MAX_MS {
            return Err(format!(
                "hooks.timeout_ms must be between 1 and {HOOK_TIMEOUT_MAX_MS}"
            ));
        }
        for (phase, hooks) in self.phases() {
            if hooks.len() > HOOKS_PER_PHASE_MAX {
                return Err(format!(
                    "hooks.{phase} lists more than {HOOKS_PER_PHASE_MAX} hooks"
                ));
            }
            for (at, hook) in hooks.iter().enumerate() {
                if !hook_name(&hook.name) {
                    return Err(format!(
                        "hooks.{phase} name {:?} must be 1-64 characters of [A-Za-z0-9._-]",
                        hook.name
                    ));
                }
                if hooks[..at].iter().any(|other| other.name == hook.name) {
                    return Err(format!("hooks.{phase} names {} twice", hook.name));
                }
                if hook.argv.is_empty() || hook.argv.len() > ARGV_MAX {
                    return Err(format!(
                        "hooks.{phase}.{} argv must have 1-{ARGV_MAX} entries",
                        hook.name
                    ));
                }
                if hook.argv[0].is_empty()
                    || hook
                        .argv
                        .iter()
                        .any(|arg| arg.len() > ARG_MAX_BYTES || arg.contains('\0'))
                {
                    return Err(format!(
                        "hooks.{phase}.{} argv has an empty program or an invalid argument",
                        hook.name
                    ));
                }
                if phase == "turn_end" && !hook.tools.is_empty() {
                    return Err(format!(
                        "hooks.turn_end.{} cannot name tools; it runs once per turn",
                        hook.name
                    ));
                }
                if hook.tools.len() > TOOLS_MAX || hook.tools.iter().any(|tool| !tool_name(tool)) {
                    return Err(format!(
                        "hooks.{phase}.{} tools must be at most {TOOLS_MAX} lowercase tool names",
                        hook.name
                    ));
                }
                if hook.tools.iter().any(|tool| tool.starts_with("corral_")) {
                    return Err(format!(
                        "hooks.{phase}.{} cannot wrap corral operations",
                        hook.name
                    ));
                }
            }
        }
        Ok(())
    }

    fn phases(&self) -> [(&'static str, &[HookCommand]); 3] {
        [
            ("pre_tool", &self.pre_tool),
            ("post_tool", &self.post_tool),
            ("turn_end", &self.turn_end),
        ]
    }

    /// Whether a configured pre- or post-tool hook can run around a call of `tool`.
    /// Corral operations are never wrapped.
    pub fn wraps(&self, tool: &str) -> bool {
        !tool.starts_with("corral_")
            && self
                .pre_tool
                .iter()
                .chain(&self.post_tool)
                .any(|hook| hook.tools.is_empty() || hook.tools.iter().any(|t| t == tool))
    }

    /// The enabled hook settings a generation snapshot recorded, if any.
    pub fn of(snapshot: &CapabilitySnapshot) -> Option<Self> {
        let state = snapshot
            .features
            .get("hooks")
            .filter(|state| state.enabled)?;
        serde_json::from_value(Value::Object(state.settings.clone().into_iter().collect())).ok()
    }

    pub fn summary(&self) -> String {
        let names = |hooks: &[HookCommand]| {
            let names: Vec<&str> = hooks.iter().map(|hook| hook.name.as_str()).collect();
            format!("[{}]", names.join(", "))
        };
        format!(
            "hooks pre_tool={}, post_tool={}, turn_end={}, timeout_ms={}",
            names(&self.pre_tool),
            names(&self.post_tool),
            names(&self.turn_end),
            self.timeout_ms
        )
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BashRuleAction {
    #[default]
    Allow,
    Ask,
    Deny,
}

impl BashRuleAction {
    pub fn as_str(self) -> &'static str {
        match self {
            BashRuleAction::Allow => "allow",
            BashRuleAction::Ask => "ask",
            BashRuleAction::Deny => "deny",
        }
    }
}

/// `[agent.capabilities.<role>.bash_rules]`: command globs. Deny wins over ask, ask over allow;
/// `default` decides a command no rule matches.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BashRuleSettings {
    pub allow: Vec<String>,
    pub ask: Vec<String>,
    pub deny: Vec<String>,
    pub default: BashRuleAction,
}

impl BashRuleSettings {
    pub fn validate(&self) -> Result<(), String> {
        for (list, patterns) in [
            ("allow", &self.allow),
            ("ask", &self.ask),
            ("deny", &self.deny),
        ] {
            if patterns.len() > BASH_RULES_MAX {
                return Err(format!(
                    "bash_rules.{list} lists more than {BASH_RULES_MAX} patterns"
                ));
            }
            if patterns.iter().any(|pattern| {
                pattern.trim().is_empty()
                    || pattern.len() > PATTERN_MAX_BYTES
                    || pattern.contains('\0')
            }) {
                return Err(format!(
                    "bash_rules.{list} patterns must be 1-{PATTERN_MAX_BYTES} bytes and not blank"
                ));
            }
        }
        Ok(())
    }

    pub fn summary(&self) -> String {
        format!(
            "bash_rules deny={}, ask={}, allow={}, default={}",
            self.deny.len(),
            self.ask.len(),
            self.allow.len(),
            self.default.as_str()
        )
    }
}

fn effective<T: Serialize + for<'de> Deserialize<'de>>(
    table: &CapabilitySettings,
    validate: fn(&T) -> Result<(), String>,
) -> Result<CapabilitySettings, String> {
    let settings: T = serde_json::from_value(Value::Object(table.clone().into_iter().collect()))
        .map_err(|error| format!("is invalid: {error}"))?;
    validate(&settings)?;
    match serde_json::to_value(&settings) {
        Ok(Value::Object(map)) => Ok(map.into_iter().collect()),
        _ => Err("could not be recorded".into()),
    }
}

/// `[agent.capabilities.worker.hooks]` settings with every default filled in.
pub(crate) fn hook_settings(table: &CapabilitySettings) -> Result<CapabilitySettings, String> {
    effective(table, HookSettings::validate)
}

/// `[agent.capabilities.<role>.bash_rules]` settings with every default filled in.
pub(crate) fn bash_rule_settings(table: &CapabilitySettings) -> Result<CapabilitySettings, String> {
    effective(table, BashRuleSettings::validate)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(name: &str, argv: &[&str], tools: &[&str]) -> HookCommand {
        HookCommand {
            name: name.into(),
            argv: argv.iter().map(|s| (*s).into()).collect(),
            tools: tools.iter().map(|s| (*s).into()).collect(),
        }
    }

    #[test]
    fn hooks_wrap_only_named_or_all_local_tools_and_never_corral() {
        // Given a tool-scoped pre hook and an unscoped post hook.
        let scoped = HookSettings {
            pre_tool: vec![hook("lint", &["lint"], &["edit"])],
            ..HookSettings::default()
        };
        let all = HookSettings {
            post_tool: vec![hook("audit", &["audit"], &[])],
            ..HookSettings::default()
        };
        // Then scoping is honored and corral operations are never wrapped.
        assert!(scoped.wraps("edit") && !scoped.wraps("read"));
        assert!(all.wraps("read") && all.wraps("bash"));
        assert!(!all.wraps("corral_done"));
        assert!(!HookSettings::default().wraps("read"));
    }

    #[test]
    fn invalid_hook_and_rule_settings_are_refused() {
        // Given settings that break each bound.
        let bad_hooks = [
            HookSettings {
                timeout_ms: 0,
                ..HookSettings::default()
            },
            HookSettings {
                pre_tool: vec![hook("x", &[], &[])],
                ..HookSettings::default()
            },
            HookSettings {
                pre_tool: vec![hook("bad name", &["x"], &[])],
                ..HookSettings::default()
            },
            HookSettings {
                pre_tool: vec![hook("x", &["x"], &[]), hook("x", &["y"], &[])],
                ..HookSettings::default()
            },
            HookSettings {
                post_tool: vec![hook("x", &["x"], &["corral_done"])],
                ..HookSettings::default()
            },
            HookSettings {
                turn_end: vec![hook("x", &["x"], &["read"])],
                ..HookSettings::default()
            },
        ];
        // Then each is refused, while the defaults and a valid hook pass.
        for settings in bad_hooks {
            assert!(settings.validate().is_err(), "{settings:?}");
        }
        HookSettings::default().validate().unwrap();
        let rules = BashRuleSettings {
            deny: vec!["  ".into()],
            ..BashRuleSettings::default()
        };
        assert!(rules.validate().is_err());
        BashRuleSettings::default().validate().unwrap();
    }
}
