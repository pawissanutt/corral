//! The dedicated worktree browser. Selectors are rooted on the daemon, not on this device.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use corral_protocol::{
    app::SESSION_FILES_V1,
    detail::{DetailBody, DetailRef, SessionFile, SessionFileKind as Kind},
};

use crate::model::{Hue, Id};

use super::{Action, Cx, PartBody, ReadState, SessionRole, tabs};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesUi {
    pub path: String,
    pub directory: bool,
    pub filter: String,
}

impl FilesUi {
    pub const EMPTY: Self = Self {
        path: String::new(),
        directory: true,
        filter: String::new(),
    };

    pub fn target(&self, session: &Id) -> DetailRef {
        let id = session.to_string();
        let path = self.path.clone();
        if self.directory {
            DetailRef::SessionFiles {
                id,
                path,
                filter: (!self.filter.is_empty()).then(|| self.filter.clone()),
            }
        } else {
            DetailRef::SessionFile { id, path }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesAction {
    Open {
        session: Id,
        path: String,
        directory: bool,
    },
    Filter {
        session: Id,
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    pub name: String,
    pub directory: bool,
    pub open: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesContent {
    Listing {
        rows: Vec<FileRow>,
        truncated: bool,
    },
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
    pub part: PartBody,
    pub path: String,
    pub up: Option<Action>,
    pub filter: Option<String>,
    pub content: Option<FilesContent>,
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

fn select(cx: &Cx<'_>, path: String, directory: bool) -> Action {
    Action::Files(FilesAction::Open {
        session: cx.id.clone(),
        path,
        directory,
    })
}

pub(super) fn view(cx: &Cx<'_>) -> FilesView {
    let state = &cx.ui.files;
    let target = state.target(cx.id);
    let summary = if state.directory {
        "Tracked and untracked files"
    } else {
        "Worktree file"
    };
    let part = tabs::part(cx, Ok(target.clone()), None, summary.into(), Hue::Neutral);
    let up = (!state.path.is_empty()).then(|| {
        let parent = state.path.rsplit_once('/').map_or("", |(parent, _)| parent);
        select(cx, parent.to_owned(), true)
    });
    let content = cx
        .feed
        .detail(&target)
        .and_then(|cache| cache.body())
        .map(|body| match body {
            DetailBody::SessionFiles(listing) => FilesContent::Listing {
                rows: listing
                    .entries
                    .iter()
                    .map(|entry| FileRow {
                        name: entry.name.clone(),
                        directory: entry.directory,
                        open: select(
                            cx,
                            if state.path.is_empty() {
                                entry.name.clone()
                            } else {
                                format!("{}/{}", state.path, entry.name)
                            },
                            entry.directory,
                        ),
                    })
                    .collect(),
                truncated: listing.truncated,
            },
            DetailBody::SessionFile(file) => {
                preview(file, matches!(&part.read, ReadState::Read { .. }))
            }
            _ => FilesContent::Unavailable("The daemon did not serve a worktree preview.".into()),
        });
    FilesView {
        part,
        path: state.path.clone(),
        up,
        filter: state.directory.then(|| state.filter.clone()),
        content,
    }
}

pub(super) fn fetches(cx: &Cx<'_>) -> Vec<Action> {
    tabs::load(cx, Ok(cx.ui.files.target(cx.id)), None)
        .into_iter()
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
}
