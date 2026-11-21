//! Worktree files replace the main view; the session chrome carries the return to conversation.
use nsh_core::prelude::*;

use crate::model::{
    Form, Id,
    session::{Action, Body, FilesAction, FilesContent, FilesView, SessionPage, Tab},
};
use crate::screens::markdown::{markdown_without_images, provide_styles};
use crate::state::AppState;
use crate::ui::composer::copyable_input;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, Token, color};

use super::{super::run, tabs};

pub(super) fn tab(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let shown = computed(move || {
        page.with(|page| {
            let Body::Files(files) = &page.body else {
                return None;
            };
            let changes = page.tabs.iter().any(|item| item.tab == Tab::Changes);
            let back = Action::Tab {
                session: page.id.clone(),
                tab: if changes { Tab::Changes } else { Tab::Chat },
            };
            Some((files.clone(), changes, back))
        })
    });
    let filter = computed(move || {
        page.with(|page| match &page.body {
            Body::Files(files) => files
                .filter
                .clone()
                .map(|filter| (page.id.clone(), files.path.clone(), filter)),
            _ => None,
        })
    });
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .child(dynamic(move || {
            filter
                .get()
                .map(|(session, _, current)| untracked(|| filter_bar(app, session, current)))
        }))
        .child(dynamic(move || {
            shown
                .get()
                .map(|(files, changes, back)| untracked(|| draw(app, &files, changes, back)))
        }))
        .into_element()
}

fn filter_bar(app: AppState, session: Id, current: String) -> AnyElement {
    let draft = signal(current);
    let enter = session.clone();
    let height = match app.form {
        Form::Terminal => term::rows(1.0),
        Form::Desktop => px(30.0),
        Form::Phone => px(44.0),
    };
    let search = copyable_input(app, None, move || draft.get())
        .test_id("field-session-files")
        .label("Filter filenames in this folder")
        .placeholder("Filter filenames")
        .on_change(move |typed: &str| draft.set(typed.to_owned()))
        .on_submit(move |typed: &str| {
            run(
                app,
                Action::Files(FilesAction::Filter {
                    session: enter.clone(),
                    text: typed.to_owned(),
                }),
            )
        })
        .flex_1()
        .min_w(px(0.0))
        .h(height)
        .text_color(color(Token::Ink));
    view()
        .flex_none()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .px(px(10.0))
        .child(search)
        .child(button(
            app,
            "Apply",
            ButtonKind::Quiet,
            "btn-files-filter",
            move || {
                run(
                    app,
                    Action::Files(FilesAction::Filter {
                        session: session.clone(),
                        text: draft.get_untracked(),
                    }),
                );
            },
        ))
        .into_element()
}

fn draw(app: AppState, files: &FilesView, changes: bool, back: Action) -> AnyElement {
    let mut bar = view()
        .flex_none()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(6.0));
    if changes || app.form == Form::Terminal {
        bar = bar.child(button(
            app,
            if changes {
                "‹ Changes"
            } else {
                "‹ Session"
            },
            ButtonKind::Quiet,
            "btn-files-back",
            move || run(app, back.clone()),
        ));
    }
    if let Some(up) = files.up.clone() {
        bar = bar.child(button(
            app,
            "↑ Parent folder",
            ButtonKind::Quiet,
            "btn-files-up",
            move || run(app, up.clone()),
        ));
    }
    let path = if files.path.is_empty() {
        "Worktree".into()
    } else {
        files.path.clone()
    };
    let body = view()
        .flex_col()
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(bar)
        .child(
            text(path)
                .test_id("session-files-path")
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink)),
        )
        .child(tabs::part(app, &files.part, "files"))
        .children(files.content.as_ref().map(|content| drawn(app, content)));
    tabs::surface(app, "session-files", body.into_element())
}

fn drawn(app: AppState, content: &FilesContent) -> AnyElement {
    match content {
        FilesContent::Listing { rows, truncated } => view()
            .flex_col()
            .children(
                (*truncated)
                    .then(|| note("Directory scan limit reached. Some entries are not shown.")),
            )
            .children(
                rows.is_empty()
                    .then(|| note("No browsable files in this folder.")),
            )
            .children(rows.iter().enumerate().map(|(index, row)| {
                let label = if row.directory {
                    format!("{}/", row.name)
                } else {
                    row.name.clone()
                };
                let action = row.open.clone();
                button(
                    app,
                    label,
                    ButtonKind::Quiet,
                    format!("file-entry-{index}"),
                    move || run(app, action.clone()),
                )
            }))
            .into_element(),
        FilesContent::Source {
            text,
            html,
            can_open,
        } => {
            let mut column = view().flex_col();
            if *html {
                column = column.child(note("HTML source · not rendered here"));
                column = column.children(browser_button(app, text, *can_open));
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

#[cfg(not(target_arch = "wasm32"))]
#[path = "files_native.rs"]
mod native;
#[cfg(not(target_arch = "wasm32"))]
use native::browser_button;

#[cfg(target_arch = "wasm32")]
fn browser_button(app: AppState, source: &str, current: bool) -> Option<AnyElement> {
    if !current {
        return None;
    }
    let source = source.to_owned();
    Some(view().flex_col()
        .child(note("Opens a sandboxed copy. Inline scripts can run and may navigate the frame to external sites. Open only files you trust."))
        .child(button(app, "Open in browser", ButtonKind::Ghost, "btn-open-html-browser", move || {
            if open_browser(&source).is_err() {
                app.toast("Could not open the HTML preview in the browser.");
            }
        })).into_element())
}

fn sandboxed_document(source: &str) -> String {
    // Escaping the attribute is distinct from sandboxing its separately parsed document. The
    // outer blob contains no author HTML and the child has an opaque, isolated origin.
    let escaped = source
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        concat!(
            "<!doctype html><meta charset=utf-8><title>Worktree HTML preview</title>",
            "<style>body{{margin:0}}iframe{{display:block;border:0;width:100vw;height:100vh}}</style>",
            "<iframe title=\"Worktree HTML preview\" sandbox=\"allow-scripts\" referrerpolicy=\"no-referrer\" srcdoc=\"",
            "&lt;!doctype html&gt;&lt;meta charset=utf-8&gt;",
            "&lt;meta http-equiv='Content-Security-Policy' content=&quot;default-src 'none'; ",
            "script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:; base-uri 'none'; form-action 'none'&quot;&gt;",
            "{}\"></iframe>"
        ),
        escaped
    )
}

#[cfg(target_arch = "wasm32")]
fn open_browser(source: &str) -> Result<(), wasm_bindgen::JsValue> {
    use wasm_bindgen::{JsCast as _, JsValue, closure::Closure};
    use web_sys::{Blob, BlobPropertyBag, Url};

    let window = web_sys::window().ok_or_else(|| JsValue::from_str("missing browser window"))?;
    let body = js_sys::Array::new();
    body.push(&JsValue::from_str(&sandboxed_document(source)));
    let props = BlobPropertyBag::new();
    props.set_type("text/html;charset=utf-8");
    let blob = Blob::new_with_str_sequence_and_options(&body, &props)?;
    let url = Url::create_object_url_with_blob(&blob)?;
    if let Err(error) =
        window.open_with_url_and_target_and_features(&url, "_blank", "noopener,noreferrer")
    {
        let _ = Url::revoke_object_url(&url);
        return Err(error);
    }
    let retire = url.clone();
    let revoke = Closure::once_into_js(move || {
        let _ = Url::revoke_object_url(&retire);
    });
    if let Err(error) =
        window.set_timeout_with_callback_and_timeout_and_arguments_0(revoke.unchecked_ref(), 60_000)
    {
        let _ = Url::revoke_object_url(&url);
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_cannot_escape_into_the_opened_tabs_origin() {
        let source = "\"></iframe><script>bad()</script>&quot;";
        let wrapped = sandboxed_document(source);
        assert!(wrapped.contains("sandbox=\"allow-scripts\""));
        assert!(!wrapped.contains("allow-same-origin"));
        assert_eq!(wrapped.matches("</iframe>").count(), 1);
        assert!(!wrapped.contains("<script>"));
        assert!(wrapped.contains("&amp;quot;"));
    }
}
