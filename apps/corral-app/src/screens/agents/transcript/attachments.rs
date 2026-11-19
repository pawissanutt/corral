//! A message's attachments in the transcript: a chip each in its header (an image or file opens
//! its canonical readback under the message; a link is never opened or fetched by the app), and
//! the opened one as corrald serves it: an image drawn from its sniffed bytes, text as plain
//! words, never markup.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::assistant::{AttachmentView, FileChip, Readback};
use crate::model::hue::Hue;
use crate::state::AppState;
use crate::ui::kit::chip::chip;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::text::dim;
use crate::ui::tokens::{FONT_MONO, Token, color};

use crate::ui::ask::act;

/// The widest an attached image shows.
const IMAGE_MAX: f32 = 480.0;
/// An image's frame before it has loaded and told its own proportions.
const IMAGE_RATIO: f32 = 4.0 / 3.0;
/// The tallest a text attachment shows before it scrolls.
const TEXT_MAX: f32 = 320.0;

/// The header chips of a message's attachments.
pub(crate) fn chips(app: AppState, files: &[FileChip]) -> Vec<AnyElement> {
    files
        .iter()
        .enumerate()
        .map(|(at, file)| file_chip(app, at, file))
        .collect()
}

fn file_chip(app: AppState, at: usize, file: &FileChip) -> AnyElement {
    let hue = if file.opened {
        Hue::Corral
    } else {
        Hue::Neutral
    };
    let face = chip(app, file.label.clone(), hue);
    let Some(action) = file.open.clone() else {
        return face;
    };
    let spoken = if file.opened {
        format!("Hide {}", file.label)
    } else {
        format!("Show {}", file.label)
    };
    view()
        .test_id(&format!("btn-attachment-{at}"))
        .role(AccessibilityRole::Button)
        .label(spoken)
        .on_press(move || act(app, action.clone()))
        .cursor_pointer()
        .rounded(px(4.0))
        .focus_visible(|s| {
            s.outline(px(2.0))
                .outline_color(color(Token::Focus))
                .outline_offset(px(1.0))
        })
        .child(face)
        .into_element()
}

/// The opened attachment under the message's words.
pub(crate) fn readback(app: AppState, opened: &AttachmentView) -> AnyElement {
    let form = app.form;
    let body = match &opened.body {
        Readback::Loading => dim(app, "Loading from corrald…").into_element(),
        Readback::Failed(line) => dim(app, line.clone())
            .text_color(hue_ink(Hue::Fail))
            .into_element(),
        Readback::Unsupported(line) => dim(app, *line).into_element(),
        Readback::Image { .. } if form == Form::Terminal => dim(
            app,
            "A terminal cannot draw images; open this thread in the desktop or web app to see it.",
        )
        .into_element(),
        Readback::Image { uri, truncated } => view()
            .gap(px(4.0))
            .child(picture(opened.title.clone(), uri.clone()))
            .children(truncated.then(|| cut_note(app)))
            .into_element(),
        Readback::Text {
            text: words,
            truncated,
        } => view()
            .gap(px(4.0))
            .child(plain_text(app, words.clone()))
            .children(truncated.then(|| cut_note(app)))
            .into_element(),
    };
    view()
        .test_id("attachment-readback")
        .gap(px(4.0))
        .child(dim(app, format!("{} · as corrald stored it", opened.title)))
        .child(body)
        .into_element()
}

fn cut_note(app: AppState) -> AnyElement {
    dim(
        app,
        "corrald stops this attachment at its size bound; the rest is not served.",
    )
    .into_element()
}

/// An attached image at its own proportions, on a hairline frame.
fn picture(title: String, uri: String) -> AnyElement {
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
                .label(title)
                .on_load(move |size: &Size<DevicePixels>| {
                    if size.width.0 > 0 && size.height.0 > 0 {
                        ratio.set(size.width.0 as f32 / size.height.0 as f32);
                    }
                }),
        )
        .into_element()
}

/// A text attachment as plain words in the mono face, scrolling past its height.
fn plain_text(app: AppState, words: String) -> AnyElement {
    let body = text(words).text_color(color(Token::Ink));
    if app.form == Form::Terminal {
        return view().child(body).into_element();
    }
    scroll()
        .max_h(px(TEXT_MAX))
        .rounded(px(6.0))
        .border_1()
        .border_color(color(Token::Rule))
        .bg(color(Token::BgSunk))
        .child(
            view().px(px(10.0)).py(px(8.0)).child(
                body.font_family(FONT_MONO)
                    .text_size(px(11.5))
                    .line_height(px(17.0)),
            ),
        )
        .into_element()
}
