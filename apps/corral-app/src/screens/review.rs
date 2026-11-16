//! Review: the candidate list, the selected candidate with its SHA-bound verdicts, the evidence
//! tabs and the pinned decide bar. Every word, count and branch comes from `model::review`, in
//! the Review screen's own project scope; this module lays the model's page out for each form and
//! sends each control's action back through the feed-backed reducer (`apply_live_filtered`).
//!
//! Desktop is the mockup's two columns (the 300 pt candidate list, the detail beside it); the
//! detail measures its own width and asks the model for that width, so the diff falls back to
//! unified rows when side by side does not fit. Phone: the candidates are a list whose rows push
//! one candidate's page; that page scrolls with the decide bar pinned in place of the tab bar.
//! Terminal is the desktop's columns in Cells, the diff always unified.

mod compose;
mod decide;
mod diff;
mod evidence;
mod head;
mod list;

use std::collections::BTreeMap;

use corral_protocol::detail::DetailRef;
use nsh_core::element::StyleRefinement;
use nsh_core::prelude::*;

use crate::model::act;
use crate::model::daemon::{Feed, Link, Task};
use crate::model::projects::{FilterScreen, ProjectFilter};
use crate::model::review::{self, Action, Body, ButtonStyle, DetailView, Layout, PageView, Tab};
use crate::model::work::{self, Record};
use crate::model::{Form, Hue, Id, Rich, Selection};
use crate::screens::now::parts::{keep_loaded, wanted};
use crate::screens::now::verb_buttons;
use crate::screens::{PhoneBar, PhonePage};
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with, seg, tab_strip};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::notice::empty;
use crate::ui::kit::page::{Width, bar_pane, columns, pane};
use crate::ui::kit::text::dim;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use decide::decide;
use head::{header, notice_row, verdicts};
use list::{candidates, phone_list};

/// The candidate list's column (`grid-template-columns: 300px ...`).
const LIST_WIDTH: f32 = 300.0;
/// `.is-stale .stalefx`: the last known state reads quieter while offline or down.
const STALE_OPACITY: f32 = 0.78;
/// `.mono` is 0.93 of the text around it.
const MONO_SCALE: f32 = 0.93;
/// Width handed to the model where width plays no part (tabs, decide bar, stale state).
const ANY_WIDTH: f32 = f32::INFINITY;

/// What every piece of one Review page shares: the app, the candidate a phone page was pushed
/// for (else the selected one), and the unsent drafts of its composers, which outlive the
/// evidence being rebuilt by a refresh.
#[derive(Clone)]
struct Ctx {
    app: AppState,
    focus: Option<Id>,
    drafts: Signal<BTreeMap<String, String>>,
}

impl Ctx {
    fn new(app: AppState, focus: Option<Id>) -> Ctx {
        Ctx {
            app,
            focus,
            drafts: signal(BTreeMap::new()),
        }
    }
}

/// The Review destination on every form. Desktop and terminal fill the content container and
/// scroll inside themselves; a phone lists the candidates, each pushing its page.
pub fn screen(app: AppState) -> AnyElement {
    let cx = Ctx::new(app, None);
    match app.form {
        Form::Desktop | Form::Terminal => columns_screen(cx),
        Form::Phone => phone_list(cx),
    }
}

/// A candidate pushed on the phone, by its task id: the scrolling body and the decide bar in
/// place of the tab bar.
pub fn live_review_phone(app: AppState, task: Id) -> PhonePage {
    let cx = Ctx::new(app, Some(task));
    let title = untracked(|| page_view(&cx, 0.0).title);
    PhonePage {
        title,
        body: phone_body(cx.clone()),
        trailing: None,
        bar: Some(PhoneBar::always(decide(cx))),
        ask: true,
        scroll: true,
    }
}

/// The Review screen's own project scope, tracked.
fn scope(app: AppState) -> ProjectFilter {
    app.project_filter(FilterScreen::Review)
}

/// The model's page for this form at `width` (tracked: state, connection, selection, scope and
/// feed).
fn page_view(cx: &Ctx, width: f32) -> PageView {
    let app = cx.app;
    let env = app.env();
    let selection = app.selection();
    let filter = scope(app);
    app.feed.with(|feed| {
        app.review.with(|state| {
            review::live_page_filtered(
                state,
                feed,
                &selection,
                cx.focus.as_ref(),
                env,
                width,
                &filter,
            )
        })
    })
}

/// Runs a model action through the feed-backed reducer and the app's effects. Restoring an
/// unsent note puts its text back into the composer's draft first.
fn act(cx: &Ctx, action: Action) -> impl Fn() + 'static {
    let app = cx.app;
    let drafts = cx.drafts;
    move || {
        if let Action::Restore { draft, text, .. } = &action {
            drafts.update(|all| {
                all.insert(draft.clone(), text.clone());
            });
        }
        let action = action.clone();
        let filter = untracked(|| scope(app));
        app.reduce(app.review, move |state, feed, selection, env| {
            state.apply_live_filtered(action, feed, selection, env, &filter)
        });
    }
}

/// Keeps the served candidate current: its details are fetched when it is first shown, and
/// revalidated when its board row changes, the link comes back, or a command sent from here
/// settles. Every fetch carries the cached etag, so an unchanged detail costs one small reply.
/// While shown, the candidate's task detail (whose owner revision its commands are bound to)
/// stays loaded, and both details stay pinned in the client's cache, through the source the
/// page mounted on.
fn keep_current(cx: &Ctx) {
    let focus = cx.focus.clone();
    let app = cx.app;
    let key = {
        let focus = focus.clone();
        computed(move || {
            let selection = app.selection();
            let filter = scope(app);
            app.feed.with(|feed| {
                let task = shown_task(feed, &selection, focus.as_ref(), &filter)?;
                let settled = app.review.with(|state| {
                    state
                        .sent
                        .iter()
                        .filter(|sent| {
                            act::status(feed, &sent.key).is_some_and(|status| status.settled())
                        })
                        .count()
                });
                Some((task.id.clone(), task.rev, feed.link == Link::Up, settled))
            })
        })
    };
    let refresh = act(cx, Action::Refresh);
    effect(move || {
        if key.get().is_some_and(|(_, _, up, _)| up) {
            untracked(&refresh);
        }
    });
    let task = computed(move || {
        let selection = app.selection();
        let filter = scope(app);
        app.feed.with(|feed| {
            shown_task(feed, &selection, focus.as_ref(), &filter)
                .and_then(|task| wanted(feed, task))
        })
    });
    keep_loaded(app, task);
    pin_review(
        untracked(|| app.scope()),
        computed(move || task.with(|task| task.as_ref().map(|(id, ..)| id.clone()))),
    );
}

/// The candidate this page shows: the one it was pushed for, or the selected one in scope.
fn shown_task<'a>(
    feed: &'a Feed,
    selection: &Selection,
    focus: Option<&Id>,
    filter: &ProjectFilter,
) -> Option<&'a Task> {
    match focus {
        Some(id) => feed.board.task(id),
        None => review::selected_filtered(selection, feed, filter),
    }
}

/// Pins the shown candidate's review detail in the client's cache, letting go when another is
/// shown or the page goes.
fn pin_review(app: AppState, shown: Memo<Option<Id>>) {
    let pinned = stored(None::<Id>);
    let target = |task: &Id| DetailRef::Review {
        task: task.to_string(),
    };
    effect(move || {
        let now = shown.get();
        if pinned.with(|pinned| *pinned == now) {
            return;
        }
        let was = pinned.replace(now.clone());
        untracked(|| {
            if let Some(gone) = was {
                app.watch_detail(target(&gone), false);
            }
            if let Some(new) = &now {
                app.watch_detail(target(new), true);
            }
        });
    });
    // The owner may already be letting go of its values when this runs.
    cleanup(move || {
        if let Some(gone) = pinned.try_with(Option::take).flatten() {
            app.watch_detail(target(&gone), false);
        }
    });
}

/// The shown candidate's verbs (a candidate-bound verdict among them) under its verdicts, from
/// the model's table.
fn verb_row(cx: &Ctx) -> AnyElement {
    let focus = cx.focus.clone();
    let app = cx.app;
    let verbs = computed(move || {
        let selection = app.selection();
        let filter = scope(app);
        app.feed.with(|feed| {
            shown_task(feed, &selection, focus.as_ref(), &filter)
                .map(|task| work::record_verbs(feed, &Record::Review(task.id.clone())))
                .unwrap_or_default()
        })
    });
    dynamic(move || {
        let row = verb_buttons(app, verbs.get())?;
        let place = match app.form {
            Form::Desktop => view().px(px(24.0)).pt(px(4.0)).pb(px(12.0)),
            Form::Phone => view().px(px(16.0)).pb(px(12.0)),
            Form::Terminal => view().px(term::cells(1.0)),
        };
        Some(place.child(row).into_element())
    })
}

/// Desktop and terminal: the candidate list beside the detail, which carries the decide bar.
fn columns_screen(cx: Ctx) -> AnyElement {
    let app = cx.app;
    keep_current(&cx);
    let desktop = app.form == Form::Desktop;
    // A terminal is always unified, so only the desktop waits for its measure.
    let width = signal(if desktop { None } else { Some(ANY_WIDTH) });
    let stale = {
        let cx = cx.clone();
        computed(move || page_view(&cx, ANY_WIDTH).stale)
    };
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
            detail(cx.clone(), width),
            decide(cx.clone()),
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
                    pane(app, "review-list", candidates(cx.clone())),
                ),
                (Width::Fill, detail_column.into_element()),
            ],
            1,
        ))
        .into_element()
}

/// The phone page body: head, verdicts, the segmented tabs and the selected evidence.
fn phone_body(cx: Ctx) -> AnyElement {
    keep_current(&cx);
    let stale = {
        let cx = cx.clone();
        computed(move || page_view(&cx, 0.0).stale)
    };
    view()
        .opacity(move || if stale.get() { STALE_OPACITY } else { 1.0 })
        .child(detail(cx, signal(Some(0.0))))
        .into_element()
}

/// The detail column's scrolling content, rebuilt when the candidate's head, verdicts or
/// notices change; the tabs and the evidence follow their own state inside it.
fn detail(cx: Ctx, width: Signal<Option<f32>>) -> AnyElement {
    let shape = {
        let cx = cx.clone();
        computed(move || {
            let page = page_view(&cx, ANY_WIDTH);
            (
                page.detail.map(|detail| DetailView {
                    body: Body::Notice(String::new()),
                    tabs: Vec::new(),
                    layouts: Vec::new(),
                    ..detail
                }),
                page.empty,
                page.decide.secondary,
            )
        })
    };
    dynamic(move || {
        let (detail, empty_line, secondary) = shape.get();
        let app = cx.app;
        let Some(detail) = detail else {
            return empty(
                app,
                "Nothing to review",
                empty_line.unwrap_or_default(),
                None,
            );
        };
        let phone_actions = (app.form == Form::Phone && !secondary.is_empty()).then(|| {
            view()
                .flex_row()
                .flex_wrap()
                .gap(px(8.0))
                .px(px(16.0))
                .pb(px(8.0))
                .children(secondary.iter().map(|button| command(&cx, button)))
                .into_element()
        });
        view()
            .child(header(app, &detail.header))
            .children(detail.notice.as_ref().map(|line| notice_row(&cx, line)))
            .children(phone_actions)
            .child(verdicts(app, &detail))
            .child(verb_row(&cx))
            .child(tab_row(&cx, width))
            .child(evidence_area(&cx, width))
            .into_element()
    })
}

/// The width under which the evidence tabs and the Side by side | Unified switch cannot share
/// a row: the four tabs take about 290 pt, the switch about 190 pt, the row's padding 58 pt.
const TABS_BESIDE_SWITCH: f32 = 560.0;

/// The evidence tabs and, beside the Diff tab on desktop, the split | unified switch. The line
/// under the tabs runs across the whole detail, as the body's top rule does in the mockup. A
/// desktop detail narrower than `TABS_BESIDE_SWITCH` puts the switch on its own row under the
/// tabs, at the trailing edge, so neither covers the other.
fn tab_row(cx: &Ctx, width: Signal<Option<f32>>) -> AnyElement {
    let app = cx.app;
    let rule = || {
        view()
            .flex_shrink_0()
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
    };
    match app.form {
        Form::Desktop => {
            let room = signal(None::<f32>);
            let narrow =
                computed(move || room.get().is_some_and(|width| width < TABS_BESIDE_SWITCH));
            let cx = cx.clone();
            view()
                .w_full()
                .min_w(px(0.0))
                .on_layout(move |frame: &Bounds<Points>| room.set(Some(frame.size.width.0)))
                .child(dynamic(move || {
                    let tabs = view()
                        .flex_row()
                        .pt(px(8.0))
                        .child(rule().w(px(24.0)))
                        .child(view().flex_1().min_w(px(0.0)).child(evidence_tabs(&cx)));
                    if narrow.get() {
                        view()
                            .child(tabs.child(rule().w(px(24.0))))
                            .child(
                                view()
                                    .flex_row()
                                    .justify_end()
                                    .px(px(24.0))
                                    .pt(px(8.0))
                                    .child(layout_switch(&cx, width)),
                            )
                            .into_element()
                    } else {
                        tabs.child(
                            rule()
                                .flex_row()
                                .items_center()
                                .pl(px(10.0))
                                .pr(px(24.0))
                                .child(layout_switch(&cx, width)),
                        )
                        .into_element()
                    }
                }))
                .into_element()
        }
        Form::Phone => view()
            .mx(px(16.0))
            .mt(px(4.0))
            .mb(px(8.0))
            .child(evidence_tabs(cx))
            .into_element(),
        Form::Terminal => view()
            .child(view().px(term::cells(1.0)).child(evidence_tabs(cx)))
            .child(term::hrule())
            .into_element(),
    }
}

/// The form's evidence tabs: a tab strip on desktop and terminal, the segmented `.pseg` on a
/// phone. A tab the form does not list (Gate log on the fixture phone) leaves none selected.
fn evidence_tabs(cx: &Ctx) -> AnyElement {
    let app = cx.app;
    let tabs = untracked(|| {
        page_view(cx, ANY_WIDTH)
            .detail
            .map(|detail| detail.tabs)
            .unwrap_or_default()
    });
    let options: Vec<(Option<Tab>, &'static str)> =
        tabs.iter().map(|tab| (Some(tab.tab), tab.label)).collect();
    let selected = move || app.review.with(|state| Some(state.tab));
    let pick = {
        let cx = cx.clone();
        move |choice: Option<Tab>| {
            if let Some(tab) = choice {
                act(&cx, Action::Tab(tab))();
            }
        }
    };
    match app.form {
        Form::Phone => seg(app, &options, selected, pick, "seg-review"),
        Form::Desktop | Form::Terminal => tab_strip(app, &options, selected, pick, "seg-review"),
    }
}

/// The desktop's Side by side | Unified switch, with the model's choices, offered with the Diff
/// tab only where the detail is wide enough for side by side (the model's rule): narrower, the
/// diff is unified and a switch whose other choice could not show would mislead.
fn layout_switch(cx: &Ctx, width: Signal<Option<f32>>) -> AnyElement {
    let app = cx.app;
    let options = {
        let cx = cx.clone();
        computed(move || {
            let fits = width.get().is_some_and(|width| {
                let env = app.env();
                app.review.with(|state| {
                    let mut split = state.clone();
                    split.layout = Layout::Split;
                    review::effective_layout(&split, env, width) == Layout::Split
                })
            });
            if !fits {
                return Vec::new();
            }
            page_view(&cx, ANY_WIDTH)
                .detail
                .map(|detail| {
                    detail
                        .layouts
                        .iter()
                        .map(|choice| (Some(choice.layout), choice.label))
                        .collect::<Vec<(Option<Layout>, &'static str)>>()
                })
                .unwrap_or_default()
        })
    };
    let cx = cx.clone();
    dynamic(move || {
        let options = options.get();
        (!options.is_empty()).then(|| {
            let cx = cx.clone();
            seg(
                app,
                &options,
                move || app.review.with(|state| Some(state.layout)),
                move |choice: Option<Layout>| {
                    if let Some(layout) = choice {
                        act(&cx, Action::Layout(layout))();
                    }
                },
                "seg-layout",
            )
        })
    })
}

/// The selected evidence, rebuilt only when the model's body changes (a tab, the layout, the
/// served data, or the measured width crossing the split threshold), not on every resize.
fn evidence_area(cx: &Ctx, width: Signal<Option<f32>>) -> AnyElement {
    let app = cx.app;
    let body = {
        let cx = cx.clone();
        computed(move || {
            width.get().map(|width| {
                page_view(&cx, width)
                    .detail
                    .map_or(Body::Notice(String::new()), |detail| detail.body)
            })
        })
    };
    let content = {
        let cx = cx.clone();
        dynamic(move || body.get().map(|body| evidence(&cx, &body)))
    };
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

fn evidence(cx: &Ctx, body: &Body) -> AnyElement {
    match body {
        Body::Diff(view) => diff::diff(cx, view),
        Body::Report(report) => evidence::report(cx.app, report),
        Body::Artifacts(list) => evidence::artifacts(cx, list),
        Body::Log { lines, note } => evidence::log(cx.app, lines, note),
        Body::Notice(line) => dim(cx.app, line.clone()).into_element(),
    }
}

/// A model button as the kit's button: its emphasis, the decide bar's wide primary on a phone,
/// the speech bubble on Ask, the danger outline on Discard.
fn command(cx: &Ctx, button: &review::Button) -> AnyElement {
    let app = cx.app;
    let kind = match button.style {
        ButtonStyle::Primary => ButtonKind::Primary,
        ButtonStyle::Ghost => ButtonKind::Ghost,
        ButtonStyle::Quiet => ButtonKind::Quiet,
    };
    let options = ButtonOptions {
        icon: matches!(button.action, Action::AskAbout { .. }).then_some(Icon::Ask),
        disabled: !button.enabled,
        wide: app.form == Form::Phone && button.style == ButtonStyle::Primary,
        danger: button.action == Action::Discard,
        ..ButtonOptions::default()
    };
    button_with(
        app,
        button.label.clone(),
        kind,
        options,
        button_id(&button.action),
        act(cx, button.action.clone()),
    )
}

/// The test id of a control by its action.
fn button_id(action: &Action) -> String {
    match action {
        Action::Approve => "btn-approve".to_owned(),
        Action::RequestChanges => "btn-request-changes".to_owned(),
        Action::AskAbout { path, .. } => format!("btn-ask-{}", slug(path)),
        Action::Expand(path) => format!("btn-expand-{}", slug(path)),
        Action::Collapse(path) => format!("btn-collapse-{}", slug(path)),
        Action::Pick(id) => format!("task-{id}"),
        Action::Tab(_) | Action::Layout(_) => "seg-review".to_owned(),
        Action::Annotate { at, .. } => format!("btn-annotate-{}-{}", slug(&at.path), at.line),
        Action::CancelCompose => "btn-cancel".to_owned(),
        Action::Submit { .. } => "btn-send".to_owned(),
        Action::Resolve { thread, .. } => format!("btn-resolve-{}", slug(thread)),
        Action::Reopen { thread, .. } => format!("btn-reopen-{}", slug(thread)),
        Action::Retry => "btn-retry".to_owned(),
        Action::Discard => "btn-discard".to_owned(),
        Action::Recover(_) => "btn-check-again".to_owned(),
        Action::Dismiss(_) => "btn-dismiss".to_owned(),
        Action::Restore { .. } => "btn-edit-again".to_owned(),
        Action::OpenArtifact(key) | Action::CloseArtifact(key) => {
            format!("btn-artifact-{}", slug(key))
        }
        Action::Refresh => "btn-refresh".to_owned(),
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
