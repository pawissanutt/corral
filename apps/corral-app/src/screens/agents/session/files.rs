//! The Files tab: the worktree's tree on the left (`files_tree.rs`) and the preview of the file
//! opened from it on the right, under the file's breadcrumbs. Selecting a folder or a file never
//! replaces the tree: both stand side by side from 560 pt of tab width on desktop and 64 Cells
//! in a terminal; narrower, and on the phone, the tree shows until a file opens, and the
//! preview's `‹ Files` and breadcrumbs return to it. test ids `session-files`,
//! `session-files-path`, `crumb-<n>`, `btn-files-tree`, `btn-files-close`,
//! `session-file-preview`, `session-files-empty`, `session-file-source`,
//! `btn-open-html-browser`; the session chrome carries the return to the conversation.
#[path = "files_open.rs"]
mod files_open;
#[path = "files_tree.rs"]
mod tree;

use nsh_core::prelude::*;

use crate::model::{
    Form,
    session::{
        Action, Body, FileEditor, FilesContent, FilesView, SessionPage, Tab,
        files_large::HtmlProgress,
    },
};
use crate::screens::markdown::{markdown_without_images, provide_styles};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::{super::run, tabs};

/// Tab widths from which the tree stands beside the preview: points, and Cells in a terminal.
const BESIDE: f32 = 560.0;
const BESIDE_CELLS: f32 = 64.0;

pub(super) fn tab(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let form = app.form;
    let shown = computed(move || {
        page.with(|page| match &page.body {
            Body::Files(files) => Some(files.clone()),
            _ => None,
        })
    });
    let back = page.with_untracked(|page| {
        let changes = page.tabs.iter().any(|item| item.tab == Tab::Changes);
        (changes || form == Form::Terminal).then(|| {
            let action = Action::Tab {
                session: page.id.clone(),
                tab: if changes { Tab::Changes } else { Tab::Chat },
            };
            (
                if changes {
                    "‹ Changes"
                } else {
                    "‹ Session"
                },
                action,
            )
        })
    });
    let html = files_open::progress();
    let width = signal(None::<f32>);
    let beside = computed(move || {
        let width = width.get();
        match form {
            Form::Phone => false,
            Form::Desktop => width.is_some_and(|width| width >= BESIDE),
            Form::Terminal => width.is_some_and(|width| width >= term::cells(BESIDE_CELLS).0),
        }
    });
    // The tree's column: a third of the tab between bounds, so a wide tab gives the room to
    // the preview.
    let column = computed(move || {
        let width = width.get().unwrap_or(0.0);
        match form {
            Form::Terminal if width >= term::cells(110.0).0 => term::cells(34.0),
            Form::Terminal => term::cells(26.0),
            _ => px((width * 0.3).clamp(220.0, 320.0)),
        }
    });
    let browsing = computed(move || shown.with(|files| files.as_ref().is_some_and(|f| f.browsing)));
    let content = dynamic(move || {
        let beside = beside.get();
        // On a split view selection updates the editor, not the tree's scroll container.
        let browsing = !beside && browsing.get();
        let back = back.clone();
        untracked(|| {
            let explorer = || tree::explorer(app, shown, back.clone());
            if beside {
                let rule = match form {
                    Form::Terminal => term::vrule(),
                    _ => view()
                        .w(px(1.0))
                        .flex_none()
                        .bg(color(Token::Rule))
                        .into_element(),
                };
                view()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex_row()
                    .child(
                        view()
                            .w(move || column.get())
                            .flex_none()
                            .min_h(px(0.0))
                            .flex_col()
                            .when(form != Form::Terminal, |side| side.bg(color(Token::BgSunk)))
                            .child(explorer()),
                    )
                    .child(rule)
                    .child(editor(app, shown, html, false))
                    .into_element()
            } else if browsing {
                explorer()
            } else {
                editor(app, shown, html, true)
            }
        })
    });
    view()
        .test_id("session-files")
        .flex_1()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .flex_col()
        .on_layout(move |frame: &Bounds<Points>| {
            let measured = Some(frame.size.width.0);
            if width.get_untracked() != measured {
                width.set(measured);
            }
        })
        .child(content)
        .into_element()
}

/// The preview's column: the open file's breadcrumbs and read, then its content; with no file
/// open, what to do. `alone` where it stands in the tree's place.
fn editor(
    app: AppState,
    shown: Memo<Option<FilesView>>,
    html: Signal<HtmlProgress>,
    alone: bool,
) -> AnyElement {
    let opened = computed(move || {
        shown.with(|files| {
            files
                .as_ref()
                .map(|files| (files.session.clone(), files.editor.clone()))
        })
    });
    let body = dynamic(move || {
        let opened = opened.get();
        untracked(|| match opened {
            Some((session, Some(editor))) => {
                let head = crumbs(app, &editor, alone);
                let part = tabs::part(app, &editor.part, "files");
                let open = {
                    let (source, html_file) = match &editor.content {
                        Some(FilesContent::Source {
                            text,
                            html: true,
                            can_open,
                        }) => (can_open.then_some(text.as_str()), true),
                        _ => (None, false),
                    };
                    (html_file || editor.can_fetch)
                        .then(|| {
                            files_open::browser_button(
                                app,
                                html,
                                &session,
                                &editor.path,
                                source,
                                editor.can_fetch,
                            )
                        })
                        .flatten()
                };
                let content = editor.content.as_ref().map(|content| drawn(app, content));
                let body = view()
                    .flex_col()
                    .gap(px(if app.form == Form::Terminal { 0.0 } else { 8.0 }))
                    .child(part)
                    .children(open)
                    .children(content);
                view()
                    .flex_1()
                    .min_h(px(0.0))
                    .min_w(px(0.0))
                    .flex_col()
                    .child(head)
                    .child(tabs::surface(
                        app,
                        "session-file-preview",
                        body.into_element(),
                    ))
                    .into_element()
            }
            _ => view()
                .test_id("session-files-empty")
                .flex_1()
                .items_center()
                .justify_center()
                .px(px(16.0))
                .py(px(16.0))
                .child(note("Select a file in the tree to preview it."))
                .into_element(),
        })
    });
    view()
        .flex_1()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .flex_col()
        .child(body)
        .into_element()
}

/// The preview's head: `‹ Files` where it stands alone, the file's path as breadcrumbs (each
/// folder reveals it in the tree), and Close.
fn crumbs(app: AppState, editor: &FileEditor, alone: bool) -> AnyElement {
    let form = app.form;
    let separator = || {
        text("›")
            .text_color(color(Token::Ink4))
            .flex_shrink_0()
            .into_element()
    };
    let mut path = view()
        .test_id("session-files-path")
        .flex_1()
        .min_w(px(0.0))
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(if form == Form::Terminal { 8.0 } else { 2.0 }));
    for (n, crumb) in editor.crumbs.iter().enumerate() {
        if n > 0 {
            path = path.child(separator());
        }
        path = match crumb.press.clone() {
            Some(action) => path.child(button(
                app,
                crumb.name.clone(),
                ButtonKind::Quiet,
                format!("crumb-{n}"),
                move || run(app, action.clone()),
            )),
            None => path.child({
                let name = text(crumb.name.clone())
                    .test_id(&format!("crumb-{n}"))
                    .number_of_lines(1u32)
                    .truncate()
                    .min_w(px(0.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(Token::Ink));
                match form {
                    Form::Terminal => name,
                    _ => name.font_family(FONT_TEXT).text_size(px(13.0)),
                }
            }),
        };
    }
    let browse = editor.browse.clone();
    let close = editor.close.clone();
    let head = view()
        .flex_none()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .children(alone.then(|| {
            button(
                app,
                "‹ Files",
                ButtonKind::Quiet,
                "btn-files-tree",
                move || run(app, browse.clone()),
            )
        }))
        .child(path)
        .child(button(
            app,
            if form == Form::Terminal {
                "×"
            } else {
                "Close"
            },
            ButtonKind::Quiet,
            "btn-files-close",
            move || run(app, close.clone()),
        ));
    let head = match form {
        Form::Terminal => head.px(term::cells(1.0)),
        Form::Desktop => head.px(px(12.0)).min_h(px(34.0)),
        Form::Phone => head.px(px(12.0)).min_h(px(44.0)),
    };
    view()
        .flex_none()
        .child(head)
        .child(match form {
            Form::Terminal => term::hrule(),
            _ => view().h(px(1.0)).bg(color(Token::Rule)).into_element(),
        })
        .into_element()
}

fn drawn(app: AppState, content: &FilesContent) -> AnyElement {
    match content {
        FilesContent::Source { text, html, .. } => {
            let mut column = view().flex_col();
            if *html {
                column = column.child(note("HTML source · not rendered here"));
            }
            column.child(source(app, text)).into_element()
        }
        FilesContent::Markdown(words) => {
            provide_styles(app);
            let words = safe_display(words);
            markdown_without_images(app, &words)
        }
        FilesContent::Image(uri) => {
            if app.form == Form::Terminal {
                return note("Image preview is available in the graphical app.");
            }
            let ratio = signal(1.0_f32);
            view()
                .max_w(px(720.0))
                .child(
                    image(uri.clone())
                        .w_full()
                        .aspect_ratio(move || ratio.get())
                        .object_fit(ResizeMode::Contain)
                        .on_load(move |size: &Size<DevicePixels>| {
                            if size.width.0 > 0 && size.height.0 > 0 {
                                ratio.set(size.width.0 as f32 / size.height.0 as f32);
                            }
                        }),
                )
                .into_element()
        }
        FilesContent::Unavailable(why) => note(why),
    }
}

fn note(words: &str) -> AnyElement {
    text(words.to_owned())
        .text_color(color(Token::Ink2))
        .into_element()
}

fn safe_display(source: &str) -> String {
    source
        .replace("\r\n", "\n")
        .chars()
        .map(|c| {
            if c.is_control() && !matches!(c, '\n' | '\t') {
                '\u{fffd}'
            } else {
                c
            }
        })
        .collect()
}

fn source(app: AppState, value: &str) -> AnyElement {
    let words = text(safe_display(value))
        .select_text()
        .font_family(FONT_MONO)
        .text_color(color(Token::Ink));
    if app.form == Form::Terminal {
        return crate::ui::composer::terminal_selectable(app, view().selectable(true))
            .test_id("session-file-source")
            .child(words)
            .into_element();
    }
    scroll()
        .test_id("session-file-source")
        .horizontal()
        .child(
            view()
                .flex_shrink_0()
                .selectable(true)
                .child(words.flex_shrink_0()),
        )
        .into_element()
}
