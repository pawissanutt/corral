//! Rows of the Profiles and Harnesses sections: the profile list, one profile's editor, and each harness's
//! install state and command template. Harness detection (`HarnessReport`) orders and dims the harness pills
//! and supplies the model list; model flags and effort levels always come from the staged config.

use corral_core::{Config, FALLBACK_PROFILE, FlagMap, Profile, Status, Task};
use corral_ops::{HarnessInfo, HarnessReport, ModelSource};

use super::model::{Field, Kind, Row, opt};

const MODEL_COL_MAX: usize = 32;

/// Every harness a profile can name: `[harnesses]` plus legacy `[agents]` keys, sorted.
pub(crate) fn harness_keys(cfg: &Config) -> Vec<String> {
    let mut keys: Vec<String> = cfg
        .harnesses
        .keys()
        .chain(cfg.agents.keys())
        .cloned()
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

fn info<'a>(report: Option<&'a HarnessReport>, key: &str) -> Option<&'a HarnessInfo> {
    report.and_then(|r| r.get(key))
}

/// Harness keys for the pills, installed ones first (then undetected, then missing), and the missing ones.
pub(crate) fn harness_order(
    cfg: &Config,
    report: Option<&HarnessReport>,
) -> (Vec<String>, Vec<String>) {
    let rank = |k: &String| match info(report, k).map(HarnessInfo::installed) {
        Some(true) => 0,
        None => 1,
        Some(false) => 2,
    };
    let mut keys = harness_keys(cfg);
    keys.sort_by_key(rank);
    let missing = keys.iter().filter(|k| rank(k) == 2).cloned().collect();
    (keys, missing)
}

/// A fresh profile: the first installed harness (else the first configured one), no model, effort or args.
pub(crate) fn new_profile(cfg: &Config, report: Option<&HarnessReport>) -> Profile {
    let harness = harness_order(cfg, report)
        .0
        .into_iter()
        .next()
        .unwrap_or_else(|| "generic".into());
    Profile {
        harness,
        model: None,
        effort: None,
        args: Vec::new(),
        vendor: None,
        escalate_to: None,
        metadata: None,
    }
}

/// Adds profile `name` (a copy of `template`, else a fresh one). Names are letters, digits, `-` and `_`.
pub(crate) fn add_profile(
    cfg: &mut Config,
    name: &str,
    template: Option<Profile>,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("type a profile name".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("{name:?}: use letters, digits, '-' and '_' only"));
    }
    if cfg.profiles.contains_key(name) || cfg.agents.contains_key(name) {
        return Err(format!("profile {name} already exists"));
    }
    let profile = template.unwrap_or_else(|| new_profile(cfg, None));
    cfg.profiles.insert(name.to_string(), profile);
    Ok(())
}

/// The profile dispatch falls back to when neither task nor project names one.
pub(crate) fn default_profile(cfg: &Config) -> &str {
    cfg.defaults.profile.as_deref().unwrap_or(FALLBACK_PROFILE)
}

/// Where profile `name` is referenced: config keys, then open tasks (by id). Empty when it can be deleted.
pub(crate) fn profile_refs(cfg: &Config, tasks: &[Task], name: &str) -> Vec<String> {
    let mut out = Vec::new();
    match cfg.defaults.profile.as_deref() {
        Some(p) if p == name => out.push("defaults.profile".to_string()),
        None if name == FALLBACK_PROFILE => {
            out.push("the built-in default (defaults.profile is unset)".to_string())
        }
        _ => {}
    }
    if cfg.pm.agent == name {
        out.push("pm.agent".into());
    }
    if cfg.review.reviewer.as_deref() == Some(name) {
        out.push("review.reviewer".into());
    }
    for (k, p) in &cfg.projects {
        if p.default_profile.as_deref() == Some(name) {
            out.push(format!("projects.{k}.default_profile"));
        }
        if p.review.reviewer.as_deref() == Some(name) {
            out.push(format!("projects.{k}.review.reviewer"));
        }
    }
    for t in tasks {
        let open = !matches!(t.status, Status::Done | Status::Dropped);
        if open && t.profile.as_deref() == Some(name) {
            out.push(t.id.clone());
        }
    }
    out
}

fn default_mark(cfg: &Config, name: &str) -> &'static str {
    if default_profile(cfg) == name {
        "★ default"
    } else {
        ""
    }
}

struct Columns {
    name: usize,
    harness: usize,
    model: usize,
}

fn columns(cfg: &Config) -> Columns {
    let widest = |init: usize, it: &mut dyn Iterator<Item = usize>| it.fold(init, usize::max);
    Columns {
        name: widest(
            "profile".len(),
            &mut cfg.profiles.keys().map(|k| k.chars().count()),
        ),
        harness: widest(
            "harness".len(),
            &mut cfg.profiles.values().map(|p| p.harness.chars().count()),
        ),
        model: widest(
            "model".len(),
            &mut cfg
                .profiles
                .values()
                .filter_map(|p| p.model.as_ref().map(|m| m.chars().count())),
        )
        .min(MODEL_COL_MAX),
    }
}

/// The Profiles list's column titles, aligned with `profile_list` rows (after the 2-column marker).
pub(crate) fn profile_header(cfg: &Config) -> String {
    let c = columns(cfg);
    format!(
        "{:<n$}  {:<h$}  {:<m$}  effort",
        "profile",
        "harness",
        "model",
        n = c.name,
        h = c.harness,
        m = c.model
    )
}

/// One `Entry` row per `[profiles]` entry (harness, model, effort, default mark), then "+ new profile".
pub(crate) fn profile_list(cfg: &Config) -> Vec<Row> {
    let c = columns(cfg);
    let mut out: Vec<Row> = cfg
        .profiles
        .iter()
        .map(|(name, p)| {
            let model: String = p
                .model
                .as_deref()
                .unwrap_or("—")
                .chars()
                .take(c.model)
                .collect();
            let value = format!(
                "{:<h$}  {:<m$}  {:<7} {}",
                p.harness,
                model,
                p.effort.as_deref().unwrap_or("—"),
                default_mark(cfg, name),
                h = c.harness,
                m = c.model
            );
            Row::new(
                Field::Profile(name.clone()),
                format!("{name:<n$}", n = c.name),
                Kind::Entry,
                value.trim_end().to_string(),
            )
            .key(format!("profiles.{name}"))
        })
        .collect();
    out.push(Row::new(
        Field::AddProfile,
        "+ new profile",
        Kind::Add,
        String::new(),
    ));
    out
}

/// The editor of profile `name`: header (owns the profile's problems), harness pills, model, effort, args.
/// Empty when the profile does not exist.
pub(crate) fn editor_rows(cfg: &Config, name: &str, report: Option<&HarnessReport>) -> Vec<Row> {
    let Some(p) = cfg.profiles.get(name) else {
        return Vec::new();
    };
    let flags: Option<FlagMap> = cfg.harness(&p.harness).ok().map(|(_, f)| f);
    let (order, missing) = harness_order(cfg, report);
    let mut harnesses: Vec<Option<String>> = order.into_iter().map(Some).collect();
    if !harnesses.iter().any(|h| h.as_deref() == Some(&p.harness)) {
        harnesses.push(Some(p.harness.clone()));
    }
    let h = &p.harness;
    let model = match flags.as_ref().and_then(|f| f.model_flag.as_ref()) {
        None => Row::new(
            Field::ProfileModel(name.into()),
            "Model",
            Kind::Pick(vec![None]),
            String::new(),
        )
        .inherits(format!("{h} takes no model flag")),
        Some(_) => Row::new(
            Field::ProfileModel(name.into()),
            "Model",
            Kind::Picker,
            p.model.clone().unwrap_or_default(),
        )
        .help("⏎ opens the model list: type to filter; a name not in the list is accepted too.")
        .inherits("the harness's default model"),
    };
    let effort_value = p.effort.clone().unwrap_or_default();
    let effort = match flags.as_ref() {
        Some(f) if f.effort_flag.is_some() && f.efforts.is_empty() => Row::new(
            Field::ProfileEffort(name.into()),
            "Effort",
            Kind::Text { optional: true },
            effort_value,
        )
        .help("Passed to the harness's effort flag as typed.")
        .inherits("the harness's default effort"),
        Some(f) if f.effort_flag.is_some() => {
            let mut levels = vec![None];
            levels.extend(f.efforts.iter().cloned().map(Some));
            Row::new(
                Field::ProfileEffort(name.into()),
                "Effort",
                Kind::Pick(levels),
                effort_value,
            )
            .help("Levels this harness accepts; inherit passes no effort flag.")
            .inherits("the harness's default effort")
        }
        _ => Row::new(
            Field::ProfileEffort(name.into()),
            "Effort",
            Kind::Pick(vec![None]),
            String::new(),
        )
        .inherits(format!("{h} takes no effort flag")),
    };
    vec![
        Row::new(
            Field::Profile(name.into()),
            name,
            Kind::Header,
            default_mark(cfg, name).into(),
        )
        .key(format!("profiles.{name}")),
        Row::new(
            Field::ProfileHarness(name.into()),
            "Harness",
            Kind::Pick(harnesses),
            p.harness.clone(),
        )
        .help("Agent CLI this profile launches; installed ones first, struck-out ones were not found.")
        .muted(missing),
        model,
        effort,
        Row::new(
            Field::ProfileArgs(name.into()),
            "Extra args",
            Kind::Text { optional: true },
            p.args.join(" "),
        )
        .help("Appended after the model and effort flags (space-separated).")
        .inherits("none"),
    ]
}

fn source_label(s: ModelSource) -> &'static str {
    match s {
        ModelSource::Listed => "listed by the harness",
        ModelSource::Builtin => "built-in aliases",
        ModelSource::FreeText | ModelSource::None => "free text",
    }
}

fn flags_summary(cfg: &Config, key: &str, info: Option<&HarnessInfo>) -> String {
    let f = match cfg.harness(key) {
        Ok((_, f)) => f,
        Err(e) => return format!("{e:#}"),
    };
    let model = match (&f.model_flag, info) {
        (None, _) => "no model flag".to_string(),
        (Some(flag), Some(i)) if !i.models.is_empty() => format!(
            "{flag} <model> ({} {})",
            i.models.len(),
            source_label(i.model_source)
        ),
        (Some(flag), _) => format!("{flag} <model> (free text)"),
    };
    let effort = match &f.effort_flag {
        None => "no effort flag".to_string(),
        Some(flag) if f.efforts.is_empty() => format!("{flag} <any>"),
        Some(flag) => format!("{flag} {}", f.efforts.join("|")),
    };
    format!("{model} · {effort}")
}

fn status(report: Option<&HarnessReport>, info: Option<&HarnessInfo>) -> String {
    match (report, info) {
        (None, _) => "… detecting".into(),
        (Some(_), None) => "not detected yet (r re-detects)".into(),
        (Some(_), Some(i)) => match &i.path {
            Some(path) => format!(
                "✓ installed {}· {}",
                i.version
                    .as_deref()
                    .map(|v| format!("{v} "))
                    .unwrap_or_default(),
                path.display()
            ),
            None => format!(
                "✗ not installed ({})",
                i.program
                    .as_deref()
                    .map_or("no program in the command".into(), |p| format!(
                        "{p} not found"
                    ))
            ),
        },
    }
}

/// Per harness: header (install state), command template, model/effort flags, and probe notes if any.
pub(crate) fn harness_rows(cfg: &Config, report: Option<&HarnessReport>) -> Vec<Row> {
    let mut out = Vec::new();
    for key in harness_keys(cfg) {
        let i = info(report, &key);
        out.push(
            Row::new(
                Field::Harness(key.clone()),
                key.clone(),
                Kind::Header,
                status(report, i),
            )
            .key(format!("harnesses.{key}")),
        );
        let (command, table) = match cfg.harnesses.get(&key) {
            Some(h) => (h.command.clone(), "harnesses"),
            None => (
                cfg.agents
                    .get(&key)
                    .map(|a| a.command.clone())
                    .unwrap_or_default(),
                "agents",
            ),
        };
        out.push(
            Row::new(
                Field::HarnessCommand(key.clone()),
                "Command",
                Kind::Text { optional: false },
                command,
            )
            .help("Sent to the run's pane: {prompt_file} {name} {task} {worktree} {corral} {model_args}.")
            .key(format!("{table}.{key}.command")),
        );
        out.push(Row::new(
            Field::HarnessFlags(key.clone()),
            "Model & effort",
            Kind::Info,
            flags_summary(cfg, &key, i),
        ));
        if let Some(i) = i.filter(|i| !i.notes.is_empty()) {
            out.push(Row::new(
                Field::HarnessFlags(key.clone()),
                "Notes",
                Kind::Info,
                i.notes.join("; "),
            ));
        }
    }
    out
}

fn profile<'a>(cfg: &'a mut Config, name: &str) -> Result<&'a mut Profile, String> {
    cfg.profiles
        .get_mut(name)
        .ok_or_else(|| format!("profile {name} no longer exists"))
}

/// Applies an edit of a profile or harness field. Switching harness drops a model or effort the new harness
/// cannot take.
pub(crate) fn set_value(cfg: &mut Config, field: &Field, text: &str) -> Result<(), String> {
    match field {
        Field::ProfileHarness(n) => {
            let harness = text.trim().to_string();
            let flags = cfg.harness(&harness).ok().map(|(_, f)| f);
            let p = profile(cfg, n)?;
            p.harness = harness;
            if let Some(f) = flags {
                if f.model_flag.is_none() {
                    p.model = None;
                }
                let fits = |e: &String| {
                    f.effort_flag.is_some()
                        && (f.efforts.is_empty() || f.efforts.contains(&e.to_ascii_lowercase()))
                };
                if !p.effort.as_ref().is_some_and(fits) {
                    p.effort = None;
                }
            }
        }
        Field::ProfileModel(n) => profile(cfg, n)?.model = opt(text),
        Field::ProfileEffort(n) => {
            profile(cfg, n)?.effort = opt(text).map(|e| e.to_ascii_lowercase())
        }
        Field::ProfileArgs(n) => {
            profile(cfg, n)?.args = text.split_whitespace().map(str::to_string).collect()
        }
        Field::HarnessCommand(k) => {
            let command = text.trim().to_string();
            if let Some(h) = cfg.harnesses.get_mut(k) {
                h.command = command;
            } else if let Some(a) = cfg.agents.get_mut(k) {
                a.command = command;
            } else {
                return Err(format!("harness {k} no longer exists"));
            }
        }
        _ => return Err("not a profile or harness field".into()),
    }
    Ok(())
}
