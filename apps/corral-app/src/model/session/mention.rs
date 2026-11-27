//! `@` file mentions: while the draft's last word starts with `@`, a popup over the composer
//! lists the session worktree's entries in the folder the typed path names (`@src/mo` lists
//! `src/`), narrowed by what follows its last `/` (name start first, then anywhere in the name,
//! then its letters in order). Choosing a folder descends (`@src/`), a file puts `@path ` in the
//! draft, as Pi's composer does: the host reads `@path` as a file mention; a path with a space is
//! quoted (`@"my dir/a.rs"`), a folder's quote left open (`@"my dir/`) so the popup goes on
//! reading it. The folder is the same `SessionFiles` read the Files tab makes, one per folder;
//! where corrald cut that listing short, what is typed is searched by corrald's own filter. A
//! session without a worktree the app can read keeps the popup open with why. Esc closes it on
//! that text and keeps the text. While the dock takes the text as a reason or an answer there is
//! no popup.
//!
//! A host with `session.file_search.v1` is asked instead for the files the typed word fuzzy-matches
//! anywhere in the worktree, once something is typed that is not a folder path ending in `/`;
//! its paths show in the rank it gave them and choosing one puts the whole `@path ` in the draft.
//! Only the read of the exact word typed now is ever shown, so a slower reply for an earlier word
//! is never drawn. An empty `@`, a path ending in `/`, a word corrald would refuse as a query and
//! a host without the capability keep the folder listing above.

use corral_protocol::app::SESSION_FILE_SEARCH_V1;
use corral_protocol::detail::{
    DetailBody, DetailRef, SESSION_FILE_SEARCH_LIMIT_MAX, SESSION_FILE_SEARCH_QUERY_MAX_BYTES,
    SessionFileSearch, SessionFiles,
};

use crate::model::Id;
use crate::model::daemon::{Feed, Link};
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
/// the text after that folder's `/`. None unless the draft ends in a mention: its last word
/// starting with `@`, or a quoted `@"…` (which may hold spaces) not yet closed, or closed with
/// nothing after it. A `/` command or a `!` shell command takes none.
pub(super) fn typed(draft: &str) -> Option<(&str, &str, &str)> {
    let (start, word) = word(draft)?;
    let (folder, query) = word.rsplit_once('/').unwrap_or(("", word));
    Some((&draft[..start], folder.trim_start_matches("./"), query))
}

/// Where the draft's last mention starts and what follows its `@` (and opening quote).
fn word(draft: &str) -> Option<(usize, &str)> {
    if draft.starts_with('/') || super::composer::shell_typed(draft) {
        return None;
    }
    quoted(draft).or_else(|| {
        let start = draft
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map_or(0, |(at, c)| at + c.len_utf8());
        Some((start, draft[start..].strip_prefix('@')?))
    })
}

/// The draft's last `@"…` mention, where it starts and what it quotes so far, while it is open
/// or closed with nothing after it.
fn quoted(draft: &str) -> Option<(usize, &str)> {
    let start = draft.rfind("@\"")?;
    if draft[..start]
        .chars()
        .next_back()
        .is_some_and(|c| !c.is_whitespace())
    {
        return None;
    }
    let inner = &draft[start + 2..];
    let inner = inner.strip_suffix('"').unwrap_or(inner);
    (!inner.contains(['"', '\n'])).then_some((start, inner))
}

/// The listing of `folder`, narrowed by corrald to names holding `filter` when given.
fn listing(session: &Id, folder: &str, filter: Option<&str>) -> DetailRef {
    DetailRef::SessionFiles {
        id: session.to_string(),
        path: folder.to_owned(),
        filter: filter.map(str::to_owned),
    }
}

/// The search corrald runs for `query` in a folder whose whole listing it cut short, so a name
/// past its bound can still be found; none while the listing is whole or nothing is typed, or
/// for text corrald would refuse as a filter.
fn search<'q>(feed: &Feed, whole: &DetailRef, query: &'q str) -> Option<&'q str> {
    let cut = served(feed, whole).is_some_and(|listing| listing.truncated);
    (cut && !query.is_empty() && query.len() <= 128 && !query.chars().any(char::is_control))
        .then_some(query)
}

/// The fuzzy search of the typed word, when the host has it and corrald would accept the word as
/// a query: text, not a folder path ending in `/`, within the query bound and without controls.
/// Anything else is not sent, so a refused query is never asked for again on every keystroke.
fn fuzzy(cx: &Cx<'_>) -> Option<DetailRef> {
    if !cx.feed.capable(SESSION_FILE_SEARCH_V1) {
        return None;
    }
    let (_, word) = word(&cx.ui.draft)?;
    let query = word.strip_prefix("./").unwrap_or(word);
    let asked = !query.trim().is_empty()
        && !query.ends_with('/')
        && query.len() <= SESSION_FILE_SEARCH_QUERY_MAX_BYTES
        && !query.chars().any(char::is_control);
    asked.then(|| DetailRef::SessionFileSearch {
        id: cx.id.to_string(),
        query: query.to_owned(),
        limit: SESSION_FILE_SEARCH_LIMIT_MAX,
    })
}

fn found<'f>(feed: &'f Feed, target: &DetailRef) -> Option<&'f SessionFileSearch> {
    let cache = feed.detail(target)?;
    if feed.link != Link::Up || cache.loading || cache.stale || cache.error.is_some() {
        return None;
    }
    match cache.body()? {
        DetailBody::SessionFileSearch(found) => Some(found),
        _ => None,
    }
}

/// Whether the popup is the composer's now: not while the dock takes the text as a reason or an
/// answer, whose Enter gives it.
fn offered(cx: &Cx<'_>) -> bool {
    cx.ui.dock.compose.is_none() && cx.ui.mention.closed.as_deref() != Some(cx.ui.draft.as_str())
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

/// `@path`, quoted where the path holds a space, as Pi reads it. A folder's (its `/` kept) quote
/// stays open, so what is typed next is still inside the mention and the popup goes on listing.
fn mention(path: &str, directory: bool) -> String {
    match (path.contains(char::is_whitespace), directory) {
        (false, _) => format!("@{path}"),
        (true, true) => format!("@\"{path}"),
        (true, false) => format!("@\"{path}\""),
    }
}

/// The popup of the session `cx` reads; none while the draft does not end in an `@` mention,
/// after Esc closed it on this text, or while the dock takes the text.
pub fn menu(cx: &Cx<'_>) -> Option<MentionMenu> {
    let draft = cx.ui.draft.as_str();
    let (before, folder, query) = typed(draft)?;
    if !offered(cx) {
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
    if let Some(target) = fuzzy(cx) {
        menu.folder = String::new();
        search_rows(cx, &mut menu, before, &target);
        return Some(menu);
    }
    let query = query.to_lowercase();
    let whole = listing(cx.id, folder, None);
    // corrald's search of a cut-short folder once it answers; its whole listing meanwhile.
    let searched = search(cx.feed, &whole, &query)
        .map(|query| listing(cx.id, folder, Some(query)))
        .filter(|target| served(cx.feed, target).is_some());
    let filtered = searched.is_some();
    // corrald matches substrings only, so its search adds the names the cut listing left out
    // while the listing keeps the looser local matches.
    let held = if filtered {
        served(cx.feed, &whole)
    } else {
        None
    };
    let target = searched.unwrap_or(whole);
    let Some(listing) = served(cx.feed, &target) else {
        menu.note = Some(match ReadState::of(cx.feed.detail(&target), cx.feed) {
            ReadState::Failed(why) => (Hue::Fail, format!("Could not read {shown_folder}: {why}")),
            _ => (Hue::Neutral, "Reading the worktree…".into()),
        });
        return Some(menu);
    };
    let mut seen = std::collections::HashSet::new();
    let mut ranked: Vec<(u8, &str, bool)> = held
        .into_iter()
        .flat_map(|held| held.entries.iter())
        .chain(listing.entries.iter())
        .filter(|entry| entry.name != ".git" && seen.insert(entry.name.as_str()))
        .filter_map(|entry| {
            rank(&entry.name, &query).map(|rank| (rank, entry.name.as_str(), entry.directory))
        })
        .collect();
    // Stable: the served order stays within a rank.
    ranked.sort_by_key(|(rank, _, _)| *rank);
    let total = ranked.len();
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
        let first = listing.entries.len();
        let folder = or_root(&shown_folder);
        menu.note = Some((
            Hue::Neutral,
            if filtered {
                format!("Only the first {first} names in {folder} holding {query} are searched.")
            } else if query.is_empty() {
                format!("Only the first {first} entries of {folder} are listed; type to search.")
            } else {
                format!("Only the first {first} entries of {folder} are searched so far.")
            },
        ));
    }
    fill(cx, &mut menu, total, |at| {
        let (_, name, directory) = ranked[at];
        if directory {
            (
                format!("{name}/"),
                true,
                format!(
                    "{before}{}",
                    mention(&format!("{shown_folder}{name}/"), true)
                ),
            )
        } else {
            (
                name.to_owned(),
                false,
                format!(
                    "{before}{} ",
                    mention(&format!("{shown_folder}{name}"), false)
                ),
            )
        }
    });
    Some(menu)
}

/// The window of `total` matched rows around the highlighted one; `row` gives the label, whether
/// it is a folder and the draft choosing it makes, for the matched row at an index.
fn fill(
    cx: &Cx<'_>,
    menu: &mut MentionMenu,
    total: usize,
    row: impl Fn(usize) -> (String, bool, String),
) {
    menu.total = total;
    if total == 0 {
        return;
    }
    let at = cx.ui.mention.at.min(total - 1);
    let first = at
        .saturating_sub(SHOWN / 2)
        .min(total.saturating_sub(SHOWN));
    let end = total.min(first + SHOWN);
    menu.rows = (first..end)
        .map(|n| {
            let (name, directory, text) = row(n);
            MentionRow {
                id: format!("mention-{n}"),
                name,
                directory,
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
}

/// The rows of corrald's search of the exact word typed now: its paths in its rank, each
/// choosing the whole `@path`. Nothing of another word's read shows while this one is read.
fn search_rows(cx: &Cx<'_>, menu: &mut MentionMenu, before: &str, target: &DetailRef) {
    let DetailRef::SessionFileSearch { query, limit, .. } = target else {
        return;
    };
    let Some(served) = found(cx.feed, target) else {
        menu.note = Some(if cx.feed.link != Link::Up {
            (Hue::Held, "Connect to search the worktree.".into())
        } else if let Some(err) = cx
            .feed
            .detail(target)
            .and_then(|cache| cache.error.as_ref())
        {
            (
                Hue::Fail,
                format!("Could not search the worktree: {}", err.message),
            )
        } else {
            (Hue::Neutral, "Searching the worktree…".into())
        });
        return;
    };
    let paths = &served.paths[..served.paths.len().min(usize::from(*limit))];
    if paths.is_empty() {
        menu.note = Some((
            Hue::Neutral,
            if served.truncated {
                format!("No files found for {query} in the part of the worktree searched.")
            } else {
                format!("No files match {query}")
            },
        ));
        return;
    }
    if served.truncated {
        menu.note = Some((
            Hue::Neutral,
            "More files may exist. End a folder path with / to browse it.".into(),
        ));
    }
    fill(cx, menu, paths.len(), |at| {
        let path = &paths[at];
        (
            path.clone(),
            false,
            format!("{before}{} ", mention(path, false)),
        )
    });
}

fn or_root(folder: &str) -> &str {
    if folder.is_empty() {
        "the worktree's root"
    } else {
        folder
    }
}

/// The folder's read while the popup lists it: once, while nothing was read; then, where that
/// listing was cut short, corrald's search of the folder for what is typed.
pub(super) fn fetch(cx: &Cx<'_>) -> Option<Action> {
    let (_, folder, query) = typed(&cx.ui.draft)?;
    if !offered(cx) || !files::applies(cx) {
        return None;
    }
    if let Some(target) = fuzzy(cx) {
        // The newest draft gets its own read as soon as the connection's outstanding search
        // settles. Intermediate keystrokes neither queue scans nor pick an earlier word's rows.
        if cx.feed.link != Link::Up || cx.feed.file_search_loading() {
            return None;
        }
        return tabs::load(cx, Ok(target), None);
    }
    let whole = listing(cx.id, folder, None);
    let query = query.to_lowercase();
    let target = match search(cx.feed, &whole, &query) {
        Some(query) => listing(cx.id, folder, Some(query)),
        None => whole,
    };
    tabs::load(cx, Ok(target), None)
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
    use crate::model::session::dock::Compose;
    use crate::model::session::{Facts, SessionRole, Ui};

    fn entry(name: &str, directory: bool) -> SessionFileEntry {
        SessionFileEntry {
            name: name.into(),
            directory,
        }
    }

    /// A folder with a space went in as `@"my dir"/`, which no longer read as a mention, so the
    /// popup could not descend into it.
    #[test]
    fn a_quoted_folder_mention_still_reads_as_one() {
        let picked = format!("see {}", mention("my dir/", true));
        assert_eq!(typed(&picked), Some(("see ", "my dir", "")));
        assert_eq!(typed("see @\"my dir/ma"), Some(("see ", "my dir", "ma")));
        assert_eq!(typed("see @\"my dir/main.rs\" "), None);
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
            let target = listing(&id, folder, None);
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
        let mut ui = Ui {
            draft: "look at @src/mo".into(),
            ..Ui::default()
        };
        // While the dock takes the text as a reason, Enter must give it, not choose a file.
        ui.dock.compose = Some(Compose::Reason {
            card: "c1".into(),
            option: "land".into(),
            label: "Land".into(),
            why: String::new(),
        });
        let cx = Cx::new(&feed, &ui, &id, daemon_env(&feed, Form::Desktop), &facts);
        assert_eq!(menu(&cx), None);
        assert_eq!(fetch(&cx), None);
        ui.dock.compose = None;
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

    /// Typing after `@` listed the typed folder only, so a file in another folder needed its
    /// whole path; and a slower reply for an earlier word could have shown under a later one.
    #[test]
    fn a_typed_word_shows_only_its_own_search_and_picks_the_whole_path() {
        let id = Id::from("T-1_r1");
        let mut feed = Feed {
            link: Link::Up,
            ..Feed::default()
        };
        feed.capabilities.insert(SESSION_FILES_V1.into());
        feed.capabilities.insert(SESSION_FILE_SEARCH_V1.into());
        let search = |query: &str| DetailRef::SessionFileSearch {
            id: id.to_string(),
            query: query.into(),
            limit: SESSION_FILE_SEARCH_LIMIT_MAX,
        };
        for (query, paths) in [
            ("mo", vec!["src/model/mod.rs", "docs/more.md"]),
            ("mod", vec!["stale.rs"]),
        ] {
            let target = search(query);
            feed.details.insert(
                detail_key(&target),
                DetailCache {
                    transcript_seams: Default::default(),
                    target,
                    pages: Vec::new(),
                    loading: false,
                    stale: false,
                    error: None,
                    merged: Some(DetailBody::SessionFileSearch(SessionFileSearch {
                        paths: paths.into_iter().map(str::to_owned).collect(),
                        truncated: false,
                    })),
                },
            );
        }
        let facts = |role| Facts {
            role,
            task: Some(Id::from("T-1")),
        };
        let worker = facts(SessionRole::Worker);
        let mut ui = Ui {
            draft: "look at @mo".into(),
            ..Ui::default()
        };
        let cx = Cx::new(&feed, &ui, &id, daemon_env(&feed, Form::Desktop), &worker);
        let rows = menu(&cx).map(|menu| menu.rows).unwrap_or_default();
        let shown: Vec<(String, Action)> = rows.into_iter().map(|r| (r.name, r.pick)).collect();
        // The server's order, each choice the whole path.
        assert_eq!(shown[0].0, "src/model/mod.rs");
        assert_eq!(shown[1].0, "docs/more.md");
        assert_eq!(
            shown[0].1,
            Action::Draft {
                session: id.clone(),
                text: "look at @src/model/mod.rs ".into(),
            }
        );
        assert_eq!(fetch(&cx), None);

        // The word typed next has its own selector: nothing of `mo`'s answer shows meanwhile, and
        // it is read. A folder path ending in `/` keeps the folder listing.
        ui.draft = "look at @mox".into();
        let older = detail_key(&search("mod"));
        feed.details.get_mut(&older).unwrap().loading = true;
        let cx = Cx::new(&feed, &ui, &id, daemon_env(&feed, Form::Desktop), &worker);
        assert_eq!(menu(&cx).map(|menu| menu.rows.len()), Some(0));
        assert_eq!(fetch(&cx), None);
        feed.details.get_mut(&older).unwrap().loading = false;
        let cx = Cx::new(&feed, &ui, &id, daemon_env(&feed, Form::Desktop), &worker);
        assert_eq!(fetch(&cx), Some(Action::Refresh(search("mox"))));
        feed.details.get_mut(&older).unwrap().stale = true;
        ui.draft = "look at @mod".into();
        let cx = Cx::new(&feed, &ui, &id, daemon_env(&feed, Form::Desktop), &worker);
        assert_eq!(menu(&cx).map(|menu| menu.rows.len()), Some(0));
        ui.draft = "look at @src/".into();
        let cx = Cx::new(&feed, &ui, &id, daemon_env(&feed, Form::Desktop), &worker);
        assert_eq!(fetch(&cx), Some(Action::Refresh(listing(&id, "src", None))));

        // A role without a worktree says so and reads nothing.
        ui.draft = "look at @mo".into();
        let assistant = facts(SessionRole::Assistant);
        let cx = Cx::new(
            &feed,
            &ui,
            &id,
            daemon_env(&feed, Form::Desktop),
            &assistant,
        );
        assert!(menu(&cx).is_some_and(|menu| menu.rows.is_empty() && menu.note.is_some()));
        assert_eq!(fetch(&cx), None);
    }
}
