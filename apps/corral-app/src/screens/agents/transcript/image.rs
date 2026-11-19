//! A recorded image. Desktop and phone draw it at its own proportions on a hairline frame,
//! captioned with its type and size; a press shows it at the transcript's full width and a
//! second press returns it to its place. A terminal names it in one row (`▣ image/png · 34
//! KB`). Until its complete bytes are here it is named, with a control to fetch it.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::session::transcript::Picture;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::tokens::{Token, color};

use super::super::run;
use super::parts::{small, state};

/// The widest a recorded image is drawn in its place on the desktop, and its shape until it
/// has loaded.
const PICTURE_MAX: f32 = 480.0;
const PICTURE_RATIO: f32 = 4.0 / 3.0;

pub(super) fn picture(app: AppState, picture: &Picture, id: &str) -> AnyElement {
    let form = app.form;
    let fetch = picture.action.clone().filter(|_| form != Form::Terminal);
    let control = fetch.map(|action| {
        let size = picture.more.size().unwrap_or_default().to_owned();
        button(
            app,
            format!("Show image · {size}"),
            ButtonKind::Quiet,
            format!("btn-expand-{id}"),
            move || run(app, action.clone()),
        )
    });
    let caption = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(8.0))
        .child(small(app, format!("▣ {}", picture.label), Token::Ink3))
        .children(
            picture
                .note
                .clone()
                .map(|note| small(app, note, Token::Held)),
        );
    let drawn = picture
        .uri
        .clone()
        .filter(|_| form != Form::Terminal)
        .map(|uri| {
            let ratio = signal(PICTURE_RATIO);
            let full = signal(false);
            let label = picture.label.clone();
            let test = format!("btn-image-{id}");
            dynamic(move || {
                let big = full.get();
                let uri = uri.clone();
                view()
                    .test_id(&test)
                    .role(AccessibilityRole::ImageButton)
                    .label(if big {
                        format!("{label}, full size; press to return it to its place")
                    } else {
                        format!("{label}; press to show it full size")
                    })
                    .cursor_pointer()
                    .on_press(move || full.set(!full.get_untracked()))
                    .when(form == Form::Desktop && !big, |frame| {
                        frame.max_w(px(PICTURE_MAX))
                    })
                    .w_full()
                    .mb(px(4.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(color(if big { Token::Rule2 } else { Token::Rule }))
                    .bg(color(Token::BgSunk))
                    .overflow_hidden()
                    .focus_visible(|s| {
                        s.outline(px(2.0))
                            .outline_color(color(Token::Focus))
                            .outline_offset(px(1.0))
                    })
                    .child(
                        image(uri)
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
            })
        });
    view()
        .min_w(px(0.0))
        .when(form != Form::Terminal, |block| block.my(px(4.0)))
        .children(drawn)
        .child(caption)
        .children(control.map(|control| view().flex_row().child(control)))
        .children(state(app, &picture.more, id, false))
        .into_element()
}
