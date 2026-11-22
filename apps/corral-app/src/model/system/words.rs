//! The page's sentences: a card's figure under each overlay, the summary over the map, the
//! legend, the notices and the one provenance line.

use std::collections::BTreeMap;

use corral_types::{ReadStatus, SystemMap, SystemRow, SystemStatus};

use super::scene::{Card, EdgeKind, Kind, Level, Scene};
use super::{Overlay, Window, count, plural};
use crate::model::Hue;

/// Languages that get their own colour; the rest share one.
pub const LANGUAGE_COLOURS: usize = 5;

/// The commits and lines changed in a window, if the card has activity facts.
pub fn activity(card: &Card, window: Window) -> Option<(u64, u64, u64)> {
    let stats = card.stats.as_ref()?;
    let days = match window {
        Window::Days7 => &stats.days_7,
        Window::Days30 => &stats.days_30,
    };
    Some((days.commits, days.added, days.removed))
}

/// What a card says under its name with this overlay.
pub fn figure(card: &Card, overlay: Overlay, window: Window) -> String {
    match card.kind {
        Kind::More => {
            if card.lines > 0 {
                format!("{} lines · not listed", count(card.lines))
            } else {
                "not listed".into()
            }
        }
        Kind::Port => match card.references {
            0 => "declared, no module reference".into(),
            n => plural(n, "reference", "references"),
        },
        Kind::Definition => match &card.definition {
            Some(d) if d.end_line > d.line => format!("lines {}–{}", d.line, d.end_line),
            Some(d) => format!("line {}", d.line),
            None => String::new(),
        },
        Kind::File => match overlay {
            Overlay::Structure => plural(card.lines, "line", "lines"),
            Overlay::Activity => "activity is counted per module".into(),
            Overlay::Coverage => match card.file.as_ref().map(|f| f.2) {
                Some(ReadStatus::Complete) => "read".into(),
                Some(ReadStatus::Unsupported) => "not read: unsupported".into(),
                Some(ReadStatus::Failed) => "read failed".into(),
                None => String::new(),
            },
        },
        Kind::Package | Kind::Folder | Kind::Module => match overlay {
            Overlay::Structure => format!(
                "{} · {}",
                plural(card.lines, "line", "lines"),
                plural(card.files(), "file", "files")
            ),
            Overlay::Activity => match activity(card, window) {
                Some((0, _, _)) | None => format!("no commits in {} days", window.days()),
                Some((commits, added, removed)) => format!(
                    "{} · +{} −{}",
                    plural(commits, "commit", "commits"),
                    count(added),
                    count(removed)
                ),
            },
            Overlay::Coverage => coverage(card),
        },
    }
}

fn coverage(card: &Card) -> String {
    let Some(stats) = &card.stats else {
        return String::new();
    };
    let mut words = format!("{} of {} read", count(stats.complete), plural(card.files(), "file", "files"));
    if stats.failed > 0 {
        words.push_str(&format!(" · {} failed", count(stats.failed)));
    }
    words
}

/// The share of a card's files read, unsupported and failed.
pub fn read_shares(card: &Card) -> Option<(f32, f32, f32)> {
    if let Some((_, _, status)) = &card.file {
        return Some(match status {
            ReadStatus::Complete => (1.0, 0.0, 0.0),
            ReadStatus::Unsupported => (0.0, 1.0, 0.0),
            ReadStatus::Failed => (0.0, 0.0, 1.0),
        });
    }
    let stats = card.stats.as_ref()?;
    let total = card.files();
    (total > 0).then(|| {
        let share = |n: u64| n as f32 / total as f32;
        (share(stats.complete), share(stats.unsupported), share(stats.failed))
    })
}

/// The level's languages by lines, most first.
pub fn languages(scene: &Scene) -> Vec<String> {
    let mut totals: BTreeMap<&str, u64> = BTreeMap::new();
    for card in &scene.cards {
        if let Some(stats) = &card.stats {
            for (language, lines) in &stats.languages {
                *totals.entry(language).or_default() += lines;
            }
        }
        if let Some((language, _, _)) = &card.file {
            *totals.entry(language).or_default() += card.lines;
        }
    }
    let mut list: Vec<_> = totals.into_iter().collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    list.into_iter().map(|(l, _)| l.to_owned()).collect()
}

/// The colour slot of a language on this level: its rank among the first few, else none.
pub fn language_slot(languages: &[String], language: &str) -> Option<usize> {
    languages
        .iter()
        .take(LANGUAGE_COLOURS)
        .position(|l| l == language)
}

/// The hue of a language slot.
pub const fn slot_hue(slot: usize) -> Hue {
    match slot {
        0 => Hue::Corral,
        1 => Hue::Worker,
        2 => Hue::Ok,
        3 => Hue::Held,
        _ => Hue::You,
    }
}

/// The busiest card's commits in the window, at least one.
pub fn busiest(scene: &Scene, window: Window) -> u64 {
    scene
        .cards
        .iter()
        .filter_map(|card| activity(card, window).map(|a| a.0))
        .max()
        .unwrap_or(0)
        .max(1)
}

/// The summary over the map: a few figures for this level and overlay.
pub fn summary(scene: &Scene, map: &SystemMap, overlay: Overlay, window: Window) -> Vec<String> {
    let lines: u64 = scene.cards.iter().filter(|c| c.kind != Kind::Port).map(|c| c.lines).sum();
    let files: u64 = scene.cards.iter().filter(|c| c.kind != Kind::Port).map(Card::files).sum();
    let mut out = Vec::new();
    match overlay {
        Overlay::Structure => {
            match scene.level {
                Level::Project => {
                    let packages = scene.cards.iter().filter(|c| c.kind == Kind::Package).count();
                    let folders = scene.cards.iter().filter(|c| c.kind == Kind::Folder).count();
                    out.push(format!("{} · {}", plural(files, "file", "files"), plural(lines, "line", "lines")));
                    out.push(format!(
                        "{} · {}",
                        plural(packages as u64, "package", "packages"),
                        plural(folders as u64, "folder", "folders")
                    ));
                }
                Level::Package => {
                    let modules = scene.cards.iter().filter(|c| c.kind == Kind::Module).count();
                    out.push(plural(modules as u64, "module", "modules"));
                    out.push(format!("{} · {}", plural(files, "file", "files"), plural(lines, "line", "lines")));
                }
                Level::Module => {
                    out.push(format!("{} · {}", plural(files, "file", "files"), plural(lines, "line", "lines")));
                    out.push("largest first".into());
                }
                Level::File => {
                    let defs = scene.cards.iter().filter(|c| c.kind == Kind::Definition).count();
                    out.push(plural(defs as u64, "definition", "definitions"));
                    out.push("in source order".into());
                }
            }
            let langs = languages(scene);
            if !langs.is_empty() && matches!(scene.level, Level::Project | Level::Package) {
                out.push(langs.into_iter().take(4).collect::<Vec<_>>().join(" · "));
            }
        }
        Overlay::Activity => {
            let busiest = scene
                .cards
                .iter()
                .filter_map(|c| activity(c, window).map(|a| (a.0, c)))
                .filter(|(n, _)| *n > 0)
                .max_by_key(|(n, _)| *n);
            let quiet = scene
                .cards
                .iter()
                .filter(|c| activity(c, window).is_some_and(|a| a.0 == 0))
                .count();
            match busiest {
                Some((n, card)) => out.push(format!(
                    "Busiest: {} · {} in {} days",
                    card.name,
                    plural(n, "commit", "commits"),
                    window.days()
                )),
                None if matches!(scene.level, Level::Module | Level::File) => {
                    out.push("Activity is counted per package and module".into())
                }
                None => out.push(format!("No commits in {} days", window.days())),
            }
            if quiet > 0 {
                out.push(format!("{} with no commits", count(quiet as u64)));
            }
        }
        Overlay::Coverage => match (&map.coverage, scene.level) {
            (Some(c), Level::Project) => {
                out.push(format!(
                    "Read {} of {} · {} unsupported · {} failed",
                    count(c.complete),
                    plural(c.complete + c.unsupported + c.failed, "file", "files"),
                    count(c.unsupported),
                    count(c.failed)
                ));
                out.push(format!(
                    "References {} resolved · {} unresolved",
                    count(c.resolved_references),
                    count(c.unresolved_references)
                ));
            }
            _ => {
                let read: u64 = scene
                    .cards
                    .iter()
                    .filter(|c| c.kind != Kind::Port)
                    .filter_map(|c| match (&c.stats, &c.file) {
                        (Some(s), _) => Some(s.complete),
                        (None, Some((_, _, ReadStatus::Complete))) => Some(1),
                        _ => None,
                    })
                    .sum();
                out.push(format!("Read {} of {}", count(read), plural(files, "file", "files")));
            }
        },
    }
    out
}

/// A legend entry's sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Swatch {
    Language(usize),
    /// Lines in a language past the coloured ones.
    OtherLanguage,
    Activity,
    Read,
    Unsupported,
    Failed,
    Edge(EdgeKind),
    Excluded,
    Folder,
    Spawn,
}

/// The legend for this level and overlay.
pub fn legend(scene: &Scene, overlay: Overlay, window: Window) -> Vec<(Swatch, String)> {
    let mut out = Vec::new();
    match overlay {
        Overlay::Structure => {
            let langs = languages(scene);
            for (i, l) in langs.iter().take(LANGUAGE_COLOURS).enumerate() {
                out.push((Swatch::Language(i), l.clone()));
            }
            if langs.len() > LANGUAGE_COLOURS {
                out.push((Swatch::OtherLanguage, "other languages".into()));
            }
        }
        Overlay::Activity => out.push((
            Swatch::Activity,
            format!("commits in {} days, relative to the busiest here", window.days()),
        )),
        Overlay::Coverage => {
            out.push((Swatch::Read, "read".into()));
            out.push((Swatch::Unsupported, "not read: unsupported".into()));
            out.push((Swatch::Failed, "read failed".into()));
        }
    }
    match scene.level {
        Level::Project => {
            for kind in [EdgeKind::Declared, EdgeKind::DeclaredOnly, EdgeKind::ReferencedOnly] {
                out.push((Swatch::Edge(kind), kind.word().into()));
            }
            out.push((Swatch::Spawn, "processes it spawns, with sites".into()));
            out.push((Swatch::Excluded, "excluded from its workspace".into()));
            out.push((Swatch::Folder, "folder, no manifest".into()));
        }
        Level::Package => {
            out.push((Swatch::Edge(EdgeKind::Module), "module reference".into()));
            out.push((Swatch::Edge(EdgeKind::Boundary), "to or from a neighbouring package".into()));
            let total = scene.edges.len() + scene.undrawn.len();
            if !scene.undrawn.is_empty() {
                out.push((
                    Swatch::Edge(EdgeKind::Module),
                    format!(
                        "drawn: {} of {} references, each module's strongest first; select a module for all of its own",
                        count(scene.edges.len() as u64),
                        count(total as u64)
                    ),
                ));
            }
        }
        Level::Module | Level::File => {}
    }
    out
}

/// A notice over the map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub hue: Hue,
    pub words: String,
}

/// What the page says before or over the map: facts building, a failed build, an extension's
/// error (the built-in map is still drawn), rows left out to fit the wire.
pub fn notices(map: Option<&SystemMap>, row: Option<&SystemRow>) -> Vec<Notice> {
    let mut out = Vec::new();
    let building = map.map(|m| m.status) == Some(SystemStatus::Building)
        || row.map(|r| r.status) == Some(SystemStatus::Building);
    if building {
        out.push(Notice {
            hue: Hue::Corral,
            words: "Building this project's facts. The map fills in when they are ready.".into(),
        });
    }
    if let Some(error) = map.and_then(|m| m.error.clone()).or_else(|| row.and_then(|r| r.error.clone())) {
        out.push(Notice {
            hue: Hue::Fail,
            words: format!("The map could not be built: {error}"),
        });
    }
    let extension = map
        .and_then(|m| m.provider.as_ref())
        .map(|p| p.extension.clone())
        .or_else(|| row.and_then(|r| r.extension.clone()));
    let failed = map
        .and_then(|m| m.provider.as_ref())
        .and_then(|p| p.error.clone())
        .or_else(|| row.and_then(|r| r.last_extension_error.clone()));
    if let Some(error) = failed {
        out.push(Notice {
            hue: Hue::Held,
            words: format!(
                "Extension {} failed: {error}. The built-in map is drawn without its depth.",
                extension.as_deref().unwrap_or("facts")
            ),
        });
    }
    if let Some(map) = map.filter(|m| m.omitted_rows > 0) {
        out.push(Notice {
            hue: Hue::Neutral,
            words: format!(
                "{} left out to stay within the wire budget.",
                plural(map.omitted_rows as u64, "row", "rows")
            ),
        });
    }
    out
}

/// The one provenance line: the commit, what established the facts and what an extension
/// supplied.
pub fn provenance(map: Option<&SystemMap>, row: Option<&SystemRow>) -> String {
    let head = map
        .and_then(|m| m.head.clone())
        .or_else(|| row.and_then(|r| r.head.clone()));
    let at = match &head {
        Some(head) => format!("Facts at {}", head.chars().take(9).collect::<String>()),
        None => "No commit read yet".into(),
    };
    let extension = row.and_then(|r| r.extension.clone());
    let depth = match (map.and_then(|m| m.provider.as_ref()), extension) {
        (Some(p), _) if p.coverage.provided.is_empty() => {
            format!("{} answered with nothing", p.extension)
        }
        (Some(p), _) => format!("{} supplied {}", p.extension, p.coverage.provided.join(", ")),
        (None, Some(name)) => format!("{name} has not answered"),
        (None, None) => "no facts extension configured".into(),
    };
    format!("{at} · built in from manifests, the file tree, an import scan and git · {depth}")
}
