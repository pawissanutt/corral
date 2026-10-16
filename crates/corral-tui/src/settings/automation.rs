//! The settings screen's Review section (`Section::Automation`): the review autopilot switch and the review policy, workspace-wide
//! (`[review]`) and per project (`[projects.<p>.review]`). Unset keys inherit; each row says what applies then.

use std::collections::BTreeMap;

use corral_core::{
    Accept, Config, MergeMode, PolicySource, ReviewOverride, ReviewPolicy, Reviewer,
};

use super::gate::{self, Gates};
use super::model::{Field, Kind, Row, number, opt, profile_names};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Scope {
    Global,
    Project(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum List {
    Gate,
    OwnerPaths,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Key {
    Heading,
    /// Project scope only: inherit the list or keep an own one.
    Mode(List),
    Item(List, usize),
    Add(List),
    /// Why a project with no gate has no preset to offer (read-only).
    Detected,
    /// A detected gate preset; `⏎` stages its commands as the project's own gate list.
    Preset(usize),
    /// Effective gate env / size limits (read-only; edited in corral.toml).
    Env,
    SizeLimits,
    Reviewer,
    Accept,
    MaxRounds,
    Merge,
    MaxDiff,
}

fn layer<'a>(cfg: &'a Config, scope: &Scope) -> Option<&'a ReviewOverride> {
    match scope {
        Scope::Global => Some(&cfg.review),
        Scope::Project(k) => cfg.projects.get(k).map(|p| &p.review),
    }
}

fn layer_mut<'a>(cfg: &'a mut Config, scope: &Scope) -> Result<&'a mut ReviewOverride, String> {
    match scope {
        Scope::Global => Ok(&mut cfg.review),
        Scope::Project(k) => cfg
            .projects
            .get_mut(k)
            .map(|p| &mut p.review)
            .ok_or_else(|| format!("project {k} no longer exists")),
    }
}

fn list_mut(r: &mut ReviewOverride, l: List) -> &mut Option<Vec<String>> {
    match l {
        List::Gate => &mut r.gate,
        List::OwnerPaths => &mut r.owner_paths,
    }
}

fn list_of(r: &ReviewOverride, l: List) -> Option<&Vec<String>> {
    match l {
        List::Gate => r.gate.as_ref(),
        List::OwnerPaths => r.owner_paths.as_ref(),
    }
}

fn reviewer_text(r: &Reviewer) -> String {
    match r {
        Reviewer::None => "none".into(),
        Reviewer::Pm => "pm".into(),
        Reviewer::CrossVendor => "cross-vendor".into(),
        Reviewer::Profile(p) => p.clone(),
    }
}

fn accept_text(a: Accept) -> &'static str {
    match a {
        Accept::Owner => "owner",
        Accept::Auto => "auto",
    }
}

fn merge_text(m: MergeMode) -> &'static str {
    match m {
        MergeMode::Rebase => "rebase",
        MergeMode::Ff => "ff",
    }
}

fn pairs<V: std::fmt::Display>(m: &BTreeMap<String, V>, sep: &str) -> String {
    let v: Vec<String> = m.iter().map(|(k, v)| format!("{k}{sep}{v}")).collect();
    joined(&v)
}

fn joined(v: &[String]) -> String {
    if v.is_empty() {
        "none".into()
    } else {
        v.join(" · ")
    }
}

/// The text an unset key shows: the inherited value and the layer it comes from.
fn fallback(scope: &Scope, value: String, source: PolicySource) -> String {
    let from = match (scope, source) {
        (Scope::Global, _) | (Scope::Project(_), PolicySource::Default) => "built-in default",
        _ => "from [review]",
    };
    format!("{value} ({from})")
}

pub(crate) fn rows(cfg: &Config, gates: &Gates) -> Vec<Row> {
    let on = cfg.autopilot_enabled();
    let mut out = vec![
        Row::new(
            Field::Autopilot,
            "Autopilot",
            Kind::Toggle,
            if on { "on" } else { "off" }.into(),
        )
        .help("Drives Review tasks through gate, review and landing ([review] autopilot; O on the board)."),
    ];
    out.extend(scope_rows(cfg, &Scope::Global, gates));
    for k in cfg.projects.keys() {
        out.extend(scope_rows(cfg, &Scope::Project(k.clone()), gates));
    }
    out
}

fn scope_rows(cfg: &Config, scope: &Scope, gates: &Gates) -> Vec<Row> {
    let Some(r) = layer(cfg, scope) else {
        return Vec::new();
    };
    let (title, prefix) = match scope {
        Scope::Global => (
            "Review policy, all projects".to_string(),
            "[review]".to_string(),
        ),
        Scope::Project(k) => (format!("{k} review"), format!("[projects.{k}.review]")),
    };
    let base: ReviewPolicy = match scope {
        Scope::Global => ReviewPolicy::default(),
        Scope::Project(_) => cfg.review_policy(None, None),
    };
    let src = match scope {
        Scope::Global => Default::default(),
        Scope::Project(_) => cfg.review_policy_sources(None, None),
    };
    let f = |key: Key| Field::Review(scope.clone(), key);
    let mut out = vec![Row::new(
        f(Key::Heading),
        title,
        Kind::Header,
        String::new(),
    )];
    out.extend(list_rows(
        scope,
        r,
        List::Gate,
        &prefix,
        joined(&base.gate),
        src.gate,
    ));
    let project = match scope {
        Scope::Global => None,
        Scope::Project(k) => Some(k.as_str()),
    };
    let effective = cfg.review_policy(project, None);
    if let Some(k) = project
        && effective.gate.is_empty()
    {
        out.extend(gate::preset_rows(scope, gates.get(k)));
    }
    let mut reviewers: Vec<Option<String>> = vec![None, Some("none".into()), Some("pm".into())];
    reviewers.extend(profile_names(cfg).into_iter().map(Some));
    if let Some(cur) = &r.reviewer
        && !reviewers.iter().any(|o| o.as_deref() == Some(cur))
    {
        reviewers.push(Some(cur.clone()));
    }
    out.push(
        Row::new(
            f(Key::Reviewer),
            "Reviewer",
            Kind::Pick(reviewers),
            r.reviewer.clone().unwrap_or_default(),
        )
        .help("Who reviews after the gate passes: none, the PM agent, or an agent profile.")
        .inherits(fallback(scope, reviewer_text(&base.reviewer), src.reviewer))
        .key(format!("{prefix}: unknown reviewer")),
    );
    out.push(
        Row::new(
            f(Key::Accept),
            "Accept mode",
            Kind::Pick(vec![None, Some("owner".into()), Some("auto".into())]),
            r.accept.map(accept_text).unwrap_or_default().into(),
        )
        .help("owner: stop in Review marked reviewed; auto: land when gate and review pass.")
        .inherits(fallback(scope, accept_text(base.accept).into(), src.accept)),
    );
    out.push(
        Row::new(
            f(Key::MaxRounds),
            "Max rounds",
            Kind::Number { optional: true },
            r.max_rounds.map(|n| n.to_string()).unwrap_or_default(),
        )
        .help("Automatic change requests before the task is escalated to you.")
        .inherits(fallback(scope, base.max_rounds.to_string(), src.max_rounds)),
    );
    out.push(
        Row::new(
            f(Key::Merge),
            "Merge strategy",
            Kind::Pick(vec![None, Some("rebase".into()), Some("ff".into())]),
            r.merge.map(merge_text).unwrap_or_default().into(),
        )
        .help("rebase onto a moved base, or ff: ask the agent to rebase.")
        .inherits(fallback(scope, merge_text(base.merge).into(), src.merge)),
    );
    out.push(
        Row::new(
            f(Key::MaxDiff),
            "Diff limit",
            Kind::Number { optional: true },
            r.max_diff_lines.map(|n| n.to_string()).unwrap_or_default(),
        )
        .help("Changed lines above this always need you.")
        .inherits(fallback(
            scope,
            format!("{} lines", base.max_diff_lines),
            src.max_diff_lines,
        )),
    );
    let own = |m: Option<usize>| m.is_some_and(|n| n > 0);
    let mut env = pairs(&effective.env, "=");
    if !own(r.env.as_ref().map(BTreeMap::len)) {
        env = fallback(scope, env, src.env);
    }
    out.push(Row::new(f(Key::Env), "Gate env", Kind::Info, env).help(
        "Variables set for gate commands; merged by key across layers. Edit env in corral.toml.",
    ));
    let mut limits = pairs(&effective.size_limits, " ≤ ");
    if !own(r.size_limits.as_ref().map(BTreeMap::len)) {
        limits = fallback(scope, limits, src.size_limits);
    }
    out.push(
        Row::new(f(Key::SizeLimits), "Size limits", Kind::Info, limits)
            .help("Source lines allowed per file extension; merged by key. Edit size_limits in corral.toml."),
    );
    out.extend(list_rows(
        scope,
        r,
        List::OwnerPaths,
        &prefix,
        joined(&base.owner_paths),
        src.owner_paths,
    ));
    out
}

fn list_rows(
    scope: &Scope,
    r: &ReviewOverride,
    l: List,
    prefix: &str,
    inherited: String,
    source: PolicySource,
) -> Vec<Row> {
    let (label, item, add, help, key) = match l {
        List::Gate => (
            "Gate commands",
            "Gate",
            "+ add gate command",
            "Shell commands run in the run worktree, in order; any failure sends the run back.",
            format!("{prefix}: gate"),
        ),
        List::OwnerPaths => (
            "Owner paths",
            "Owner path",
            "+ add owner path",
            "Globs (relative to the repo root); touching any forces your acceptance.",
            format!("{prefix}: owner_paths"),
        ),
    };
    let f = |key: Key| Field::Review(scope.clone(), key);
    let own = list_of(r, l);
    let mut out = Vec::new();
    if let Scope::Project(_) = scope {
        out.push(
            Row::new(
                f(Key::Mode(l)),
                label,
                Kind::Pick(vec![None, Some("own".into())]),
                if own.is_some() { "own" } else { "" }.into(),
            )
            .help(help)
            .inherits(fallback(scope, inherited, source)),
        );
        if own.is_none() {
            return out;
        }
    }
    for (i, v) in own.into_iter().flatten().enumerate() {
        out.push(
            Row::new(
                f(Key::Item(l, i)),
                format!("{item} {}", i + 1),
                Kind::ListItem,
                v.clone(),
            )
            .key(key.clone()),
        );
    }
    let mut add = Row::new(f(Key::Add(l)), add, Kind::Add, String::new());
    if *scope == Scope::Global {
        add = add.help(help);
    }
    out.push(add);
    out
}

pub(crate) fn set_value(cfg: &mut Config, field: &Field, text: &str) -> Result<(), String> {
    match field {
        Field::Autopilot => cfg.review.autopilot = Some(text == "on"),
        Field::Review(scope, key) => set_key(cfg, scope, key, text)?,
        _ => return Err("not an automation field".into()),
    }
    Ok(())
}

fn set_key(cfg: &mut Config, scope: &Scope, key: &Key, text: &str) -> Result<(), String> {
    let inherited_lists = cfg.review.clone();
    let r = layer_mut(cfg, scope)?;
    match key {
        Key::Heading | Key::Detected | Key::Env | Key::SizeLimits => {
            return Err("this row is read-only here".into());
        }
        Key::Preset(_) => {
            let commands: Vec<String> = text.lines().filter_map(opt).collect();
            if commands.is_empty() {
                return Err("the preset has no commands".into());
            }
            r.gate = Some(commands);
        }
        Key::Mode(l) => {
            *list_mut(r, *l) =
                opt(text).map(|_| list_of(&inherited_lists, *l).cloned().unwrap_or_default());
        }
        Key::Item(l, i) => {
            let global = *scope == Scope::Global;
            let list = list_mut(r, *l);
            let items = list.as_mut().ok_or("the list is inherited")?;
            match (opt(text), *i < items.len()) {
                (_, false) => return Err(format!("item {} no longer exists", i + 1)),
                (Some(t), true) => items[*i] = t,
                (None, true) => {
                    items.remove(*i);
                }
            }
            if global && items.is_empty() {
                *list = None;
            }
        }
        Key::Add(l) => {
            if let Some(t) = opt(text) {
                list_mut(r, *l).get_or_insert_with(Vec::new).push(t);
            }
        }
        Key::Reviewer => r.reviewer = opt(text),
        Key::Accept => {
            r.accept = match opt(text).as_deref() {
                None => None,
                Some("owner") => Some(Accept::Owner),
                Some("auto") => Some(Accept::Auto),
                Some(o) => return Err(format!("{o:?} is not owner or auto")),
            }
        }
        Key::Merge => {
            r.merge = match opt(text).as_deref() {
                None => None,
                Some("rebase") => Some(MergeMode::Rebase),
                Some("ff") => Some(MergeMode::Ff),
                Some(o) => return Err(format!("{o:?} is not rebase or ff")),
            }
        }
        Key::MaxRounds => r.max_rounds = opt(text).map(|t| number(&t)).transpose()?,
        Key::MaxDiff => r.max_diff_lines = opt(text).map(|t| number(&t)).transpose()?,
    }
    Ok(())
}

pub(crate) fn remove(cfg: &mut Config, field: &Field) -> bool {
    matches!(field, Field::Review(_, Key::Item(..))) && set_value(cfg, field, "").is_ok()
}

pub(crate) fn move_item(cfg: &mut Config, field: &Field, up: bool) -> Option<Field> {
    let Field::Review(scope, Key::Item(l, i)) = field else {
        return None;
    };
    let items = list_mut(layer_mut(cfg, scope).ok()?, *l).as_mut()?;
    let j = if up { i.checked_sub(1)? } else { i + 1 };
    if j >= items.len() || *i >= items.len() {
        return None;
    }
    items.swap(*i, j);
    Some(Field::Review(scope.clone(), Key::Item(*l, j)))
}
