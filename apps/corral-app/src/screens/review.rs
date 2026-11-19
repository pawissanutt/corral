//! Review: the tasks in Review beside the selected candidate, its Brief (ready to land? and
//! what changed) or its evidence (Diff, Gate log, Summary, Artifacts), its side facts and the
//! pinned decide bar. Every word, count and branch comes from `model::review::loop_page_filtered`,
//! in the Review screen's own project scope; this module lays that page out for each form and
//! sends each control's action back through the feed-backed reducer (`apply_live_filtered`).
//!
//! Desktop is the mock's three columns: the outline, the candidate, its side facts beside the
//! Brief. Under `SIDE_BESIDE` points of content the facts move under the Brief; under
//! `LIST_BESIDE` the outline is the page and a picked candidate takes its place, with a way
//! back. The candidate's column measures itself, so the model draws split diff rows only where
//! they fit. Phone: the outline, each row pushing its candidate's page, which scrolls with the
//! decide bar pinned in place of the tab bar. Terminal: outline | candidate in Cells, the facts
//! under the Brief, the diffs unified.

pub(crate) mod brief;
mod decide;
pub(crate) mod diff;
mod evidence;
mod head;
mod list;
mod onto;
mod rounds;
mod status;

use corral_protocol::detail::DetailRef;
use nsh_core::element::StyleRefinement;
use nsh_core::prelude::*;

use crate::model::act;
use crate::model::daemon::{Feed, Link, Task};
use crate::model::projects::{FilterScreen, ProjectFilter};
use crate::model::review::{self, Action, Body, ButtonStyle, Layout, LoopContent, LoopPage};
use crate::model::work::{Press, Record};
use crate::model::{Form, Hue, Id, Live, Rich, Selection};
use crate::screens::now::parts::press;
use crate::screens::{PhoneBar, PhonePage};
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::glance::{page_head, stats};
use crate::ui::kit::hint::hint_on;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::page::{Width, bar_pane, columns, pane};
use crate::ui::kit::project_filter::project_filter;
use crate::ui::kit::text::dim;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// The outline's column: `272px` beside the side facts, `260px` once they stand under the Brief.
const LIST_WIDE: f32 = 272.0;
const LIST_NARROW: f32 = 260.0;
/// The side facts' column (`.split.s-review`).
const SIDE_WIDTH: f32 = 300.0;
/// The content widths from which the side facts stand beside the candidate, and the outline
/// beside it (the mock's 1180 and 900 rules).
const SIDE_BESIDE: f32 = 1180.0;
const LIST_BESIDE: f32 = 900.0;
/// `.is-stale .stalefx`: the last known state reads quieter while offline or down.
const STALE_OPACITY: f32 = 0.78;
/// `.mono` is 0.93 of the text around it.
const MONO_SCALE: f32 = 0.93;

/// What every piece of one Review page shares: the app, the candidate a phone page was pushed
/// for (else the selected one), whether the candidate's column holds split diff rows, and the
/// model's page.
#[derive(Clone)]
struct Ctx {
    app: AppState,
    focus: Option<Id>,
    /// Desktop: whether the candidate's column is wide enough for side-by-side rows, unknown
    /// until it is measured; elsewhere never.
    split: Signal<Option<bool>>,
    page: Memo<LoopPage>,
}

impl Ctx {
    fn new(app: AppState, focus: Option<Id>) -> Ctx {
        let split = signal((app.form != Form::Desktop).then_some(false));
        let page = {
            let focus = focus.clone();
            // The model reads the width only against its split threshold, so the page is asked
            // at either side of it rather than again at every measured point.
            computed(move || {
                let width = if split.get() == Some(true) {
                    f32::INFINITY
                } else {
                    0.0
                };
                loop_page(app, focus.as_ref(), width)
            })
        };
        Ctx {
            app,
            focus,
            split,
            page,
        }
    }
}

/// How the desktop arranges the outline, the candidate and its side facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arrangement {
    /// Outline | candidate | side facts (with the Brief).
    Three,
    /// Outline | candidate, the facts under the Brief.
    Two,
    /// The outline or, once one is picked, the candidate.
    One,
}

impl Arrangement {
    fn at(width: f32) -> Arrangement {
        if width >= SIDE_BESIDE {
            Arrangement::Three
        } else if width >= LIST_BESIDE {
            Arrangement::Two
        } else {
            Arrangement::One
        }
    }
}

/// The Review destination on every form. Desktop and terminal fill the content container and
/// scroll inside themselves; a phone lists the candidates, each pushing its page.
pub fn screen(app: AppState) -> AnyElement {
    let cx = Ctx::new(app, None);
    match app.form {
        Form::Desktop => desktop(cx),
        Form::Terminal => terminal(cx),
        Form::Phone => phone_list(cx),
    }
}

/// A candidate pushed on the phone, by its task id: the scrolling body and the decide bar, which
/// carries Ask, in place of the tab bar.
pub fn live_review_phone(app: AppState, task: Id) -> PhonePage {
    let title = task.to_string();
    let cx = Ctx::new(app, Some(task));
    PhonePage {
        title,
        body: phone_body(cx.clone()),
        trailing: None,
        bar: Some(PhoneBar::always(decide::decide(cx))),
        ask: false,
        scroll: true,
    }
}

/// The Review screen's own project scope, tracked.
fn scope(app: AppState) -> ProjectFilter {
    app.project_filter(FilterScreen::Review)
}

/// The model's page at `width` (tracked: state, connection, selection, scope and feed).
fn loop_page(app: AppState, focus: Option<&Id>, width: f32) -> LoopPage {
    let env = app.env();
    let selection = app.selection();
    let filter = scope(app);
    app.feed.with(|feed| {
        app.review.with(|state| {
            review::loop_page_filtered(state, feed, &selection, focus, env, width, &filter)
        })
    })
}

/// Whether a candidate column `width` points wide holds side-by-side rows, by the model's rule.
fn fits_split(app: AppState, width: f32) -> bool {
    let env = untracked(|| app.env());
    let wanted = review::State {
        layout: Layout::Split,
        ..review::State::default()
    };
    review::effective_layout(&wanted, env, width) == Layout::Split
}

/// Runs a model action through the feed-backed reducer and the app's effects. A shared verb
/// (Request changes) runs through the work state instead, whose composer asks for its words.
fn act(cx: &Ctx, action: Action) -> impl Fn() + 'static {
    let app = cx.app;
    let focus = cx.focus.clone();
    move || {
        let filter = untracked(|| scope(app));
        if let Action::Verb(verb) = &action {
            let shown = app.feed.with_untracked(|feed| {
                let selection = untracked(|| app.selection());
                shown_task(feed, &selection, focus.as_ref(), &filter).map(|task| task.id.clone())
            });
            if let Some(task) = shown {
                press(
                    app,
                    Press::Verb {
                        record: Record::Task(task),
                        verb: *verb,
                    },
                );
            }
            return;
        }
        let action = action.clone();
        let focus = focus.clone();
        app.reduce(app.review, move |state, feed, selection, env| {
            state.apply_live_filtered(action, feed, selection, focus.as_ref(), env, &filter)
        });
    }
}

/// Keeps the served candidate current: its review and task details are fetched when it is first
/// shown, and revalidated when its board row changes, the link comes back, or an operation sent
/// from here settles. Every fetch carries the cached etag, so an unchanged detail costs one
/// small reply. While shown, both details stay pinned in the client's cache, through the source
/// the page mounted on.
fn keep_current(cx: &Ctx) {
    let focus = cx.focus.clone();
    let app = cx.app;
    let key = computed(move || {
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
            Some((
                task.id.clone(),
                task.row.clone(),
                feed.link == Link::Up,
                settled,
            ))
        })
    });
    let refresh = act(cx, Action::Refresh);
    effect(move || {
        if key.with(|key| key.as_ref().is_some_and(|(_, _, up, _)| *up)) {
            untracked(&refresh);
        }
    });
    pin_details(
        untracked(|| app.scope()),
        computed(move || key.with(|key| key.as_ref().map(|(id, ..)| id.clone()))),
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

/// Pins the shown candidate's review and task details in the client's cache, letting go when
/// another is shown or the page goes.
fn pin_details(app: AppState, shown: Memo<Option<Id>>) {
    let pinned = stored(None::<Id>);
    let targets = |task: &Id| {
        [
            DetailRef::Review {
                task: task.to_string(),
            },
            DetailRef::Task {
                id: task.to_string(),
            },
        ]
    };
    effect(move || {
        let now = shown.get();
        if pinned.with(|pinned| *pinned == now) {
            return;
        }
        let was = pinned.replace(now.clone());
        untracked(|| {
            for gone in was.iter().flat_map(targets) {
                app.watch_detail(gone, false);
            }
            for new in now.iter().flat_map(targets) {
                app.watch_detail(new, true);
            }
        });
    });
    // The owner may already be letting go of its values when this runs.
    cleanup(move || {
        if let Some(gone) = pinned.try_with(Option::take).flatten() {
            for target in targets(&gone) {
                app.watch_detail(target, false);
            }
        }
    });
}

/// The page head (`.phead`): Review and its counts.
fn page_title(cx: &Ctx) -> AnyElement {
    let app = cx.app;
    let page = cx.page;
    let head = computed(move || page.with(|page| (page.title, page.stats.clone())));
    dynamic(move || {
        let (title, stats) = head.get();
        page_head(app, title, &stats, Vec::new())
    })
}

/// Tracked: whether there is anything to lay out, a candidate listed or shown.
fn anything(page: Memo<LoopPage>) -> Memo<bool> {
    computed(move || page.with(|page| !page.outline.is_empty() || page.detail.is_some()))
}

/// Desktop: the head and the scope over the arrangement the content's width allows.
fn desktop(cx: Ctx) -> AnyElement {
    let app = cx.app;
    keep_current(&cx);
    let page = cx.page;
    let room = signal(None::<f32>);
    let arrangement = computed(move || room.get().map(Arrangement::at));
    let anything = anything(page);
    let stale = computed(move || page.with(|page| page.stale));
    let opened = signal(false);
    // Below `LIST_BESIDE` the outline comes first after a plain `go`; a candidate opened by
    // name (from Now, the palette) shows at once, and again on each further open. A row
    // pressed here opens it too, without moving this key.
    let arrival = computed(move || {
        app.nav.with(|nav| {
            let named = matches!(nav.opened, Some(Live::Review(_))).then_some(nav.opens);
            (nav.goes, named)
        })
    });
    effect(move || {
        let named = arrival.get().1.is_some();
        untracked(|| opened.set(named));
    });
    let body = {
        let cx = cx.clone();
        dynamic(move || {
            let arrangement = arrangement.get()?;
            if !anything.get() {
                return Some(empty_page(&cx));
            }
            Some(match arrangement {
                Arrangement::Three => beside(&cx, LIST_WIDE, true),
                Arrangement::Two => beside(&cx, LIST_NARROW, false),
                Arrangement::One if opened.get() => candidate(&cx, true, Some(back(&cx, opened))),
                Arrangement::One => outline_pane(&cx, Some(opened)),
            })
        })
    };
    view()
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .opacity(move || if stale.get() { STALE_OPACITY } else { 1.0 })
        .child(page_title(&cx))
        .child(project_filter(app, FilterScreen::Review))
        .child(
            view()
                .flex_1()
                .flex_col()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .on_layout(move |frame: &Bounds<Points>| room.set(Some(frame.size.width.0)))
                .child(body),
        )
        .into_element()
}

/// Terminal: the head and the scope over outline | candidate, which stack when the grid is
/// narrow (`page::columns`).
fn terminal(cx: Ctx) -> AnyElement {
    let app = cx.app;
    keep_current(&cx);
    let anything = anything(cx.page);
    let body = {
        let cx = cx.clone();
        dynamic(move || {
            if !anything.get() {
                return empty_page(&cx);
            }
            columns(
                app,
                vec![
                    (Width::Fixed(LIST_WIDE), outline_pane(&cx, None)),
                    (Width::Fill, candidate(&cx, true, None)),
                ],
                1,
            )
        })
    };
    view()
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(page_title(&cx))
        .child(project_filter(app, FilterScreen::Review))
        .child(
            view()
                .flex_1()
                .flex_col()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .child(body),
        )
        .into_element()
}

/// The phone's destination, pushed over More: its counts and the scope over the outline, each
/// row pushing its candidate's page. The shell's pushed head already carries the page's title
/// (`page-title`), so this page draws no head of its own.
fn phone_list(cx: Ctx) -> AnyElement {
    let app = cx.app;
    view()
        .flex_1()
        .min_h(px(0.0))
        .child(phone_counts(&cx))
        .child(pane(
            app,
            "review-list",
            view()
                .child(project_filter(app, FilterScreen::Review))
                .child(list::outline(&cx, None))
                .into_element(),
        ))
        .into_element()
}

/// The page's counts on a line of their own under the shell's head (`.stats`); nothing while
/// nothing is counted.
fn phone_counts(cx: &Ctx) -> AnyElement {
    let app = cx.app;
    let page = cx.page;
    let counts = computed(move || page.with(|page| page.stats.clone()));
    dynamic(move || {
        let counts = counts.get();
        (!counts.is_empty()).then(|| {
            view()
                .flex_row()
                .flex_none()
                .px(px(18.0))
                .pt(px(4.0))
                .pb(px(10.0))
                .child(stats(app, &counts))
                .into_element()
        })
    })
}

/// The pushed candidate's scrolling body: its head and tabs, Run gate and Start review (and Land
/// anyway when offered), then the Brief with the side facts at its end, or the selected
/// evidence.
fn phone_body(cx: Ctx) -> AnyElement {
    keep_current(&cx);
    let page = cx.page;
    let stale = computed(move || page.with(|page| page.stale));
    view()
        .opacity(move || if stale.get() { STALE_OPACITY } else { 1.0 })
        .child(head::head(&cx, None))
        .child(decide::phone_secondary(&cx))
        .child(content(&cx, true))
        .into_element()
}

/// Why nothing is listed, in the place of the columns (`.split.one`).
fn empty_page(cx: &Ctx) -> AnyElement {
    let app = cx.app;
    let page = cx.page;
    let line = computed(move || page.with(|page| page.empty.clone()));
    dynamic(move || line.get().map(|line| list::empty_line(app, line)))
}

/// The outline in its scrolling pane.
fn outline_pane(cx: &Ctx, opened: Option<Signal<bool>>) -> AnyElement {
    pane(cx.app, "review-list", list::outline(cx, opened))
}

/// Desktop: the outline `list` points wide beside the candidate, and the side facts beside the
/// Brief when `side_beside`.
fn beside(cx: &Ctx, list: f32, side_beside: bool) -> AnyElement {
    view()
        .flex_1()
        .flex_row()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(
            view()
                .w(px(list))
                .flex_shrink_0()
                .flex_col()
                .min_h(px(0.0))
                .child(outline_pane(cx, None)),
        )
        .child(
            view()
                .flex_1()
                .flex_col()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .border_l(px(1.0))
                .border_l_color(color(Token::Rule))
                .child(candidate(cx, !side_beside, None)),
        )
        .when(side_beside, |row| row.child(side_column(cx)))
        .into_element()
}

/// The side facts' column, while the Brief shows (`.aside`).
fn side_column(cx: &Ctx) -> AnyElement {
    let page = cx.page;
    let side = computed(move || {
        page.with(
            |page| match page.detail.as_ref().map(|detail| &detail.content) {
                Some(LoopContent::Brief(brief)) => Some(brief.side.clone()),
                _ => None,
            },
        )
    });
    let cx = cx.clone();
    dynamic(move || {
        side.get().map(|facts| {
            view()
                .w(px(SIDE_WIDTH))
                .flex_shrink_0()
                .flex_col()
                .min_h(px(0.0))
                .border_l(px(1.0))
                .border_l_color(color(Token::Rule))
                .child(pane(cx.app, "review-side", brief::side(&cx, &facts)))
                .into_element()
        })
    })
}

/// The way back to the outline where the candidate stands in its place: the page's title.
fn back(cx: &Ctx, opened: Signal<bool>) -> AnyElement {
    let app = cx.app;
    let title = cx.page.with_untracked(|page| page.title);
    button_with(
        app,
        title,
        ButtonKind::Quiet,
        ButtonOptions {
            icon: Some(Icon::Back),
            ..ButtonOptions::default()
        },
        "page-back",
        move || {
            opened.set(false);
            app.nav.update(|nav| nav.dismiss_opened());
        },
    )
}

/// The candidate's column: its head and tabs, the scrolling content (the side facts under the
/// Brief when `side_below`), the decide bar pinned under it. A desktop column measures itself
/// for the model's split rows.
fn candidate(cx: &Ctx, side_below: bool, back: Option<AnyElement>) -> AnyElement {
    let app = cx.app;
    let split = cx.split;
    view()
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .when(app.form == Form::Desktop, |column| {
            column.on_layout(move |frame: &Bounds<Points>| {
                let fits = fits_split(app, frame.size.width.0);
                if split.with_untracked(|now| *now != Some(fits)) {
                    split.set(Some(fits));
                }
            })
        })
        .child(head::head(cx, back))
        .child(bar_pane(
            app,
            "review-detail",
            content(cx, side_below),
            decide::decide(cx.clone()),
        ))
        .into_element()
}

/// What the candidate's column scrolls: the Brief or the selected evidence, or why no candidate
/// is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Main {
    Content(LoopContent),
    Empty(String),
}

/// The selected content, rebuilt only when the model's content changes (a tab, the layout, the
/// served data, the column crossing the split threshold). A desktop draws nothing until its
/// column is measured, so it never shows split rows it has to take back.
fn content(cx: &Ctx, side_below: bool) -> AnyElement {
    let page = cx.page;
    let split = cx.split;
    let main = computed(move || {
        split.get()?;
        page.with(|page| {
            Some(match &page.detail {
                Some(detail) => Main::Content(detail.content.clone()),
                None => Main::Empty(page.empty.clone().unwrap_or_default()),
            })
        })
    });
    let cx = cx.clone();
    dynamic(move || {
        Some(match main.get()? {
            Main::Content(LoopContent::Brief(brief)) => brief::brief(&cx, &brief, side_below),
            Main::Content(LoopContent::Evidence(body)) => evidence(&cx, &body),
            Main::Empty(line) => list::empty_line(cx.app, line),
        })
    })
}

/// The Diff, Gate log, Report or Artifacts tab, padded as a block (`.blk`); the Diff tab led by
/// its layout control.
fn evidence(cx: &Ctx, body: &Body) -> AnyElement {
    let app = cx.app;
    let content = match body {
        Body::Diff(files) => view()
            .child(head::layout_switch(cx))
            .child(diff::diff(cx, files))
            .into_element(),
        Body::Report(report) => evidence::report(app, report),
        Body::Artifacts(list) => evidence::artifacts(cx, list),
        Body::Log { lines, note } => evidence::log(app, lines, note),
        Body::Notice(line) => dim(app, line.clone()).into_element(),
    };
    match app.form {
        Form::Desktop => view()
            .px(px(20.0))
            .py(px(16.0))
            .child(content)
            .into_element(),
        Form::Phone => view()
            .px(px(14.0))
            .pt(px(12.0))
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

/// A model button as the kit's button, with what it does beyond its label as its hint.
fn command(cx: &Ctx, button: &review::Button) -> AnyElement {
    command_hinted(cx, button, button.hint.clone())
}

/// A model button as the kit's button (its emphasis, the decide bar's wide primary on a phone,
/// the speech bubble on Ask, the danger outline on Land anyway) with `hint` on pointer rest and
/// keyboard focus. A phone has no hover: there the caller says what needs saying in the page.
fn command_hinted(cx: &Ctx, button: &review::Button, hint: Option<String>) -> AnyElement {
    let app = cx.app;
    let kind = match button.style {
        ButtonStyle::Primary => ButtonKind::Primary,
        ButtonStyle::Ghost => ButtonKind::Ghost,
        ButtonStyle::Quiet => ButtonKind::Quiet,
    };
    let approve = button.action == Action::Approve && app.form != Form::Phone;
    let focus = approve.then(|| {
        let handle = element_ref();
        app.focus_when_asked(review::APPROVE, handle);
        handle
    });
    let options = ButtonOptions {
        icon: match button.action {
            Action::AskAbout { .. } | Action::AskCandidate => Some(Icon::Ask),
            Action::OpenSession | Action::OpenReviewer => Some(Icon::Agents),
            _ => None,
        },
        wide: app.form == Form::Phone && button.style == ButtonStyle::Primary,
        danger: button.action == Action::LandAnyway,
        key_hint: approve.then_some("A"),
        focus,
        ..ButtonOptions::default()
    };
    let control = button_with(
        app,
        button.label.clone(),
        kind,
        options,
        button_id(&button.action),
        act(cx, button.action.clone()),
    );
    match hint {
        Some(hint) if app.form != Form::Phone => {
            hint_on(app, view().flex_row().flex_shrink_0().child(control), hint).into_element()
        }
        _ => control,
    }
}

/// The test id of a control by its action.
fn button_id(action: &Action) -> String {
    match action {
        Action::Approve => review::APPROVE.to_owned(),
        Action::LandAnyway => "btn-land-anyway".to_owned(),
        Action::Gate => "btn-gate".to_owned(),
        Action::StartReview => "btn-review".to_owned(),
        Action::Verb(verb) => format!("btn-{}", verb.slug()),
        Action::AskAbout { path, .. } => format!("btn-ask-{}", slug(path)),
        Action::Expand(path) => format!("btn-expand-{}", slug(path)),
        Action::Collapse(path) => format!("btn-collapse-{}", slug(path)),
        Action::Pick(id) => format!("task-{id}"),
        Action::Tab(_) => "seg-review".to_owned(),
        Action::Layout(_) => "seg-layout".to_owned(),
        Action::Dismiss(_) => "btn-dismiss".to_owned(),
        Action::OpenArtifact(key) | Action::CloseArtifact(key) => {
            format!("btn-artifact-{}", slug(key))
        }
        Action::Refresh => "btn-refresh".to_owned(),
        Action::AskCandidate => "btn-ask-candidate".to_owned(),
        Action::OpenSession => "btn-open-session".to_owned(),
        Action::OpenReviewer => "btn-open-reviewer".to_owned(),
        Action::Onto(_) => "field-onto".to_owned(),
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

/// The model's [`Rich`] text at `size` in `ink`, for the small runs the kit's two paragraph
/// sizes do not cover (11.5 pt file heads and attributions, 12.5 pt comments). `mono` sets the
/// whole run in the mono font; otherwise mono spans take 0.93 of the size. Bold spans are ink,
/// dim spans ink-3, a hue its colour. A terminal keeps its own font and size.
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
