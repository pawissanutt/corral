//! The worktree explorer: one tree of the session's dedicated worktree, its folders unfolding in
//! place, beside the preview of the file opened from it. Selectors are rooted on the daemon, not
//! on this device: every unfolded folder is its own `SessionFiles` read, the open file a
//! `SessionFile`.
use std::collections::BTreeSet;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use corral_protocol::{
    app::{SESSION_FILES_V1, SESSION_HTML_V1},
    detail::{DetailBody, DetailRef, SessionFile, SessionFileKind as Kind},
};

use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::{Hue, Id};

use super::{Action, Cx, PartBody, ReadState, SessionRole, State, Tab, tabs};

/// Folders unfolded at once. Each is a read while it shows; past the bound, unfolding one more
/// folds every folder off its way.
const MAX_OPEN: usize = 64;
/// Tree rows drawn at once, so a wide unfolded tree cannot stall a redraw.
const MAX_ROWS: usize = 3000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesUi {
    /// The selected entry: a folder, or the file the preview shows.
    pub path: String,
    pub directory: bool,
    /// The file the preview shows; selecting a folder keeps it.
    pub open: Option<String>,
    /// Folders unfolded in place; the root always is.
    pub expanded: BTreeSet<String>,
    /// The filter and the folder whose names it narrows: the daemon filters within one folder.
    pub filter: String,
    pub filter_in: String,
    /// Where tree and preview cannot stand side by side, the tree shows in the preview's place.
    pub browsing: bool,
}

impl FilesUi {
    pub const EMPTY: Self = Self {
        path: String::new(),
        directory: true,
        open: None,
        expanded: BTreeSet::new(),
        filter: String::new(),
        filter_in: String::new(),
        browsing: true,
    };

    fn listing(&self, session: &Id, folder: &str) -> DetailRef {
        DetailRef::SessionFiles {
            id: session.to_string(),
            path: folder.to_owned(),
            filter: (folder == self.filter_in && !self.filter.is_empty())
                .then(|| self.filter.clone()),
        }
    }

    fn file(&self, session: &Id) -> Option<DetailRef> {
        self.open.as_ref().map(|path| DetailRef::SessionFile {
            id: session.to_string(),
            path: path.clone(),
        })
    }

    /// Whether `folder`'s entries show: it and every folder above it are unfolded.
    fn shows(&self, folder: &str) -> bool {
        folder.is_empty() || (self.expanded.contains(folder) && self.shows(parent(folder)))
    }

    /// Every read the explorer shows: each folder showing, then the open file.
    pub fn targets(&self, session: &Id) -> Vec<DetailRef> {
        std::iter::once("")
            .chain(self.expanded.iter().map(String::as_str))
            .filter(|folder| self.shows(folder))
            .map(|folder| self.listing(session, folder))
            .chain(self.file(session))
            .collect()
    }

    fn unfold(&mut self, folder: &str) {
        if folder.is_empty() || self.expanded.contains(folder) {
            return;
        }
        if self.expanded.len() >= MAX_OPEN {
            self.expanded
                .retain(|kept| folder.starts_with(&format!("{kept}/")));
        }
        self.expanded.insert(folder.to_owned());
    }

    /// Selects `path`, unfolding the folders above it (and a folder itself); a file opens in
    /// the preview.
    fn reveal(&mut self, path: &str, directory: bool) {
        for (at, _) in path.match_indices('/') {
            self.unfold(&path[..at]);
        }
        if directory {
            self.unfold(path);
        } else {
            self.open = Some(path.to_owned());
        }
        self.path = path.to_owned();
        self.directory = directory;
        self.browsing = directory;
    }

    /// The folder a filter typed now narrows: the selected folder, or the selected file's.
    fn focused_folder(&self) -> &str {
        if self.directory {
            &self.path
        } else {
            parent(&self.path)
        }
    }
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn joined(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_owned()
    } else {
        format!("{folder}/{name}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesAction {
    /// Selects `path` with the folders above it unfolded; a file opens in the preview, a folder
    /// unfolds and shows the tree where only one of them fits.
    Open {
        session: Id,
        path: String,
        directory: bool,
    },
    /// A folder's row: unfolds or folds it in place and selects it.
    Toggle { session: Id, path: String },
    /// Narrows the focused folder's names.
    Filter { session: Id, text: String },
    /// Reads every folder showing and the open file again.
    Reread { session: Id },
    /// Where only one fits, the tree in the preview's place.
    Browse { session: Id },
    /// Closes the preview.
    Close { session: Id },
}

pub(super) fn act(state: &mut State, action: FilesAction, feed: &Feed) -> Vec<Effect> {
    let (session, reads) = match action {
        FilesAction::Open {
            session,
            path,
            directory,
        } => {
            let files = &mut state.ui_mut(&session).files;
            files.reveal(&path, directory);
            let reads = if directory {
                vec![files.listing(&session, &path)]
            } else {
                files.file(&session).into_iter().collect()
            };
            (session, reads)
        }
        FilesAction::Toggle { session, path } => {
            let files = &mut state.ui_mut(&session).files;
            let unfolding = !files.expanded.remove(&path);
            if unfolding {
                files.unfold(&path);
            }
            files.path = path.clone();
            files.directory = true;
            let reads = if unfolding {
                vec![files.listing(&session, &path)]
            } else {
                Vec::new()
            };
            (session, reads)
        }
        FilesAction::Filter { session, text } => {
            let files = &mut state.ui_mut(&session).files;
            let text = text.trim();
            files.filter = text[..text.floor_char_boundary(text.len().min(128))]
                .chars()
                .filter(|c| !c.is_control())
                .collect();
            files.filter_in = files.focused_folder().to_owned();
            let folder = files.filter_in.clone();
            files.reveal(&folder, true);
            let reads = vec![files.listing(&session, &folder)];
            (session, reads)
        }
        FilesAction::Reread { session } => {
            let reads = state.ui(session.as_str()).files.targets(&session);
            (session, reads)
        }
        FilesAction::Browse { session } => {
            state.ui_mut(&session).files.browsing = true;
            (session, Vec::new())
        }
        FilesAction::Close { session } => {
            let files = &mut state.ui_mut(&session).files;
            files.open = None;
            files.browsing = true;
            (session, Vec::new())
        }
    };
    if state.ui(session.as_str()).tab != Tab::Files {
        // Entering the tab reads everything it shows.
        return state.act(
            Action::Tab {
                session,
                tab: Tab::Files,
            },
            feed,
        );
    }
    reads
        .into_iter()
        .flat_map(|target| state.act(Action::Refresh(target), feed))
        .collect()
}

/// The kind of mark a file row leads with, from its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileIcon {
    Folder,
    Markdown,
    Html,
    Image,
    Code,
    Data,
    Text,
}

impl FileIcon {
    fn of(name: &str) -> Self {
        let extension = name
            .rsplit_once('.')
            .map(|(_, extension)| extension.to_ascii_lowercase())
            .unwrap_or_default();
        match extension.as_str() {
            "md" | "markdown" | "mdx" => Self::Markdown,
            "html" | "htm" => Self::Html,
            "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" => Self::Image,
            "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "py" | "go" | "c" | "h" | "cc"
            | "cpp" | "java" | "kt" | "swift" | "rb" | "sh" | "css" | "scss" => Self::Code,
            "json" | "toml" | "yaml" | "yml" | "lock" | "xml" | "csv" => Self::Data,
            _ => Self::Text,
        }
    }
}

/// One entry of the tree, `depth` folders below the root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub path: String,
    pub name: String,
    pub depth: usize,
    pub directory: bool,
    /// A folder unfolded in place.
    pub open: bool,
    pub icon: FileIcon,
    pub press: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeItem {
    Row(TreeRow),
    /// What stands for a folder's entries or follows them: reading, failed (with Retry), empty,
    /// cut at a bound.
    Note {
        depth: usize,
        words: String,
        retry: Option<Action>,
    },
}

/// A step of the open file's path; the last is the file itself and does not press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crumb {
    pub name: String,
    pub press: Option<Action>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileEditor {
    pub part: PartBody,
    pub path: String,
    pub crumbs: Vec<Crumb>,
    pub content: Option<FilesContent>,
    /// The current read may be fetched whole to open in a browser (`files_large`).
    pub can_fetch: bool,
    pub close: Action,
    /// Back to the tree where only one of them fits.
    pub browse: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesContent {
    Source {
        text: String,
        html: bool,
        can_open: bool,
    },
    Markdown(String),
    Image(String),
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FilesView {
    pub session: Id,
    /// The root's read, its Reread reading every folder showing.
    pub tree: PartBody,
    pub items: Vec<TreeItem>,
    pub selected: String,
    pub filter: String,
    /// The folder the filter narrows, as the field names it.
    pub filter_in: String,
    pub editor: Option<FileEditor>,
    /// Where only one fits: the tree shows rather than the preview.
    pub browsing: bool,
}

pub(super) fn applies(cx: &Cx<'_>) -> bool {
    cx.feed.capable(SESSION_FILES_V1)
        && match cx.role {
            SessionRole::Build => cx.build().is_none_or(|build| !build.here),
            SessionRole::Child | SessionRole::Assistant => false,
            _ => {
                cx.run.is_some()
                    || cx.ended.is_some()
                    || (cx.role.task_bound() && cx.task.is_some())
            }
        }
}

fn files_action(cx: &Cx<'_>, action: impl FnOnce(Id) -> FilesAction) -> Action {
    Action::Files(action(cx.id.clone()))
}

pub(super) fn view(cx: &Cx<'_>) -> FilesView {
    let state = &cx.ui.files;
    let root = state.listing(cx.id, "");
    let mut tree = tabs::part(
        cx,
        Ok(root),
        None,
        "Tracked and untracked files".into(),
        Hue::Neutral,
    );
    if tree.reread.is_some() {
        tree.reread = Some(files_action(cx, |session| FilesAction::Reread { session }));
    }
    let mut items = Vec::new();
    walk(cx, "", 0, &mut items);
    FilesView {
        session: cx.id.clone(),
        tree,
        items,
        selected: state.path.clone(),
        filter: state.filter.clone(),
        filter_in: state.filter_in.clone(),
        editor: editor(cx),
        browsing: state.browsing || state.open.is_none(),
    }
}

/// `folder`'s entries as rows, each unfolded folder's under it.
fn walk(cx: &Cx<'_>, folder: &str, depth: usize, items: &mut Vec<TreeItem>) {
    let state = &cx.ui.files;
    let target = state.listing(cx.id, folder);
    let cache = cx.feed.detail(&target);
    let note = |words: &str, retry: Option<Action>| TreeItem::Note {
        depth,
        words: words.to_owned(),
        retry,
    };
    let listing = match cache.and_then(|cache| cache.body()) {
        Some(DetailBody::SessionFiles(listing)) => listing,
        Some(_) => {
            items.push(note("The daemon did not serve a folder listing.", None));
            return;
        }
        // The root's read stands in the tree's head.
        None if depth == 0 => return,
        None => {
            items.push(match ReadState::of(cache, cx.feed) {
                ReadState::Failed(why) => note(&why, Some(Action::Refresh(target))),
                _ => note("Reading…", None),
            });
            return;
        }
    };
    if listing.entries.is_empty() {
        items.push(if target_filtered(&target) {
            note("No names match the filter.", None)
        } else {
            note("No browsable files in this folder.", None)
        });
    }
    for entry in &listing.entries {
        if items.len() >= MAX_ROWS {
            items.push(note("Fold a folder to see more rows.", None));
            return;
        }
        let path = joined(folder, &entry.name);
        let open = entry.directory && state.expanded.contains(&path);
        let press = if entry.directory {
            let path = path.clone();
            files_action(cx, |session| FilesAction::Toggle { session, path })
        } else {
            let path = path.clone();
            files_action(cx, |session| FilesAction::Open {
                session,
                path,
                directory: false,
            })
        };
        items.push(TreeItem::Row(TreeRow {
            path: path.clone(),
            name: entry.name.clone(),
            depth,
            directory: entry.directory,
            open,
            icon: if entry.directory {
                FileIcon::Folder
            } else {
                FileIcon::of(&entry.name)
            },
            press,
        }));
        if open {
            walk(cx, &path, depth + 1, items);
            if items.len() > MAX_ROWS {
                return;
            }
        }
    }
    if listing.truncated {
        items.push(note(
            "Directory scan limit reached. Some entries are not shown.",
            None,
        ));
    }
}

fn target_filtered(target: &DetailRef) -> bool {
    matches!(
        target,
        DetailRef::SessionFiles {
            filter: Some(_),
            ..
        }
    )
}

fn editor(cx: &Cx<'_>) -> Option<FileEditor> {
    let state = &cx.ui.files;
    let path = state.open.clone()?;
    let target = state.file(cx.id)?;
    let part = tabs::part(
        cx,
        Ok(target.clone()),
        None,
        "Current worktree copy".into(),
        Hue::Neutral,
    );
    let current = matches!(&part.read, ReadState::Read { .. });
    let html = cx.feed.capable(SESSION_HTML_V1);
    let (content, can_fetch) = match cx.feed.detail(&target).and_then(|cache| cache.body()) {
        Some(DetailBody::SessionFile(file)) => {
            let can_fetch = current
                && html
                && (file.kind == Kind::Html
                    || (file.kind == Kind::TooLarge
                        && super::files_large::openable(&path, file.bytes)));
            (Some(preview(file, current)), can_fetch)
        }
        Some(_) => (
            Some(FilesContent::Unavailable(
                "The daemon did not serve a worktree preview.".into(),
            )),
            false,
        ),
        None => (None, false),
    };
    let mut crumbs = vec![Crumb {
        name: "Worktree".into(),
        press: Some(files_action(cx, |session| FilesAction::Open {
            session,
            path: String::new(),
            directory: true,
        })),
    }];
    let mut at = 0;
    for name in path.split('/') {
        let end = at + name.len();
        let last = end == path.len();
        let folder = path[..end].to_owned();
        crumbs.push(Crumb {
            name: name.to_owned(),
            press: (!last).then(|| {
                files_action(cx, |session| FilesAction::Open {
                    session,
                    path: folder,
                    directory: true,
                })
            }),
        });
        at = end + 1;
    }
    Some(FileEditor {
        part,
        path,
        crumbs,
        content,
        can_fetch,
        close: files_action(cx, |session| FilesAction::Close { session }),
        browse: files_action(cx, |session| FilesAction::Browse { session }),
    })
}

pub(super) fn fetches(cx: &Cx<'_>) -> Vec<Action> {
    cx.ui
        .files
        .targets(cx.id)
        .into_iter()
        .filter_map(|target| tabs::load(cx, Ok(target), None))
        .collect()
}

fn preview(file: &SessionFile, current: bool) -> FilesContent {
    let invalid =
        || FilesContent::Unavailable("Invalid file preview returned by the daemon.".into());
    match file.kind {
        Kind::TooLarge => {
            return FilesContent::Unavailable(format!(
                "File is too large to preview ({} bytes).",
                file.bytes
            ));
        }
        Kind::Binary | Kind::Other => {
            return FilesContent::Unavailable(format!(
                "No preview for this file type ({} bytes).",
                file.bytes
            ));
        }
        _ => {}
    }
    if file.bytes > 32 * 1024 || file.data.len() > (32usize * 1024).div_ceil(3) * 4 {
        return invalid();
    }
    let Ok(bytes) = STANDARD.decode(&file.data) else {
        return invalid();
    };
    if bytes.len() as u64 != file.bytes {
        return invalid();
    }
    if matches!(file.kind, Kind::Png | Kind::Jpeg) {
        let media_type = if file.kind == Kind::Png {
            "image/png"
        } else {
            "image/jpeg"
        };
        return FilesContent::Image(format!("data:{media_type};base64,{}", file.data));
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return invalid();
    };
    match file.kind {
        Kind::Markdown => FilesContent::Markdown(text),
        _ => FilesContent::Source {
            text,
            html: file.kind == Kind::Html,
            can_open: current && file.kind == Kind::Html,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_html_remains_source_and_stale_bytes_cannot_open() {
        let text = "<script>alert(1)</script>";
        let file = SessionFile {
            kind: Kind::Html,
            data: STANDARD.encode(text),
            bytes: text.len() as u64,
        };
        for current in [false, true] {
            assert_eq!(
                preview(&file, current),
                FilesContent::Source {
                    text: text.to_owned(),
                    html: true,
                    can_open: current,
                }
            );
        }
    }

    #[test]
    fn a_file_opened_deep_reads_every_folder_above_it_and_a_folded_one_stops_reading() {
        let session = Id::from("t-1_r-1");
        let mut files = FilesUi::EMPTY;
        files.reveal("src/model/files.rs", false);
        let folders = |files: &FilesUi| {
            files
                .targets(&session)
                .into_iter()
                .filter_map(|target| match target {
                    DetailRef::SessionFiles { path, .. } => Some(path),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(folders(&files), ["", "src", "src/model"]);
        assert_eq!(files.open.as_deref(), Some("src/model/files.rs"));
        files.expanded.remove("src");
        assert_eq!(folders(&files), [""]);
    }

    #[test]
    fn unfolding_past_the_bound_keeps_the_way_to_the_new_folder() {
        let mut files = FilesUi::EMPTY;
        for n in 0..MAX_OPEN {
            files.unfold(&format!("d{n}"));
        }
        files.reveal("d3/inner", true);
        assert!(files.expanded.len() <= MAX_OPEN);
        assert!(files.shows("d3/inner"));
    }
}
