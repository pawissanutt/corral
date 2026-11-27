//! `@` file mentions: while the draft's last word starts with `@`, a popup over the composer
//! lists the session worktree's entries in the folder the typed path names (`@src/mo` lists
//! `src/`), narrowed by what follows its last `/` (name start first, then anywhere in the name,
//! then its letters in order). Choosing a folder descends (`@src/`), a file puts `@path ` in the
//! draft, as Pi's composer does: the host reads `@path` as a file mention. The folder is the same
//! `SessionFiles` read the Files tab makes, one per folder. A session without a worktree the app
//! can read keeps the popup open with why. Esc closes it on that text and keeps the text.

use corral_protocol::detail::{DetailBody, DetailRef, SessionFiles};

use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;

use super::{Action, Cx, ReadState, State, files, tabs};

/// At most this many rows show.
pub const SHOWN: usize = 8;

/// The popup's state on this device.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MentionUi {
    /// The highlighted row among every matched one.
    pub at: usize,
    /// The draft Esc closed the popup on.
    pub closed: Option<String>,
}

impl MentionUi {
    pub const EMPTY: Self = Self {
        at: 0,
        closed: None,
    };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MentionAction {
    /// Highlight row `at`.
    At { session: Id, at: usize },
    /// Close the popup and keep the text.
    Close(Id),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MentionMenu {
    pub session: Id,
    /// The window of matched rows shown, around the highlighted one.
    pub rows: Vec<MentionRow>,
    /// The highlighted row within `rows`: what Enter and Tab choose.
    pub at: Option<usize>,
    /// Matched rows above the window.
    pub first: usize,
    /// Every matched row; ↑ ↓ walk all of them.
    pub total: usize,
    /// Matched rows below the window.
    pub more: usize,
    /// The folder listed, as typed: `src/`, or empty for the worktree's root.
    pub folder: String,
    /// What the read says, or why there is nothing to list.
    pub note: Option<(Hue, String)>,
    /// Esc: close the popup, keep the text.
    pub close: Action,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MentionRow {
    /// `mention-<n>`, by its place among the matched rows.
    pub id: String,
    /// `mod.rs`, `model/` for a folder.
    pub name: String,
    pub directory: bool,
    /// The draft with the typed mention replaced: `@src/model/` or `@src/mod.rs `.
    pub pick: Action,
}

impl MentionMenu {
    /// The highlighted row's choice, if any.
    pub fn chosen(&self) -> Option<&Action> {
        self.rows.get(self.at?).map(|row| &row.pick)
    }

    /// Highlighting the row `step` away from the highlighted one, within every matched row.
    pub fn step(&self, step: isize) -> Option<Action> {
        let at = self.first + self.at?;
        let next = at
            .saturating_add_signed(step)
            .min(self.total.saturating_sub(1));
        Some(Action::Mention(MentionAction::At {
            session: self.session.clone(),
            at: next,
        }))
    }
}

/// The typed mention: the draft before it, the folder it names (without its trailing `/`) and
/// the text after that folder's `/`. None unless the draft's last word starts with `@`; a `/`
/// command or a `!` shell command takes none.
pub(super) fn typed(draft: &str) -> Option<(&str, &str, &str)> {
    if draft.starts_with('/') || draft.starts_with('!') {
        return None;
    }
    let start = draft
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(at, c)| at + c.len_utf8());
    let word = draft[start..].strip_prefix('@')?;
    let (folder, query) = word.rsplit_once('/').unwrap_or(("", word));
    Some((&draft[..start], folder.trim_start_matches("./"), query))
}

fn listing(session: &Id, folder: &str) -> DetailRef {
    DetailRef::SessionFiles {
        id: session.to_string(),
        path: folder.to_owned(),
        filter: None,
    }
}

fn served<'f>(feed: &'f Feed, target: &DetailRef) -> Option<&'f SessionFiles> {
    match feed.detail(target)?.body()? {
        DetailBody::SessionFiles(listing) => Some(listing),
        _ => None,
    }
}

fn subsequence(name: &str, query: &str) -> bool {
    let mut rest = name.chars();
    query.chars().all(|letter| rest.any(|c| c == letter))
}

/// How well `name` matches `query`, best first; none when it does not.
fn rank(name: &str, query: &str) -> Option<u8> {
    let name = name.to_lowercase();
    if name.starts_with(query) {
        Some(0)
    } else if name.contains(query) {
        Some(1)
    } else if query.chars().count() >= 2 && subsequence(&name, query) {
        Some(2)
    } else {
        None
    }
}

/// `@path`, quoted where the path holds a space, as Pi reads it.
fn mention(path: &str) -> String {
    if path.contains(char::is_whitespace) {
        format!("@\"{path}\"")
    } else {
        format!("@{path}")
    }
}

/// The popup of the session `cx` reads; none while the draft's last word is not an `@` mention
/// or after Esc closed it on this text.
pub fn menu(cx: &Cx<'_>) -> Option<MentionMenu> {
    let draft = cx.ui.draft.as_str();
    let (before, folder, query) = typed(draft)?;
    if cx.ui.mention.closed.as_deref() == Some(draft) {
        return None;
    }
    let shown_folder = if folder.is_empty() {
        String::new()
    } else {
        format!("{folder}/")
    };
    let mut menu = MentionMenu {
        session: cx.id.clone(),
        rows: Vec::new(),
        at: None,
        first: 0,
        total: 0,
        more: 0,
        folder: shown_folder.clone(),
        note: None,
        close: Action::Mention(MentionAction::Close(cx.id.clone())),
    };
    if !files::applies(cx) {
        menu.note = Some((
            Hue::Held,
            "File mentions are unavailable for this session: it has no worktree the app can read."
                .into(),
        ));
        return Some(menu);
    }
    let target = listing(cx.id, folder);
    let Some(listing) = served(cx.feed, &target) else {
        menu.note = Some(match ReadState::of(cx.feed.detail(&target), cx.feed) {
            ReadState::Failed(why) => (Hue::Fail, format!("Could not read {shown_folder}: {why}")),
            _ => (Hue::Neutral, "Reading the worktree…".into()),
        });
        return Some(menu);
    };
    let query = query.to_lowercase();
    let mut ranked: Vec<(u8, &str, bool)> = listing
        .entries
        .iter()
        .filter(|entry| entry.name != ".git")
        .filter_map(|entry| {
            rank(&entry.name, &query).map(|rank| (rank, entry.name.as_str(), entry.directory))
        })
        .collect();
    // Stable: the served order stays within a rank.
    ranked.sort_by_key(|(rank, _, _)| *rank);
    let total = ranked.len();
    menu.total = total;
    if total == 0 {
        menu.note = Some((
            Hue::Neutral,
            if query.is_empty() {
                format!("Nothing to mention in {}", or_root(&shown_folder))
            } else {
                format!("No names in {} match {query}", or_root(&shown_folder))
            },
        ));
        return Some(menu);
    }
    if listing.truncated {
        menu.note = Some((
            Hue::Neutral,
            "This folder is large; keep typing to narrow it.".into(),
        ));
    }
    let at = cx.ui.mention.at.min(total - 1);
    let first = at
        .saturating_sub(SHOWN / 2)
        .min(total.saturating_sub(SHOWN));
    let end = total.min(first + SHOWN);
    menu.rows = ranked[first..end]
        .iter()
        .enumerate()
        .map(|(offset, (_, name, directory))| {
            let path = format!("{shown_folder}{name}");
            let text = if *directory {
                format!("{before}{}/", mention(&path))
            } else {
                format!("{before}{} ", mention(&path))
            };
            MentionRow {
                id: format!("mention-{}", first + offset),
                name: if *directory {
                    format!("{name}/")
                } else {
                    (*name).to_owned()
                },
                directory: *directory,
                pick: Action::Draft {
                    session: cx.id.clone(),
                    text,
                },
            }
        })
        .collect();
    menu.at = Some(at - first);
    menu.first = first;
    menu.more = total - end;
    Some(menu)
}

fn or_root(folder: &str) -> &str {
    if folder.is_empty() {
        "the worktree's root"
    } else {
        folder
    }
}

/// The folder's read while the popup lists it: once, while nothing was read.
pub(super) fn fetch(cx: &Cx<'_>) -> Option<Action> {
    let (_, folder, _) = typed(&cx.ui.draft)?;
    if cx.ui.mention.closed.as_deref() == Some(cx.ui.draft.as_str()) || !files::applies(cx) {
        return None;
    }
    tabs::load(cx, Ok(listing(cx.id, folder)), None)
}

pub(super) fn act(state: &mut State, action: MentionAction, _feed: &Feed) -> Vec<Effect> {
    match action {
        MentionAction::At { session, at } => state.ui_mut(&session).mention.at = at,
        MentionAction::Close(session) => {
            let ui = state.ui_mut(&session);
            ui.mention.closed = Some(ui.draft.clone());
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use corral_protocol::app::SESSION_FILES_V1;
    use corral_protocol::detail::SessionFileEntry;

    use super::*;
    use crate::model::daemon::{DetailCache, detail_key};
    use crate::model::health::daemon_env;
    use crate::model::nav::Form;
    use crate::model::session::{Facts, SessionRole, Ui};

    fn entry(name: &str, directory: bool) -> SessionFileEntry {
        SessionFileEntry {
            name: name.into(),
            directory,
        }
    }

    /// The popup listed the worktree's root for `@src/`, so a file inside a folder could never
    /// be mentioned without typing its whole path.
    #[test]
    fn a_typed_folder_lists_and_filters_that_folders_entries() {
        let id = Id::from("T-1_r1");
        let mut feed = Feed::default();
        feed.capabilities.insert(SESSION_FILES_V1.into());
        for (folder, entries) in [
            ("", vec![entry("src", true), entry("README.md", false)]),
            (
                "src",
                vec![
                    entry("model", true),
                    entry("lib.rs", false),
                    entry("main.rs", false),
                ],
            ),
        ] {
            let target = listing(&id, folder);
            feed.details.insert(
                detail_key(&target),
                DetailCache {
                    transcript_seams: Default::default(),
                    target,
                    pages: Vec::new(),
                    loading: false,
                    stale: false,
                    error: None,
                    merged: Some(DetailBody::SessionFiles(SessionFiles {
                        entries,
                        truncated: false,
                    })),
                },
            );
        }
        let facts = Facts {
            role: SessionRole::Worker,
            task: Some(Id::from("T-1")),
        };
        let ui = Ui {
            draft: "look at @src/mo".into(),
            ..Ui::default()
        };
        let cx = Cx::new(&feed, &ui, &id, daemon_env(&feed, Form::Desktop), &facts);
        let shown = menu(&cx).map(|menu| menu.rows);
        let names: Vec<(String, Action)> = shown
            .unwrap_or_default()
            .into_iter()
            .map(|row| (row.name, row.pick))
            .collect();
        assert_eq!(
            names,
            vec![(
                "model/".to_owned(),
                Action::Draft {
                    session: id.clone(),
                    text: "look at @src/model/".into(),
                },
            )]
        );
    }
}
