//! An artifact as the finder shows it: beside the results, its title, one line of facts, Open
//! full size and Open its task over the first lines of its content; full size, the facts as a
//! table and all of its content, with the way back to the results. Markdown is laid out as the
//! transcripts lay it out, a diff and a log line by line in their colours, other text as it was
//! written, an image at its own proportions where the form draws one; anything else shows why
//! only its facts are shown.

use nsh_core::prelude::*;

use super::browser::{self, Opens};
use super::save::{self, Downloads};
use crate::model::Form;
use crate::model::artifacts::browser::VIEW_SOURCE;
use crate::model::artifacts::diagram::Graph;
use crate::model::artifacts::{
    Action, BACK, Body, Content, LOADING_CONTENT, Line, OPEN_FULL, PREVIEW_LABEL, Page, Preview,
};
use crate::screens::markdown::{artifact_markdown, provide_styles};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::page::{body, measure, measured_head};
use crate::ui::kit::table::kv;
use crate::ui::kit::text::{dim, mono, plain};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, MEASURE_RECORD, Token, color};

/// A code or log line's size and leading on desktop and phone.
const CODE: (f32, f32) = (11.5, 17.0);
/// An image's frame before it has loaded and told its own proportions.
const IMAGE_RATIO: f32 = 16.0 / 9.0;
/// The widest an image is drawn.
const IMAGE_MAX: f32 = 720.0;
/// The measure of Markdown and plain text on desktop.
const MEASURE: f32 = 680.0;
/// The column a desktop preview keeps to, centred in its pane: an image's widest, so the title
/// and buttons stand over the words or the picture rather than across an empty pane. Code and
/// logs scroll sideways within it.
const PREVIEW_MEASURE: f32 = IMAGE_MAX;

/// The selected artifact beside or under the results: test ids "artifact-preview",
/// "btn-open-full", "btn-open-task".
pub(super) fn preview(
    app: AppState,
    page: Memo<Page>,
    downloads: Downloads,
    opens: Opens,
) -> AnyElement {
    let shown = computed(move || page.with(|page| page.preview.clone().map(bare)));
    let graph = computed(move || page.with(|page| graph_of(page.preview.as_ref())));
    let terminal = app.form == Form::Terminal;
    view()
        .test_id("artifact-preview")
        .role(AccessibilityRole::Region)
        .label(PREVIEW_LABEL)
        .child(dynamic(move || {
            let Some((preview, drawn)) = shown.get() else {
                return view().into_element();
            };
            untracked(|| {
                let open = preview.reference.clone();
                // An HTML page's full size is its source; Open in browser shows it.
                let full = button(
                    app,
                    if preview.html { VIEW_SOURCE } else { OPEN_FULL },
                    ButtonKind::Primary,
                    "btn-open-full",
                    move || app.artifacts_act(Action::Open(open.clone())),
                );
                let shown = view()
                    .gap(px(if terminal { 0.0 } else { 8.0 }))
                    .child(
                        plain(app, preview.title.clone())
                            .font_weight(FontWeight::BOLD)
                            .number_of_lines(2u32),
                    )
                    .child(dim(app, preview.summary.clone()))
                    .when(terminal, |part| part.child(term::blank()))
                    .child(actions(app, &preview, full, downloads, opens))
                    .child(save::note(app, downloads, &preview.save.reference))
                    .children(html_note(app, &preview, opens))
                    .when(terminal, |part| part.child(term::blank()))
                    .child(body_of(app, &preview, drawn, graph));
                measure(app, PREVIEW_MEASURE, shown.into_element())
            })
        }))
        .into_element()
}

/// What opening an HTML page in the browser does or where it stands; none for other content.
fn html_note(app: AppState, preview: &Preview, opens: Opens) -> Option<AnyElement> {
    preview
        .html
        .then(|| browser::note(app, opens, &preview.save.reference))
}

/// Open full size (or Back), Open in browser for an HTML page, Download and Open its task, in
/// a row.
fn actions(
    app: AppState,
    preview: &Preview,
    first: AnyElement,
    downloads: Downloads,
    opens: Opens,
) -> AnyElement {
    let browser = preview
        .html
        .then(|| browser::control(app, opens, &preview.save))
        .flatten();
    let task = preview.task.clone();
    let open_task = preview.open_task.clone().map(|label| {
        button_with(
            app,
            label,
            ButtonKind::Ghost,
            ButtonOptions::default(),
            "btn-open-task",
            move || app.artifacts_act(Action::OpenTask(task.clone())),
        )
    });
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(if app.form == Form::Terminal {
            16.0
        } else {
            8.0
        }))
        .child(first)
        .children(browser)
        .child(save::control(app, downloads, &preview.save))
        .children(open_task)
        .into_element()
}

/// The artifact full size in the results' place: its title and reference in the head with Back
/// and Open its task, then its facts and all of its content, kept to the record's measure on a
/// desktop, centred. test ids "artifact-full", "btn-artifact-back", "btn-open-task".
pub(super) fn full(
    app: AppState,
    page: Memo<Page>,
    downloads: Downloads,
    opens: Opens,
) -> AnyElement {
    let shown = computed(move || page.with(|page| page.full.clone().map(bare)));
    let graph = computed(move || page.with(|page| graph_of(page.full.as_ref())));
    dynamic(move || {
        let Some((preview, drawn)) = shown.get() else {
            return view().into_element();
        };
        untracked(|| {
            let back = button_with(
                app,
                BACK,
                ButtonKind::Ghost,
                ButtonOptions {
                    key_hint: Some("Esc"),
                    ..ButtonOptions::default()
                },
                "btn-artifact-back",
                move || app.artifacts_act(Action::Close),
            );
            let terminal = app.form == Form::Terminal;
            let facts = kv(
                app,
                preview
                    .facts
                    .iter()
                    .map(|(label, value)| {
                        ((*label).to_owned(), mono(app, value.clone()).into_element())
                    })
                    .collect(),
            );
            let scroll = view()
                .gap(px(if terminal { 0.0 } else { 14.0 }))
                .child(actions(app, &preview, back, downloads, opens))
                .child(save::note(app, downloads, &preview.save.reference))
                .children(html_note(app, &preview, opens))
                .when(terminal, |part| part.child(term::blank()))
                .child(facts)
                .when(terminal, |part| part.child(term::blank()))
                .child(body_of(app, &preview, drawn, graph))
                .into_element();
            view()
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .child(measured_head(
                    app,
                    MEASURE_RECORD,
                    preview.title.clone(),
                    Some(preview.reference.clone()),
                    Vec::new(),
                ))
                .child(body(
                    app,
                    "artifact-full",
                    measure(app, MEASURE_RECORD, scroll),
                ))
                .into_element()
        })
    })
    .into_element()
}

/// The preview without its diagram, and whether it has one: a selection moving through the
/// diagram changes only the diagram, so the page around it (and its WebView) is not rebuilt.
fn bare(mut preview: Preview) -> (Preview, bool) {
    let drawn = preview.diagram.take().is_some();
    preview.diagram_read = None;
    (preview, drawn)
}

fn graph_of(preview: Option<&Preview>) -> Option<Graph> {
    preview.and_then(|preview| preview.diagram.clone())
}

/// A workflow recipe corrald served as one is drawn as its diagram; anything else as its content.
fn body_of(
    app: AppState,
    preview: &Preview,
    drawn: bool,
    graph: Memo<Option<Graph>>,
) -> AnyElement {
    if drawn {
        super::diagram::diagram(app, graph)
    } else {
        content(app, &preview.content)
    }
}

/// The content as its kind is drawn, then what is left out of it.
fn content(app: AppState, content: &Content) -> AnyElement {
    let form = app.form;
    let drawn = match &content.body {
        Body::Loading => dim(app, LOADING_CONTENT).into_element(),
        Body::Failed(why) => dim(app, why.clone())
            .text_color(hue_ink(crate::model::Hue::Fail))
            .into_element(),
        Body::Facts(why) => dim(app, *why).into_element(),
        Body::Markdown(words) => {
            provide_styles(app);
            view()
                .when(form == Form::Desktop, |doc| doc.max_w(px(MEASURE)))
                .child(artifact_markdown(app, words))
                .into_element()
        }
        Body::Diff(lines) | Body::Log(lines) => code(app, lines),
        Body::Text(words) => code(
            app,
            &words
                .lines()
                .map(|line| Line {
                    text: line.to_owned(),
                    hue: crate::model::Hue::Neutral,
                })
                .collect::<Vec<_>>(),
        ),
        Body::Image(uri) => picture(uri.clone()),
    };
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(drawn)
        .children(content.cut.clone().map(|cut| {
            view()
                .when(form == Form::Terminal, |part| part.child(term::blank()))
                .child(dim(app, cut))
                .into_element()
        }))
        .into_element()
}

/// Lines in mono, each in its colour: on the sunk ground with a hairline on desktop and phone,
/// scrolling sideways rather than wrapping a long line; a terminal sets them as they are.
fn code(app: AppState, lines: &[Line]) -> AnyElement {
    let mut spans = Vec::with_capacity(lines.len() * 2);
    for (at, line) in lines.iter().enumerate() {
        if at > 0 {
            spans.push(text("\n"));
        }
        let ink = match line.hue {
            crate::model::Hue::Neutral => color(Token::Ink),
            hue => hue_ink(hue),
        };
        spans.push(text(line.text.clone()).text_color(ink));
    }
    let words = text("").text_color(color(Token::Ink)).children(spans);
    if app.form == Form::Terminal {
        return words.into_element();
    }
    view()
        .rounded(px(6.0))
        .border_1()
        .border_color(color(Token::Rule))
        .bg(color(Token::BgSunk))
        .overflow_hidden()
        .child(
            scroll().horizontal().child(
                view().flex_shrink_0().px(px(12.0)).py(px(10.0)).child(
                    words
                        .flex_shrink_0()
                        .font_family(FONT_MONO)
                        .text_size(px(CODE.0))
                        .line_height(px(CODE.1)),
                ),
            ),
        )
        .into_element()
}

/// An image at its own proportions, at most [`IMAGE_MAX`] wide, on a hairline frame.
fn picture(uri: String) -> AnyElement {
    let ratio = signal(IMAGE_RATIO);
    view()
        .max_w(px(IMAGE_MAX))
        .rounded(px(6.0))
        .border_1()
        .border_color(color(Token::Rule))
        .bg(color(Token::BgSunk))
        .overflow_hidden()
        .child(
            image(uri)
                .w_full()
                .aspect_ratio(move || ratio.get())
                .object_fit(ResizeMode::Contain)
                .role(AccessibilityRole::Image)
                .on_load(move |size: &Size<DevicePixels>| {
                    if size.width.0 > 0 && size.height.0 > 0 {
                        ratio.set(size.width.0 as f32 / size.height.0 as f32);
                    }
                }),
        )
        .into_element()
}
