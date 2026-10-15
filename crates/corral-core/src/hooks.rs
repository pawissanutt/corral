//! Opt-in project operations. Commands are argv, never shell snippets.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostLandHook {
    pub name: String,
    pub argv: Vec<String>,
    /// Relative to the project's base checkout; no parent traversal.
    pub cwd: String,
    /// Complete environment, in addition to corral's SHA/project/operation identifiers.
    pub env: BTreeMap<String, String>,
    pub timeout_secs: u32,
    /// Total attempts, including the initial attempt. No automatic retry.
    pub max_attempts: u32,
    /// Optional installed executable identity and prior-binary preservation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<HookIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookIdentity {
    /// Absolute, or relative to the hook cwd.
    pub binary: String,
    pub args: Vec<String>,
    /// Exact trimmed stdout. `{sha}` expands to the landed commit.
    pub expected_stdout: String,
}

pub(crate) fn problems(project: &str, hooks: &[PostLandHook]) -> Vec<String> {
    let mut out = Vec::new();
    let mut names = BTreeSet::new();
    for hook in hooks {
        let key = format!("projects.{project}.post_land.{}", hook.name);
        if hook.name.is_empty()
            || !hook
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            || !names.insert(&hook.name)
        {
            out.push(format!(
                "{key}: hook names must be unique and use letters, digits, - or _"
            ));
        }
        if hook.argv.is_empty() || hook.argv[0].is_empty() {
            out.push(format!("{key}.argv must name an executable"));
        }
        if hook.cwd.is_empty()
            || std::path::Path::new(&hook.cwd).components().any(|c| {
                !matches!(
                    c,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            })
        {
            out.push(format!(
                "{key}.cwd must be relative without parent traversal"
            ));
        }
        if hook.timeout_secs == 0 || hook.timeout_secs > 86_400 {
            out.push(format!("{key}.timeout_secs must be 1..86400"));
        }
        if !(1..=10).contains(&hook.max_attempts) {
            out.push(format!("{key}.max_attempts must be 1..10"));
        }
        if hook.env.iter().any(|(k, v)| {
            k.is_empty() || k.contains(['=', '\0']) || v.contains('\0') || k.starts_with("CORRAL_")
        }) || hook.argv.iter().any(|a| a.contains('\0'))
        {
            out.push(format!(
                "{key}: invalid argv/environment or reserved CORRAL_ key"
            ));
        }
        if let Some(identity) = &hook.identity
            && (identity.binary.is_empty()
                || identity.binary.contains('\0')
                || identity.args.iter().any(|a| a.contains('\0'))
                || !identity.expected_stdout.contains("{sha}"))
        {
            out.push(format!(
                "{key}.identity must name a binary and SHA-bound expected stdout containing {{sha}}"
            ));
        }
    }
    out
}
