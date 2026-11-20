//! A task's review candidate as the task page draws it: the decide bar with its Onto target, the
//! done summary, the stored diff and the checks, each read from `model::review` for the one task
//! the page shows (`loop_page_filtered` with that task as its focus) and each control sent back
//! through the review reducer (`apply_live_filtered`). The former Review destination draws the
//! same pieces from here until it goes.

pub(crate) mod brief;
pub(crate) mod decide;
pub(crate) mod diff;
pub(crate) mod evidence;
mod onto;
pub(crate) mod rounds;
mod status;

use corral_protocol::detail::DetailRef;
use nsh_core::element::StyleRefinement;
use nsh_core::prelude::*;

use crate::model::act;
use crate::model::daemon::{Feed, Link, Task};
use crate::model::projects::ProjectFilter;
use crate::model::review::{self, Action, ButtonStyle, Layout, LoopPage, Tab};
use crate::model::work::{Press, Record};
use crate::model::{Form, Hue, Id, Rich, Selection};
use crate::screens::now::parts::press;
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with, seg};
use crate::ui::kit::hint::hint_on;
use crate::ui::kit::marks::hue_ink;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// `.mono` is 0.93 of the text around it.
const MONO_SCALE: f32 = 0.93;

/// What every piece of one candidate shares: the app, the task it was shown for (else the
/// selected one), whether its column holds split diff rows, and the model's page.
#[derive(Clone)]
pub(crate) struct Ctx {
    pub(crate) app: AppState,
    pub(crate) focus: Option<Id>,
    /// Desktop: whether the candidate's column is wide enough for side-by-side rows, unknown
    /// until it is measured; elsewhere never.
    pub(crate) split: Signal<Option<bool>>,
    pub(crate) page: Memo<LoopPage>,
}

impl Ctx {
    pub(crate) fn new(app: AppState, focus: Option<Id>) -> Ctx {
        Ctx::showing(app, focus, None)
    }

    /// The candidate with `tab`'s content whatever tab the review state last chose: the task
    /// page's Changes tab is always the diff.
    pub(crate) fn showing(app: AppState, focus: Option<Id>, tab: Option<Tab>) -> Ctx {
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
                loop_page(app, focus.as_ref(), width, tab)
            })
        };
        Ctx {
            app,
            focus,
            split,
            page,
        }
    }

    /// Measures a desktop column `width` points wide against the split rule.
    pub(crate) fn measured(&self, width: f32) {
        if self.app.form != Form::Desktop {
            return;
        }
        let fits = fits_split(self.app, width);
        if self.split.with_untracked(|now| *now != Some(fits)) {
            self.split.set(Some(fits));
        }
    }
}

/// The former Review screen's own project scope, tracked.
pub(crate) fn scope(app: AppState) -> ProjectFilter {
    app.scope_filter()
}

/// The model's page at `width` (tracked: state, connection, selection, scope and feed).
fn loop_page(app: AppState, focus: Option<&Id>, width: f32, tab: Option<Tab>) -> LoopPage {
    let env = app.env();
    let selection = app.selection();
    let filter = scope(app);
    app.feed.with(|feed| {
        app.review.with(|state| match tab {
            Some(tab) if state.tab != tab => {
                let state = review::State {
                    tab,
                    ..state.clone()
                };
                review::loop_page_filtered(&state, feed, &selection, focus, env, width, &filter)
            }
            _ => review::loop_page_filtered(state, feed, &selection, focus, env, width, &filter),
        })
    })
}

/// Whether a candidate column `width` points wide holds side-by-side rows, by the model's rule.
pub(crate) fn fits_split(app: AppState, width: f32) -> bool {
    let env = untracked(|| app.env());
    let wanted = review::State {
        layout: Layout::Split,
        ..review::State::default()
    };
    review::effective_layout(&wanted, env, width) == Layout::Split
}

/// Runs a model action through the feed-backed reducer and the app's effects. A shared verb
/// (Request changes) runs through the work state instead, whose composer asks for its words.
pub(crate) fn act(cx: &Ctx, action: Action) -> impl Fn() + 'static {
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
pub(crate) fn keep_current(cx: &Ctx) {
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

/// The candidate shown: the one it was shown for, or the selected one in scope.
pub(crate) fn shown_task<'a>(
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

/// The Unified | Side by side control (`.dtools`), `seg-layout`, offered over the diff only
/// where the candidate's column is wide enough for side by side (the model's rule): narrower,
/// the diff is unified and a control whose other choice could not show would mislead.
pub(crate) fn layout_switch(cx: &Ctx) -> AnyElement {
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

/// A model button as the kit's button, with what it does beyond its label as its hint.
pub(crate) fn command(cx: &Ctx, button: &review::Button) -> AnyElement {
    command_hinted(cx, button, button.hint.clone())
}

/// A model button as the kit's button (its emphasis, the decide bar's wide primary on a phone,
/// the speech bubble on Ask, the danger outline on Land anyway) with `hint` on pointer rest and
/// keyboard focus. A phone has no hover: there the caller says what needs saying in the page.
pub(crate) fn command_hinted(
    cx: &Ctx,
    button: &review::Button,
    hint: Option<String>,
) -> AnyElement {
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
pub(crate) fn button_id(action: &Action) -> String {
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
pub(crate) fn rich_at(app: AppState, rich: &Rich, size: f32, ink: Token, mono: bool) -> Text {
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
pub(crate) fn ring(s: StyleRefinement) -> StyleRefinement {
    s.outline(px(2.0))
        .outline_color(color(Token::Focus))
        .outline_offset(px(2.0))
}
