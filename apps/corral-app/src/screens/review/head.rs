//! The candidate's head (`.dhead`): its state chip, ids, project and size on one wrapping line,
//! the title, a page notice (loading, stale, failed, superseded), then the Brief | Diff | Gate
//! log tabs; and the diff layout control above the Diff tab's files.

use nsh_core::prelude::*;

use crate::model::review::{Action, CandidateHeader, Layout, Notice, Tab};
use crate::model::{Form, Hue, span};
use crate::state::AppState;
use crate::ui::kit::button::seg;
use crate::ui::kit::chip::chip;
use crate::ui::kit::glance::stat;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::notice::notice;
use crate::ui::kit::text::mono;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::{Ctx, act, command};

/// The title's longest line (`.dhead h2`, about 52 characters at 19 pt).
const TITLE_MAX: f32 = 560.0;

/// The shown candidate's head, rebuilt when its identity, size or notice changes; the tabs
/// follow their own state inside it. `back` leads it where the candidate stands in the
/// outline's place. Nothing while no candidate is shown.
pub(super) fn head(cx: &Ctx, back: Option<AnyElement>) -> AnyElement {
    let app = cx.app;
    let page = cx.page;
    let shown = computed(move || {
        page.with(|page| {
            page.detail
                .as_ref()
                .map(|detail| (detail.header.clone(), detail.notice.clone()))
        })
    });
    let identity = {
        let cx = cx.clone();
        dynamic(move || {
            let (header, line) = shown.get()?;
            Some(
                view()
                    .child(meta(app, &header))
                    .child(title(app, header.title.clone()))
                    .children(line.as_ref().map(|line| notice_row(&cx, line)))
                    .into_element(),
            )
        })
    };
    let absent = move || shown.with(Option::is_none);
    let frame = view()
        .flex_none()
        .style_when(absent, |s| s.hidden())
        .children(back.map(|back| {
            view()
                .flex_row()
                .when(app.form == Form::Desktop, |row| {
                    row.px(px(12.0)).pt(px(8.0))
                })
                .when(app.form == Form::Terminal, |row| row.px(term::cells(1.0)))
                .child(back)
                .into_element()
        }));
    match app.form {
        Form::Desktop => frame
            .child(
                view()
                    .px(px(20.0))
                    .pt(px(16.0))
                    .pb(px(14.0))
                    .border_b(px(1.0))
                    .border_color(color(Token::Rule))
                    .child(identity)
                    .child(view().flex_row().mt(px(12.0)).child(tabs(cx))),
            )
            .into_element(),
        Form::Phone => frame
            .child(
                view()
                    .px(px(18.0))
                    .pt(px(14.0))
                    .pb(px(12.0))
                    .border_b(px(1.0))
                    .border_color(color(Token::Rule))
                    .child(identity)
                    .child(view().mt(px(12.0)).child(tabs(cx))),
            )
            .into_element(),
        Form::Terminal => frame
            .child(
                view()
                    .px(term::cells(1.0))
                    .child(identity)
                    .child(view().flex_row().child(tabs(cx))),
            )
            .child(term::hrule())
            .into_element(),
    }
}

/// `.dhead .r1`: the state chip, the task, its round and the candidate's SHA in mono, the
/// project, then the files count and the added and deleted lines once the detail tells them.
fn meta(app: AppState, header: &CandidateHeader) -> AnyElement {
    let quiet = |words: String| {
        mono(app, words)
            .text_color(color(Token::Ink3))
            .into_element()
    };
    let mut parts = vec![chip(app, header.state.label.clone(), header.state.hue)];
    parts.push(quiet(header.task.clone()));
    parts.extend(header.round.clone().map(quiet));
    parts.extend(header.sha7.clone().map(quiet));
    parts.extend(
        header
            .project_chip
            .clone()
            .map(|project| chip(app, project, Hue::Neutral)),
    );
    parts.extend(header.files.as_ref().map(|files| stat(app, files)));
    let tinted = |words: &String, hue: Hue| {
        mono(app, words.clone())
            .text_color(hue_ink(hue))
            .into_element()
    };
    parts.extend(header.added.as_ref().map(|added| tinted(added, Hue::Ok)));
    parts.extend(
        header
            .deleted
            .as_ref()
            .map(|deleted| tinted(deleted, Hue::Fail)),
    );
    let row = view().flex_row().flex_wrap().items_center();
    match app.form {
        Form::Terminal => row
            .children(parts.into_iter().map(|part| {
                view()
                    .flex_row()
                    .flex_shrink_0()
                    .child(part)
                    .child(term::space(1.0))
                    .into_element()
            }))
            .into_element(),
        Form::Desktop | Form::Phone => row
            .gap_x(px(8.0))
            .gap_y(px(4.0))
            .children(parts)
            .into_element(),
    }
}

/// The candidate's title (`.dhead h2`): 19 pt semibold on desktop, 20 pt bold on a phone,
/// wrapping; bold in a terminal.
fn title(app: AppState, words: String) -> AnyElement {
    let title = text(words)
        .role(AccessibilityRole::Header)
        .font_weight(if app.form == Form::Desktop {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::BOLD
        })
        .text_color(color(Token::Ink));
    match app.form {
        Form::Terminal => title.into_element(),
        Form::Desktop => title
            .max_w(px(TITLE_MAX))
            .mt(px(6.0))
            .font_family(FONT_TEXT)
            .text_size(px(19.0))
            .line_height(px(25.0))
            .letter_spacing(px(-0.19))
            .into_element(),
        Form::Phone => title
            .mt(px(6.0))
            .font_family(FONT_TEXT)
            .text_size(px(20.0))
            .line_height(px(25.0))
            .into_element(),
    }
}

/// A page notice (loading, stale, failed, superseded) with its refresh control.
fn notice_row(cx: &Ctx, line: &Notice) -> AnyElement {
    let app = cx.app;
    let action = line.action.as_ref().map(|button| command(cx, button));
    view()
        .when(app.form != Form::Terminal, |place| place.mt(px(10.0)))
        .child(notice(
            app,
            line.hue,
            &vec![span(line.text.clone())],
            action,
        ))
        .into_element()
}

/// The model's tabs as a segmented control (`.seg.tabs`), `seg-review`, rebuilt only when the
/// tabs on offer change; which one is pressed follows the model.
fn tabs(cx: &Ctx) -> AnyElement {
    let app = cx.app;
    let page = cx.page;
    let tabs = computed(move || {
        page.with(|page| {
            page.detail
                .as_ref()
                .map(|detail| detail.tabs.clone())
                .unwrap_or_default()
        })
    });
    let offered = computed(move || {
        tabs.with(|tabs| {
            tabs.iter()
                .map(|tab| (Some(tab.tab), tab.label))
                .collect::<Vec<(Option<Tab>, &'static str)>>()
        })
    });
    let cx = cx.clone();
    dynamic(move || {
        let options = offered.get();
        if options.is_empty() {
            return None;
        }
        let cx = cx.clone();
        Some(seg(
            app,
            &options,
            move || tabs.with(|tabs| tabs.iter().find(|tab| tab.selected).map(|tab| tab.tab)),
            move |choice: Option<Tab>| {
                if let Some(tab) = choice {
                    act(&cx, Action::Tab(tab))();
                }
            },
            "seg-review",
        ))
    })
}

/// The Unified | Side by side control (`.dtools`), `seg-layout`, offered with the Diff tab only
/// where the candidate's column is wide enough for side by side (the model's rule): narrower,
/// the diff is unified and a control whose other choice could not show would mislead.
pub(super) fn layout_switch(cx: &Ctx) -> AnyElement {
    let app = cx.app;
    let page = cx.page;
    let split = cx.split;
    let choices = computed(move || {
        if split.get() != Some(true) {
            return Vec::new();
        }
        page.with(|page| {
            page.detail
                .as_ref()
                .map(|detail| {
                    detail
                        .layouts
                        .iter()
                        .map(|choice| (Some(choice.layout), choice.label))
                        .collect::<Vec<(Option<Layout>, &'static str)>>()
                })
                .unwrap_or_default()
        })
    });
    let cx = cx.clone();
    dynamic(move || {
        let options = choices.get();
        if options.is_empty() {
            return None;
        }
        let cx = cx.clone();
        Some(
            view()
                .flex_row()
                .pb(px(12.0))
                .child(seg(
                    app,
                    &options,
                    move || app.review.with(|state| Some(state.layout)),
                    move |choice: Option<Layout>| {
                        if let Some(layout) = choice {
                            act(&cx, Action::Layout(layout))();
                        }
                    },
                    "seg-layout",
                ))
                .into_element(),
        )
    })
}
