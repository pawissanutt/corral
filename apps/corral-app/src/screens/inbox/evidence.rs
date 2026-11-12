//! Assessment: the evidence tabs and the selected artifact (the diff hunk with line numbers, the
//! file excerpt, the gate log tail, the rendered screenshot), and their loading and error looks.

use nsh_core::prelude::*;

use super::{button, code, press, rich_at, ring, words};
use crate::model::board::LineKind;
use crate::model::inbox::{Assessment, DiffLine, Evidence, EvidenceBody, EvidenceTab};
use crate::model::{Form, Hue, Rich};
use crate::state::AppState;
use crate::ui::kit::chip::illus;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::notice::{notice, skeleton};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, Token, color};

/// The evidence box's height before it scrolls (`.win .evbox .diff`, `.term`: 124 pt; the
/// phone's 196 pt), the hunk's header row included.
const BOX_DESKTOP: f32 = 124.0;
const BOX_PHONE: f32 = 196.0;
/// The streaming placeholder (`.skel` 120 pt high).
const LOADING_BAR: (f32, f32) = (1.0, 120.0);

pub(super) fn assessment(app: AppState, part: Assessment) -> AnyElement {
    match part {
        Assessment::Loading {
            skeleton_count,
            note,
        } => view()
            .label("Loading")
            .gap(px(6.0))
            .child(skeleton(
                app,
                &vec![LOADING_BAR; usize::from(skeleton_count)],
            ))
            .child(illus(app, note))
            .into_element(),
        Assessment::Error(failure) => notice(
            app,
            failure.hue,
            &failure.body,
            failure.retry.map(|retry| button(app, &retry)),
        ),
        Assessment::Ready { label, tabs, body } => view()
            .child(tab_row(app, label, tabs))
            .child(artifact(app, body))
            .into_element(),
    }
}

fn evidence_id(evidence: Evidence) -> &'static str {
    match evidence {
        Evidence::Diff => "seg-evidence-diff",
        Evidence::File => "seg-evidence-file",
        Evidence::Log => "seg-evidence-log",
        Evidence::Shot => "seg-evidence-shot",
    }
}

/// The evidence toolbar (`.ev-tabs`): small outlined toggles, the pressed one on surface-2 with a
/// rule-2 edge in semibold ink. Phone: 44 pt targets. Terminal: the labels two Cells apart, the
/// pressed one bold.
fn tab_row(app: AppState, label: &'static str, tabs: Vec<EvidenceTab>) -> AnyElement {
    let form = app.form;
    let row = view()
        .role(AccessibilityRole::Toolbar)
        .label(label)
        .flex_row()
        .flex_wrap();
    let row = match form {
        Form::Terminal => row,
        Form::Desktop | Form::Phone => row.gap(px(6.0)).mb(px(10.0)),
    };
    let mut items = Vec::with_capacity(tabs.len() * 2);
    for (at, tab) in tabs.into_iter().enumerate() {
        if at > 0 && form == Form::Terminal {
            items.push(term::space(2.0));
        }
        items.push(tab_button(app, tab));
    }
    row.children(items).into_element()
}

fn tab_button(app: AppState, tab: EvidenceTab) -> AnyElement {
    let form = app.form;
    let on = tab.selected;
    let words_of = words(
        app,
        tab.label,
        if form == Form::Phone { 13.5 } else { 12.0 },
        if on || form == Form::Terminal {
            Token::Ink
        } else {
            Token::Ink3
        },
    )
    .number_of_lines(1u32)
    .when(on, |t| t.font_weight(FontWeight::SEMIBOLD));
    let item = view()
        .flex_row()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(tab.label)
        .accessibility_state(AccessibilityState {
            pressed: Some(if on {
                nsh_core::Checked::True
            } else {
                nsh_core::Checked::False
            }),
            ..AccessibilityState::default()
        })
        .test_id(evidence_id(tab.evidence))
        .on_press(press(app, tab.action.clone()));
    let item = match form {
        Form::Terminal => item.when(on, |v| v.child(text("▸").text_color(color(Token::Ink)))),
        Form::Desktop | Form::Phone => {
            let phone = form == Form::Phone;
            item.h(px(if phone { 44.0 } else { 24.0 }))
                .px(px(if phone { 12.0 } else { 9.0 }))
                .rounded(px(if phone { 8.0 } else { 5.0 }))
                .border_1()
                .border_color(color(if on { Token::Rule2 } else { Token::Rule }))
                .when(on, |v| v.bg(color(Token::Surface2)))
                .when(!on, |v| v.hover(|s| s.bg(color(Token::Hover))))
                .focus_visible(ring)
        }
    };
    item.child(words_of).into_element()
}

/// The selected artifact under its file line.
fn artifact(app: AppState, body: EvidenceBody) -> AnyElement {
    let form = app.form;
    let note_size = if form == Form::Phone { 13.0 } else { 11.5 };
    match body {
        EvidenceBody::Diff { hunk, lines, note } => view()
            .child(file_line(app, hunk.file, None))
            .child(diff(app, hunk.at, lines))
            .child(
                view()
                    .mt(px(6.0))
                    .child(rich_at(app, &note, note_size, color(Token::Ink3))),
            )
            .into_element(),
        EvidenceBody::Text {
            file,
            attribution,
            lines,
        } => view()
            .child(file_line(app, file, attribution))
            .child(terminal_box(app, &lines, false, None))
            .into_element(),
        EvidenceBody::Shot {
            file,
            attribution,
            lines,
            cursor,
            note,
        } => view()
            .child(file_line(app, file, Some(attribution)))
            .child(terminal_box(app, &lines, cursor, Some(420.0)))
            .child(
                view()
                    .mt(px(6.0))
                    .child(words(app, note, note_size, Token::Ink3)),
            )
            .into_element(),
    }
}

/// The artifact's path in mono ink-3 (`.ev-file`), cut at the end, its illustrative tag after.
fn file_line(app: AppState, file: &'static str, attribution: Option<&'static str>) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .when(app.form != Form::Terminal, |v| v.mb(px(6.0)))
        .child(
            code(app, file, 11.0, Token::Ink3)
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .truncate(),
        )
        .children(attribution.map(|tag| illus(app, tag)))
        .into_element()
}

/// The hunk (`.diff`): its header on the sunken ground, then numbered lines in mono, additions
/// and deletions on the ok and failure grounds with their sign in that colour. The phone drops
/// the old-number column; a terminal shows the unified lines, signs and code in their colour.
fn diff(app: AppState, at: &'static str, lines: Vec<DiffLine>) -> AnyElement {
    let form = app.form;
    if form == Form::Terminal {
        return view()
            .child(term::quiet(at).truncate())
            .children(lines.into_iter().map(|line| terminal_line(app, line)))
            .into_element();
    }
    let phone = form == Form::Phone;
    let box_height = if phone { BOX_PHONE } else { BOX_DESKTOP };
    view()
        .rounded(px(6.0))
        .overflow_hidden()
        .border_1()
        .border_color(color(Token::Rule))
        .bg(color(Token::Surface))
        .child(
            view()
                .px(px(10.0))
                .py(px(3.0))
                .bg(color(Token::BgSunk))
                .border_b_1()
                .border_b_color(color(Token::Rule))
                .child(code(app, at, 11.5, Token::Ink3).truncate()),
        )
        .child(
            scroll()
                .max_h(px(box_height - 22.0))
                .child(view().children(lines.into_iter().map(|line| diff_line(app, line, phone)))),
        )
        .into_element()
}

fn sign_hue(kind: LineKind) -> Hue {
    match kind {
        LineKind::Add => Hue::Ok,
        LineKind::Del => Hue::Fail,
        LineKind::Context => Hue::Neutral,
    }
}

fn diff_line(app: AppState, line: DiffLine, phone: bool) -> AnyElement {
    let size = if phone { 11.0 } else { 11.5 };
    let height = (size * 1.6_f32).round();
    let number = |n: Option<u32>| {
        view()
            .w(px(38.0))
            .flex_shrink_0()
            .pr(px(8.0))
            .items_end()
            .accessibility_elements_hidden(true)
            .child(code(
                app,
                n.map(|n| n.to_string()).unwrap_or_default(),
                size,
                Token::Ink4,
            ))
    };
    let fill = match line.kind {
        LineKind::Add => Some(Token::OkBg),
        LineKind::Del => Some(Token::FailBg),
        LineKind::Context => None,
    };
    let sign_ink = match sign_hue(line.kind) {
        Hue::Neutral => color(Token::Ink4),
        hue => hue_ink(hue),
    };
    view()
        .flex_row()
        .items_center()
        .h(px(height))
        .when_some(fill, |v, fill| v.bg(color(fill)))
        .when(!phone, |v| v.child(number(line.old)))
        .child(number(line.new))
        .child(
            view().w(px(14.0)).flex_shrink_0().child(
                text(line.prefix)
                    .font_family(FONT_MONO)
                    .text_size(px(size))
                    .text_color(sign_ink),
            ),
        )
        .child(
            code(app, line.text, size, Token::Ink)
                .flex_1()
                .min_w(px(0.0))
                .pr(px(10.0))
                .truncate(),
        )
        .into_element()
}

/// A unified line in Cells: old and new numbers (five Cells each), the sign, the code; an
/// addition or a deletion in its colour, as a terminal may have no grounds for it.
fn terminal_line(app: AppState, line: DiffLine) -> AnyElement {
    let ink = match sign_hue(line.kind) {
        Hue::Neutral => color(Token::Ink),
        hue => hue_ink(hue),
    };
    let number = |n: Option<u32>| {
        view()
            .w(term::cells(5.0))
            .flex_shrink_0()
            .items_end()
            .accessibility_elements_hidden(true)
            .child(term::quiet(n.map(|n| n.to_string()).unwrap_or_default()))
    };
    view()
        .flex_row()
        .child(number(line.old))
        .child(number(line.new))
        .child(term::space(1.0))
        .child(
            view()
                .w(term::cells(2.0))
                .flex_shrink_0()
                .child(text(line.prefix).text_color(ink)),
        )
        .child(
            code(app, line.text, 11.5, Token::Ink)
                .text_color(ink)
                .flex_1()
                .min_w(px(0.0))
                .truncate(),
        )
        .into_element()
}

/// The terminal look of an artifact (`.term`): mono lines on the shell's dark ground in both
/// appearances, its ink, faint runs in its second ink, a passing line in the ok colour of the
/// dark appearance. `width` fixes the box (the screenshot's 420 pt); `cursor` ends the last line
/// with a block. A terminal shows the lines as they are, in its own colours.
fn terminal_box(app: AppState, lines: &[Rich], cursor: bool, width: Option<f32>) -> AnyElement {
    let form = app.form;
    if form == Form::Terminal {
        return view()
            .children(lines.iter().map(|line| {
                if line.is_empty() {
                    term::blank()
                } else {
                    rich_at(app, line, 13.0, color(Token::Ink)).into_element()
                }
            }))
            .into_element();
    }
    let phone = form == Form::Phone;
    let screenshot = width.is_some();
    let size = match (screenshot, phone) {
        (true, _) => 10.5,
        (false, true) => 11.0,
        (false, false) => 11.5,
    };
    let line_height = (size * 1.55_f32).round();
    let last = lines.len().saturating_sub(1);
    let rows = lines.iter().enumerate().map(|(at, line)| {
        let row = view()
            .flex_row()
            .items_center()
            .min_h(px(line_height))
            .child(shell_line(line, size, line_height));
        if cursor && at == last {
            row.child(
                view()
                    .w(px(7.0))
                    .h(px(13.0))
                    .bg(color(Token::ShellInk))
                    .accessibility_elements_hidden(true),
            )
            .into_element()
        } else {
            row.into_element()
        }
    });
    let content = view().children(rows.collect::<Vec<_>>());
    let framed = view()
        .px(px(12.0))
        .py(px(10.0))
        .rounded(px(6.0))
        .bg(color(Token::Shell))
        .when_some(width.filter(|_| !phone), |v, width| {
            v.w(px(width)).max_w(relative(1.0))
        });
    let box_height = if phone { BOX_PHONE } else { BOX_DESKTOP };
    framed
        .child(scroll().max_h(px(box_height - 20.0)).child(content))
        .into_element()
}

/// One line of the terminal look: plain runs in the shell ink, faint runs in its second ink,
/// bold runs bold, hued runs in their colour as the dark appearance draws it (the ground is
/// dark in both appearances).
fn shell_line(line: &Rich, size: f32, line_height: f32) -> Text {
    text("")
        .font_family(FONT_MONO)
        .text_size(px(size))
        .line_height(px(line_height))
        .text_color(color(Token::ShellInk))
        .children(line.iter().map(|run| {
            let piece = text(run.text.clone());
            let piece = if run.bold {
                piece.font_weight(FontWeight::BOLD)
            } else {
                piece
            };
            if run.hue != Hue::Neutral {
                let ink = hue_ink(run.hue);
                piece.text_color(AdaptiveColor {
                    light: ink.dark,
                    dark: ink.dark,
                })
            } else if run.dim {
                piece.text_color(color(Token::ShellInk2))
            } else {
                piece
            }
        }))
}
