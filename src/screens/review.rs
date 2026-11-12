//! Review: the candidate replay with its SHA-bound verdicts, the evidence tabs and the pinned
//! decide bar. Every word, count and branch comes from `model::review`; this module only lays
//! the model's page out for each form and sends each control's action back through its reducer.
//!
//! Desktop is the mockup's two columns (the 300 pt candidate list, the detail beside it); the
//! detail measures its own width and asks the model for that width, so the diff falls back to
//! unified rows when side by side does not fit. Phone is one scrolling page with the decide bar
//! pinned in place of the tab bar ([`page_phone`]). Terminal is the desktop's columns in Cells,
//! the diff always unified.

mod diff;
mod evidence;

use nsh_core::element::StyleRefinement;
use nsh_core::prelude::*;

use crate::model::board::Mark;
use crate::model::review::{
    self, Action, Body, ButtonStyle, CandidateGroup, CandidateRow, DecideBar, Header, Layout,
    PageView, Tab, VerdictView,
};
use crate::model::{Form, Hue, Rich, Span, flat};
use crate::screens::PhonePage;
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with, seg, tab_strip};
use crate::ui::kit::chip::{chip, illus};
use crate::ui::kit::marks::{dot, hue_ink, mark};
use crate::ui::kit::page::{Width, bar, bar_pane, columns, pane, qhead, section};
use crate::ui::kit::rows::group_header;
use crate::ui::kit::text::{dim, h3, mono};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// The candidate list's column (`grid-template-columns: 300px ...`).
const LIST_WIDTH: f32 = 300.0;
/// `.is-stale .stalefx`: the last known state reads quieter while offline or down.
const STALE_OPACITY: f32 = 0.78;
/// A struck verdict of an older candidate sits a step back (`opacity: .8`).
const OLD_VERDICT_OPACITY: f32 = 0.8;
/// `.mono` is 0.93 of the text around it.
const MONO_SCALE: f32 = 0.93;
/// Width handed to the model where width plays no part (tabs, decide bar, stale state).
const ANY_WIDTH: f32 = f32::INFINITY;

/// The Review destination on every form. Desktop and terminal fill the content container and
/// scroll inside themselves; on a phone this is the page body without its pinned bar (the phone
/// shell takes [`page_phone`], which pins the bar).
pub fn screen(app: AppState) -> AnyElement {
    match app.form {
        Form::Desktop | Form::Terminal => columns_screen(app),
        Form::Phone => phone_body(app),
    }
}

/// Review pushed on the phone: the scrolling body and the decide bar in place of the tab bar.
pub fn page_phone(app: AppState) -> PhonePage {
    let page = untracked(|| page_view(app, 0.0));
    PhonePage {
        title: page.title.to_owned(),
        back: page.back_label.unwrap_or_default(),
        body: phone_body(app),
        trailing: None,
        bar: Some(decide(app)),
        ask: true,
        scroll: true,
    }
}

/// The model's page for this form at `width` (tracked: state, connection and data mode).
fn page_view(app: AppState, width: f32) -> PageView {
    let env = app.env();
    app.review.with(|state| match app.form {
        Form::Phone => review::phone(state, env),
        Form::Desktop | Form::Terminal => review::page_for_width(state, env, width),
    })
}

/// Runs a model action through the Review reducer and the app's effects.
fn act(app: AppState, action: Action) -> impl Fn() + 'static {
    move || app.act(app.review, move |state, env| state.apply(action, env))
}

/// Desktop and terminal: the candidate list beside the detail, which carries the decide bar.
fn columns_screen(app: AppState) -> AnyElement {
    let desktop = app.form == Form::Desktop;
    let page = untracked(|| page_view(app, ANY_WIDTH));
    // A terminal is always unified, so only the desktop waits for its measure.
    let width = signal(if desktop { None } else { Some(ANY_WIDTH) });
    let stale = computed(move || page_view(app, ANY_WIDTH).stale);
    let detail_column = view()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .when(desktop, |v| {
            v.on_layout(move |frame: &Bounds<Points>| width.set(Some(frame.size.width.0)))
        })
        .child(bar_pane(
            app,
            "review-detail",
            detail(app, &page, width),
            decide(app),
        ));
    view()
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .when(desktop, |v| {
            v.opacity(move || if stale.get() { STALE_OPACITY } else { 1.0 })
        })
        .child(columns(
            app,
            vec![
                (
                    Width::Fixed(LIST_WIDTH),
                    pane(app, "review-list", candidates(app, &page)),
                ),
                (Width::Fill, detail_column.into_element()),
            ],
            1,
        ))
        .into_element()
}

/// The phone page body: head, verdicts, the segmented tabs and the selected evidence.
fn phone_body(app: AppState) -> AnyElement {
    let page = untracked(|| page_view(app, 0.0));
    let stale = computed(move || page_view(app, 0.0).stale);
    view()
        .opacity(move || if stale.get() { STALE_OPACITY } else { 1.0 })
        .child(header(app, &page.header))
        .child(verdicts(app, &page))
        .child(tab_row(app, &page))
        .child(evidence_area(app, signal(Some(0.0))))
        .into_element()
}

/// The detail column's scrolling content.
fn detail(app: AppState, page: &PageView, width: Signal<Option<f32>>) -> AnyElement {
    view()
        .child(header(app, &page.header))
        .child(verdicts(app, page))
        .child(tab_row(app, page))
        .child(evidence_area(app, width))
        .into_element()
}

/// The selected candidate's head (`.qhead`): the metadata, the phone's replay tag, the title.
fn header(app: AppState, header: &Header) -> AnyElement {
    let mut meta: Vec<AnyElement> = header
        .metadata
        .iter()
        .map(|run| meta_run(app, run))
        .collect();
    meta.extend(header.chips.iter().map(|tag| match tag.hue {
        Hue::Neutral => illus(app, tag.label),
        hue => chip(app, tag.label, hue),
    }));
    qhead(app, meta, header.title, None, Vec::new())
}

/// One run of the head's metadata: mono ids in mono, the rest as quiet text; the meta row
/// spaces the runs itself.
fn meta_run(app: AppState, run: &Span) -> AnyElement {
    let words = run.text.trim().to_owned();
    let ink = match run.hue {
        Hue::Neutral => color(Token::Ink3),
        hue => hue_ink(hue),
    };
    if run.mono {
        mono(app, words).text_color(ink).into_element()
    } else {
        dim(app, words).text_color(ink).into_element()
    }
}

/// The verdicts, each bound to the SHA it was given on.
fn verdicts(app: AppState, page: &PageView) -> AnyElement {
    let form = app.form;
    let gap = match form {
        Form::Desktop => 5.0,
        Form::Phone => 8.0,
        Form::Terminal => 0.0,
    };
    let rows = view()
        .gap(px(gap))
        .children(
            page.verdicts
                .iter()
                .map(|verdict| verdict_row(app, verdict)),
        )
        .into_element();
    let heading = page.verdict_heading;
    match form {
        Form::Desktop => section(
            app,
            None,
            None,
            Hue::Neutral,
            view()
                .child(flush_h3(app, heading))
                .child(rows)
                .into_element(),
        ),
        Form::Phone => section(
            app,
            Some(heading.to_owned()),
            page.verdict_binding.map(str::to_owned),
            Hue::Neutral,
            rows,
        ),
        Form::Terminal => section(app, Some(heading.to_owned()), None, Hue::Neutral, rows),
    }
}

/// `text::h3` keeps 18 pt above its caption (14 on a phone) for a caption inside a page; where
/// the mockup's caption opens its section (`.h3` at `margin-top: 0`) that room goes again.
fn flush_h3(app: AppState, label: &'static str) -> AnyElement {
    let lift = match app.form {
        Form::Desktop => 18.0,
        Form::Phone => 14.0,
        Form::Terminal => 0.0,
    };
    view()
        .mt(px(-lift))
        .child(h3(app, label, None))
        .into_element()
}

/// One verdict. Desktop: the mockup's columns (mark 16, who 56, SHA 78, time 44, result).
/// Phone: the mark, then who · SHA time with the tag, the result under it. Terminal: one line.
fn verdict_row(app: AppState, verdict: &VerdictView) -> AnyElement {
    let form = app.form;
    let old = verdict.mark == Mark::Stale;
    let tag = verdict
        .tag
        .as_ref()
        .map(|tag| chip(app, tag.label, tag.hue));
    match form {
        Form::Desktop => view()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .when(old, |row| row.opacity(OLD_VERDICT_OPACITY))
            .child(fixed(16.0, mark(app, verdict.mark, Hue::Neutral)))
            .child(fixed(
                56.0,
                text(verdict.who)
                    .truncate()
                    .font_family(FONT_TEXT)
                    .text_size(px(12.5))
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(Token::Ink)),
            ))
            .child(fixed(
                78.0,
                rich_at(app, &verdict.sha, 11.5, Token::Ink, true).truncate(),
            ))
            .child(fixed(
                44.0,
                mono(app, verdict.time).text_color(color(Token::Ink3)),
            ))
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.0))
                    .child(rich_at(app, &verdict.text, 12.5, Token::Ink, false))
                    .children(tag),
            )
            .into_element(),
        Form::Phone => view()
            .flex_row()
            .items_start()
            .gap(px(8.0))
            .when(old, |row| row.opacity(OLD_VERDICT_OPACITY))
            .child(view().w(px(16.0)).flex_shrink_0().pt(px(3.0)).child(mark(
                app,
                verdict.mark,
                Hue::Neutral,
            )))
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap(px(2.0))
                    .child(
                        view()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .gap(px(6.0))
                            .child(rich_at(app, &verdict.line, 13.0, Token::Ink, false))
                            .children(tag),
                    )
                    .child(rich_at(app, &verdict.text, 13.0, Token::Ink, false)),
            )
            .into_element(),
        Form::Terminal => view()
            .flex_row()
            .items_center()
            .h(term::rows(1.0))
            .child(mark(app, verdict.mark, Hue::Neutral))
            .child(term::space(1.0))
            .child(
                view().w(term::cells(7.0)).flex_shrink_0().child(
                    text(verdict.who)
                        .truncate()
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(Token::Ink)),
                ),
            )
            .child(
                view()
                    .w(term::cells(10.0))
                    .flex_shrink_0()
                    .child(rich_at(app, &verdict.sha, 0.0, Token::Ink, true).truncate()),
            )
            .child(
                view()
                    .w(term::cells(6.0))
                    .flex_shrink_0()
                    .child(term::quiet(verdict.time)),
            )
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(rich_at(app, &verdict.text, 0.0, Token::Ink, false).truncate()),
            )
            .when(tag.is_some(), |row| row.child(term::space(1.0)))
            .children(tag)
            .into_element(),
    }
}

/// A fixed column of a desktop row.
fn fixed(width: f32, content: impl IntoElement) -> AnyElement {
    view()
        .w(px(width))
        .flex_shrink_0()
        .child(content)
        .into_element()
}

/// The evidence tabs and, beside the Diff tab on desktop, the split | unified switch. The line
/// under the tabs runs across the whole detail, as the body's top rule does in the mockup.
fn tab_row(app: AppState, page: &PageView) -> AnyElement {
    let strip = evidence_tabs(app, page);
    let rule = || {
        view()
            .flex_shrink_0()
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
    };
    match app.form {
        Form::Desktop => view()
            .flex_row()
            .pt(px(8.0))
            .child(rule().w(px(24.0)))
            .child(view().flex_1().min_w(px(0.0)).child(strip))
            .child(
                rule()
                    .flex_row()
                    .items_center()
                    .pl(px(10.0))
                    .pr(px(24.0))
                    .child(layout_switch(app)),
            )
            .into_element(),
        Form::Phone => view()
            .mx(px(16.0))
            .mt(px(4.0))
            .mb(px(8.0))
            .child(strip)
            .into_element(),
        Form::Terminal => view()
            .child(view().px(term::cells(1.0)).child(strip))
            .child(term::hrule())
            .into_element(),
    }
}

/// The form's evidence tabs: a tab strip on desktop and terminal, the segmented `.pseg` on a
/// phone. A tab the form does not list (Gate log on a phone) leaves none selected.
fn evidence_tabs(app: AppState, page: &PageView) -> AnyElement {
    let options: Vec<(Option<Tab>, &'static str)> = page
        .tabs
        .iter()
        .map(|tab| (Some(tab.tab), tab.label))
        .collect();
    let actions: Vec<(Tab, Action)> = page.tabs.iter().map(|tab| (tab.tab, tab.action)).collect();
    let selected = move || {
        page_view(app, ANY_WIDTH)
            .tabs
            .iter()
            .find(|tab| tab.selected)
            .map(|tab| tab.tab)
    };
    let pick = move |choice: Option<Tab>| {
        if let Some(action) = choice.and_then(|tab| lookup(&actions, tab)) {
            act(app, action)();
        }
    };
    match app.form {
        Form::Phone => seg(app, &options, selected, pick, "seg-review"),
        Form::Desktop | Form::Terminal => tab_strip(app, &options, selected, pick, "seg-review"),
    }
}

/// The desktop's Side by side | Unified switch; the model offers it only with the Diff tab.
fn layout_switch(app: AppState) -> AnyElement {
    let shown = computed(move || !page_view(app, ANY_WIDTH).layouts.is_empty());
    dynamic(move || {
        shown.get().then(|| {
            let choices = untracked(|| page_view(app, ANY_WIDTH).layouts);
            let options: Vec<(Option<Layout>, &'static str)> = choices
                .iter()
                .map(|choice| (Some(choice.layout), choice.label))
                .collect();
            let actions: Vec<(Layout, Action)> = choices
                .iter()
                .map(|choice| (choice.layout, choice.action))
                .collect();
            seg(
                app,
                &options,
                move || {
                    page_view(app, ANY_WIDTH)
                        .layouts
                        .iter()
                        .find(|choice| choice.selected)
                        .map(|choice| choice.layout)
                },
                move |choice: Option<Layout>| {
                    if let Some(action) = choice.and_then(|layout| lookup(&actions, layout)) {
                        act(app, action)();
                    }
                },
                "seg-layout",
            )
        })
    })
}

fn lookup<K: PartialEq>(actions: &[(K, Action)], key: K) -> Option<Action> {
    actions
        .iter()
        .find(|(candidate, _)| *candidate == key)
        .map(|&(_, action)| action)
}

/// The selected evidence, rebuilt only when the model's body changes (a tab, the layout, or the
/// measured width crossing the split threshold), not on every resize.
fn evidence_area(app: AppState, width: Signal<Option<f32>>) -> AnyElement {
    let body = computed(move || width.get().map(|width| page_view(app, width).body));
    let content = dynamic(move || body.get().map(|body| evidence(app, &body)));
    match app.form {
        Form::Desktop => view()
            .px(px(24.0))
            .pt(px(12.0))
            .pb(px(12.0))
            .child(content)
            .into_element(),
        Form::Phone => view()
            .px(px(14.0))
            .pt(px(4.0))
            .pb(px(20.0))
            .child(content)
            .into_element(),
        Form::Terminal => view()
            .px(term::cells(1.0))
            .child(term::blank())
            .child(content)
            .into_element(),
    }
}

fn evidence(app: AppState, body: &Body) -> AnyElement {
    match body {
        Body::Diff(view) => diff::diff(app, view),
        Body::Report(report) => evidence::report(app, report),
        Body::Screenshot(shot) => evidence::screenshot(app, shot),
        Body::Log { lines, note } => evidence::log(app, lines, note),
    }
}

/// The candidate list (desktop and terminal; the phone has none) and its replay footnote.
fn candidates(app: AppState, page: &PageView) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    view()
        .when(!terminal, |list| list.pt(px(6.0)).pb(px(14.0)))
        .children(
            page.candidates
                .iter()
                .map(|group| candidate_group(app, group)),
        )
        .children(page.candidate_note.map(|note| {
            let place = view();
            let place = if terminal {
                place.px(term::cells(1.0))
            } else {
                place.px(px(16.0)).py(px(8.0))
            };
            place.child(note_text(app, note)).into_element()
        }))
        .into_element()
}

/// An illustrative note that wraps (`.illus` in a block): `kit::chip::illus` is one line.
fn note_text(app: AppState, note: &'static str) -> AnyElement {
    let words = text(note).italic();
    match app.form {
        Form::Terminal => words.text_color(color(Token::Ink3)).into_element(),
        Form::Desktop | Form::Phone => words
            .font_family(FONT_TEXT)
            .text_size(px(10.5))
            .line_height(px(15.0))
            .letter_spacing(px(0.1))
            .text_color(color(Token::Ink4))
            .into_element(),
    }
}

fn candidate_group(app: AppState, group: &CandidateGroup) -> AnyElement {
    view()
        .child(group_header(
            app,
            group.label,
            Some(group.count),
            group.hue,
            None,
        ))
        .children(group.rows.iter().map(|row| candidate_row(app, row)))
        .into_element()
}

/// A candidate: the actor dot, then the mono identity, the title and the state on three lines
/// (`.row` at `height: auto`). `kit::rows::list_row` is one fixed row high, so this keeps its
/// look (selected fill, the 1 pt ink bar, hover, focus ring) at the candidate's own height.
fn candidate_row(app: AppState, row: &CandidateRow) -> AnyElement {
    let candidate = row.candidate;
    let selected = row.selected;
    let press = act(app, row.action);
    let base = view()
        .relative()
        .flex_row()
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(format!(
            "{}, {}, {}",
            flat(&row.identity),
            candidate.title,
            candidate.state
        ))
        .accessibility_state(move || AccessibilityState {
            selected,
            ..AccessibilityState::default()
        })
        .test_id(&format!("task-{}", candidate.task))
        .on_press(move || press());
    match app.form {
        Form::Terminal => base
            .items_start()
            .child(
                view()
                    .w(term::cells(1.0))
                    .flex_shrink_0()
                    .accessibility_elements_hidden(true)
                    .child(text(if selected { "▌" } else { " " }).text_color(color(Token::Ink))),
            )
            .child(term::space(1.0))
            .child(dot(app, candidate.hue))
            .child(term::space(1.0))
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(rich_at(app, &row.identity, 0.0, Token::Ink3, true).truncate())
                    .child(
                        text(candidate.title)
                            .truncate()
                            .text_color(color(Token::Ink)),
                    )
                    .child(
                        term::quiet(candidate.state)
                            .flex_shrink(1.0)
                            .min_w(px(0.0))
                            .truncate(),
                    ),
            )
            .into_element(),
        Form::Desktop | Form::Phone => base
            .items_center()
            .gap(px(8.0))
            .pl(px(16.0))
            .pr(px(14.0))
            .py(px(8.0))
            .when(selected, |r| r.bg(color(Token::Sel)))
            .hover(|s| s.bg(color(Token::Hover)))
            .focus_visible(|s| {
                s.outline(px(2.0))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(-2.0))
            })
            .when(selected, |r| {
                r.child(
                    view()
                        .absolute()
                        .left(px(0.0))
                        .top(px(4.0))
                        .bottom(px(4.0))
                        .w(px(1.0))
                        .bg(color(Token::Ink)),
                )
            })
            .child(
                view()
                    .w(px(14.0))
                    .flex_shrink_0()
                    .items_center()
                    .child(dot(app, candidate.hue)),
            )
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap(px(1.0))
                    .child(rich_at(app, &row.identity, 11.5, Token::Ink3, true).truncate())
                    .child(
                        text(candidate.title)
                            .truncate()
                            .font_family(FONT_TEXT)
                            .text_size(px(13.0))
                            .text_color(color(Token::Ink)),
                    )
                    .child(
                        text(candidate.state)
                            .truncate()
                            .font_family(FONT_TEXT)
                            .text_size(px(12.0))
                            .text_color(color(Token::Ink3)),
                    ),
            )
            .into_element(),
    }
}

/// The decide bar (`.decide`, `.pdecide`): the model's buttons and, on desktop and terminal,
/// the verdict-binding explanation and the actor attribution at the trailing edge. It follows
/// the connection (the refusal and the device in the attribution).
fn decide(app: AppState) -> AnyElement {
    let model = computed(move || page_view(app, ANY_WIDTH).decide);
    dynamic(move || decide_bar(app, &model.get()))
}

fn decide_bar(app: AppState, decide: &DecideBar) -> AnyElement {
    let actions = decide
        .buttons
        .iter()
        .map(|button| command(app, button))
        .collect();
    let lines: Vec<AnyElement> = decide
        .explanation
        .iter()
        .chain(decide.attribution.iter())
        .map(|line| {
            rich_at(app, line, 11.5, Token::Ink3, false)
                .text_right()
                .when(app.form == Form::Terminal, |t| t.truncate())
                .into_element()
        })
        .collect();
    // The attribution gives way to the buttons: a terminal row sets it beside them unwrapped,
    // and a box does not shrink unless told to.
    let attribution = (!lines.is_empty()).then(|| {
        view()
            .flex_shrink(1.0)
            .min_w(px(0.0))
            .items_end()
            .children(lines)
            .into_element()
    });
    bar(app, actions, attribution)
}

/// A model button as the kit's button: its emphasis, the decide bar's wide primary on a phone,
/// the speech bubble on Ask about this hunk.
fn command(app: AppState, button: &review::Button) -> AnyElement {
    let kind = match button.style {
        ButtonStyle::Primary => ButtonKind::Primary,
        ButtonStyle::Ghost => ButtonKind::Ghost,
        ButtonStyle::Quiet => ButtonKind::Quiet,
    };
    let options = ButtonOptions {
        icon: (button.action == Action::AskHunk).then_some(Icon::Ask),
        disabled: !button.enabled,
        wide: app.form == Form::Phone && button.style == ButtonStyle::Primary,
        ..ButtonOptions::default()
    };
    button_with(
        app,
        button.label.clone(),
        kind,
        options,
        button_id(button.action),
        act(app, button.action),
    )
}

/// The test id of a control by its action.
fn button_id(action: Action) -> String {
    match action {
        Action::Approve => "btn-approve".to_owned(),
        Action::RequestChanges => "btn-request-changes".to_owned(),
        Action::AskHunk => "btn-ask-hunk".to_owned(),
        Action::CommentBox => "btn-comment".to_owned(),
        Action::ExpandFile(path) => format!("btn-expand-{}", slug(path)),
        Action::Select(id) => format!("task-{id}"),
        Action::Tab(_) | Action::Layout(_) => "seg-review".to_owned(),
    }
}

/// A path as a test id: lower-case letters and digits, everything else a hyphen.
fn slug(path: &str) -> String {
    path.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// The model's [`Rich`] text at `size` in `ink`, for the mockup's small runs the kit's two
/// paragraph sizes do not cover (11.5 pt file heads, 12.5 pt verdicts and comments). `mono`
/// sets the whole run in the mono font; otherwise mono spans take 0.93 of the size. Bold spans
/// are ink (`.who b`), dim spans ink-3, a hue its colour. A terminal keeps its own font and size.
fn rich_at(app: AppState, rich: &Rich, size: f32, ink: Token, mono: bool) -> Text {
    let terminal = app.form == Form::Terminal;
    let root = text("")
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .text_color(color(ink));
    let root = if terminal {
        root
    } else {
        root.font_family(if mono { FONT_MONO } else { FONT_TEXT })
            .text_size(px(size))
            .line_height(px((size * 1.45).round()))
    };
    root.children(rich.iter().map(|run| {
        let mut piece = text(run.text.clone());
        if run.bold {
            piece = piece
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink));
        }
        if run.mono && !mono && !terminal {
            piece = piece
                .font_family(FONT_MONO)
                .text_size(px(size * MONO_SCALE));
        }
        if run.hue != Hue::Neutral {
            piece = piece.text_color(hue_ink(run.hue));
        } else if run.dim {
            piece = piece.text_color(color(Token::Ink3));
        }
        if run.strike {
            piece = piece.line_through();
        }
        piece
    }))
}

/// The keyboard focus ring: 2 pt of the focus colour, 2 pt out (`:focus-visible`).
fn ring(s: StyleRefinement) -> StyleRefinement {
    s.outline(px(2.0))
        .outline_color(color(Token::Focus))
        .outline_offset(px(2.0))
}
