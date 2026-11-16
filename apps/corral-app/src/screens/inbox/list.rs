//! The Inbox list (needs-you rows, then what changed since the last visit), the phone's cards,
//! and the desktop pane of the selected row. A served question's row carries its record verbs
//! (escalate, cancel, deadline) on its context menu.

use corral_protocol::app::{CHANGES_V1, ChangesQuery};
use nsh_core::prelude::*;

use super::feed::{feed_row, more_row};
use super::{
    attention, blind, button, code, live, press, rich_at, ring, sheet, with_source, words,
};
use crate::model::daemon::Link;
use crate::model::inbox::{
    self, Action, EmptyState, InboxList, InboxPreview, ItemPreview, NeedYouItem, Notice,
    PlanPreview,
};
use crate::model::projects::FilterScreen;
use crate::model::work::Record;
use crate::model::{Effect, Form, Hue, Id, Rich, flat};
use crate::screens::assistant;
use crate::screens::now::with_menu;
use crate::state::AppState;
use crate::ui::kit::chip::chip;
use crate::ui::kit::marks::{dot, hue_ink};
use crate::ui::kit::notice::{empty, error, loading, notice, skeleton};
use crate::ui::kit::page::{self, group_head, pane};
use crate::ui::kit::project_filter::project_filter;
use crate::ui::kit::text::{h3, rich};
use crate::ui::term;
use crate::ui::tokens::{Token, color};

/// Placeholder bars of a loading desktop row and phone card (`.skel` widths and heights).
const ROW_BARS: [(f32, f32); 3] = [(0.4, 10.0), (0.85, 10.0), (0.6, 10.0)];
const CARD_BARS: [(f32, f32); 3] = [(0.4, 10.0), (0.9, 16.0), (0.6, 10.0)];
const PANE_BARS: [(f32, f32); 3] = [(0.3, 10.0), (0.7, 18.0), (1.0, 90.0)];
/// The changes the list previews: eight on desktop, six on the phone.
const CHANGES_PREVIEW: u16 = 8;

/// Fetch the newest changes once when none have arrived, so "Since your last visit" shows the
/// feed's history rather than nothing.
pub(super) fn load_changes(app: AppState) {
    let wanted = computed(move || {
        app.feed.with(|feed| {
            feed.changes.is_empty() && feed.capable(CHANGES_V1) && feed.link == Link::Up
        })
    });
    let asked = stored(false);
    effect(move || {
        if wanted.get() && !asked.replace(true) {
            untracked(|| {
                app.run(vec![Effect::Changes(ChangesQuery {
                    before: None,
                    after: None,
                    limit: CHANGES_PREVIEW,
                })]);
            });
        }
    });
}

/// The desktop and terminal list column: the Inbox's project scope over the rows, in a scrolling
/// pane, rows made again as the model changes. It is the destination's keyboard list
/// (`AppState::list_keys`).
pub(super) fn column(app: AppState) -> AnyElement {
    let list = blind(app, inbox::inbox_list_filtered);
    let revealed = stored(None::<String>);
    let keyed = view()
        .element_ref(&app.list_keys)
        .when(app.form == Form::Desktop, |v| v.pb(px(14.0)))
        .role(AccessibilityRole::List)
        .child(live(list, move |list| list_body(app, list, revealed)))
        .into_element();
    // The chip bar sits above the keyboard list, not in it: a focused chip keeps Enter and Space.
    let content = view()
        .when(app.form == Form::Desktop, |v| v.pt(px(6.0)))
        .child(project_filter(app, FilterScreen::Inbox))
        .child(keyed)
        .into_element();
    pane(app, "inbox-list", content)
}

/// The phone tab root: the large title, then the project scope, the cards and the feed in one
/// scroll.
pub(super) fn phone(app: AppState) -> AnyElement {
    let title = untracked(|| {
        let env = app.env();
        with_source(app, |state, src, filter| {
            inbox::inbox_filtered(state, src, env, filter).title
        })
    });
    let list = blind(app, inbox::inbox_list_filtered);
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .bg(color(Token::Bg))
        .child(page::head(app, title, None, vec![page::search(app)]))
        .child(pane(
            app,
            "inbox-list",
            view()
                .child(project_filter(app, FilterScreen::Inbox))
                .child(live(list, move |list| phone_body(app, list)))
                .into_element(),
        ))
        .into_element()
}

/// The pane of the selected row: the handover, an assistant prompt, the plan or item preview,
/// or the loading, empty and failed states. Only a change of kind replaces the pane;
/// the handover updates band by band. `compact`: the pane is narrow (a terminal, a small
/// desktop window), so the handover stacks its bands' parts and keeps the decide bar to one row.
pub(super) fn preview(app: AppState, compact: bool) -> AnyElement {
    sheet::load(app, None);
    let shown = blind(app, inbox::inbox_preview_filtered);
    let kind = computed(move || shown.with(Kind::of));
    view()
        .flex_1()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .flex_col()
        .child(dynamic(move || {
            let kind = kind.get();
            Some(untracked(move || match kind {
                Kind::Question => sheet::inline(app, compact),
                Kind::Prompt { thread, key } => assistant::live_prompt(app, thread, key),
                Kind::Other => pane(
                    app,
                    "inbox-preview",
                    live(shown, move |shown| other_pane(app, shown)),
                ),
            }))
        }))
        .into_element()
}

#[derive(Clone, PartialEq)]
enum Kind {
    Question,
    Prompt { thread: Id, key: Id },
    Other,
}

impl Kind {
    fn of(preview: &InboxPreview) -> Kind {
        match preview {
            InboxPreview::Question(_) => Kind::Question,
            InboxPreview::Prompt { thread, key } => Kind::Prompt {
                thread: thread.clone(),
                key: key.clone(),
            },
            _ => Kind::Other,
        }
    }
}

/// Placeholder rows while the list loads: plain bars for all but the last, whose bars carry the
/// model's caption as the region's status ("Loading inbox…"), so it is said once, at the end.
fn loading_rows(
    app: AppState,
    count: u8,
    bars: &[(f32, f32)],
    caption: &'static str,
    frame: impl Fn(AnyElement) -> AnyElement,
) -> Vec<AnyElement> {
    (0..count)
        .map(|at| {
            frame(if at + 1 == count {
                loading(app, caption, bars)
            } else {
                skeleton(app, bars)
            })
        })
        .collect()
}

/// The list's rows; `revealed` is the row last scrolled into view as the selection.
fn list_body(app: AppState, list: InboxList, revealed: Stored<Option<String>>) -> AnyElement {
    let form = app.form;
    match list {
        InboxList::Loading {
            skeleton_count,
            blocks_per_item,
            caption,
        } => {
            let bars = &ROW_BARS[..usize::from(blocks_per_item).min(ROW_BARS.len())];
            view()
                .children(loading_rows(app, skeleton_count, bars, caption, |bars| {
                    view()
                        .when(form == Form::Desktop, |v| v.px(px(16.0)).py(px(10.0)))
                        .when(form == Form::Terminal, |v| {
                            v.px(term::cells(1.0)).pb(px(16.0))
                        })
                        .child(bars)
                        .into_element()
                }))
                .into_element()
        }
        InboxList::Ready {
            label,
            error: refresh,
            needs_title,
            needs_count,
            items,
            empty: nothing,
            since_title,
            last_visit,
            feed,
            all_changes,
            ..
        } => view()
            .label(label)
            .children(refresh.map(|notice| error_box(app, notice)))
            .child(group_head(
                app,
                needs_title,
                Some(needs_count.to_string()),
                Hue::You,
                None,
                None,
            ))
            .children(nothing.map(|state| empty_state(app, state)))
            .children(
                items
                    .into_iter()
                    .map(|item| selectable(app, item, revealed)),
            )
            .child(
                view()
                    .when(form == Form::Desktop, |v| v.mt(px(6.0)))
                    .child(group_head(
                        app,
                        since_title,
                        last_visit,
                        Hue::Neutral,
                        None,
                        None,
                    )),
            )
            .child(
                view()
                    .when(form == Form::Desktop, |v| v.py(px(4.0)))
                    .children(feed.into_iter().map(|change| feed_row(app, change))),
            )
            .child(more_row(app, &all_changes))
            .into_element(),
    }
}

fn phone_body(app: AppState, list: InboxList) -> AnyElement {
    match list {
        InboxList::Loading {
            skeleton_count,
            blocks_per_item,
            caption,
        } => {
            let bars = &CARD_BARS[..usize::from(blocks_per_item).min(CARD_BARS.len())];
            view()
                .px(px(14.0))
                .py(px(12.0))
                .gap(px(10.0))
                .children(loading_rows(app, skeleton_count, bars, caption, |bars| {
                    view()
                        .p(px(16.0))
                        .rounded(px(14.0))
                        .border_1()
                        .border_color(color(Token::Rule))
                        .child(bars)
                        .into_element()
                }))
                .into_element()
        }
        InboxList::Ready {
            label,
            error: refresh,
            needs_title,
            needs_count,
            items,
            empty: nothing,
            since_title,
            changes_count,
            feed,
            all_changes,
            ..
        } => view()
            .label(label)
            .children(refresh.map(|notice| error_box(app, notice)))
            // The phone's empty state stands in for the whole group, head included.
            .children(nothing.map(|state| empty_state(app, state)))
            .when(!items.is_empty(), |v| {
                v.child(group_head(
                    app,
                    needs_title,
                    Some(needs_count.to_string()),
                    Hue::You,
                    None,
                    None,
                ))
            })
            .children(items.into_iter().map(|item| card(app, item)))
            .child(group_head(
                app,
                since_title,
                Some(changes_count.to_string()),
                Hue::Neutral,
                None,
                Some(button(app, &all_changes)),
            ))
            .children(feed.into_iter().map(|change| feed_row(app, change)))
            .into_element(),
    }
}

/// The refresh error over the retained rows: its bold runs in the failure colour (`.err b`).
fn error_box(app: AppState, notice: Notice) -> AnyElement {
    let body: Rich = notice
        .body
        .into_iter()
        .map(|run| if run.bold { run.hue(Hue::Fail) } else { run })
        .collect();
    error(app, &body, notice.retry.map(|retry| button(app, &retry)))
}

fn empty_state(app: AppState, state: EmptyState) -> AnyElement {
    empty(
        app,
        state.title,
        state.body,
        state.action.map(|action| button(app, &action)),
    )
}

fn row_label(item: &NeedYouItem) -> String {
    let mut label = format!(
        "{}, {}, {}, {}",
        flat(&item.heading),
        item.title,
        item.meta,
        item.age
    );
    if let Some(project) = &item.project_chip {
        label = format!("{label}, {project}");
    }
    if let Some(line) = &item.attention {
        label = format!("{label}, {}: {}", line.whose, line.reason);
    }
    label
}

/// The item's quiet project chip, while the list spans projects.
fn project(app: AppState, item: &NeedYouItem) -> Option<AnyElement> {
    item.project_chip
        .clone()
        .map(|project| chip(app, project, Hue::Neutral))
}

/// A served question's record verbs on its row's context menu; any other row as it is.
fn with_verbs(app: AppState, item_action: &Action, question: bool, row: AnyElement) -> AnyElement {
    match item_action {
        Action::Row(key) | Action::OpenRow(key) if question => {
            with_menu(app, Record::Question(key.clone()), row)
        }
        _ => row,
    }
}

/// A needs-you row. The selected one, when the selection has just moved to it, scrolls into the
/// list's view, so a keyboard move never leaves it off screen; one already in view stays put. The
/// row counts as revealed only once the scroll has run: a rebuild of the rows before then (the
/// feed's next revision) drops the pending scroll with the row it was for, and the rebuilt row
/// asks again.
fn selectable(app: AppState, item: NeedYouItem, revealed: Stored<Option<String>>) -> AnyElement {
    if !item.selected {
        return row(app, item);
    }
    let handle = element_ref();
    if revealed.with(|shown| shown.as_deref() != Some(item.test_id.as_str())) {
        let id = item.test_id.clone();
        after_answer(move || {
            revealed.replace(Some(id));
            handle.reveal(RevealOptions::default().vertical(ScrollAlignment::Nearest));
        });
    }
    view()
        .element_ref(&handle)
        .child(row(app, item))
        .into_element()
}

/// A needs-you row. Desktop: the dot, then the lead line, the title in semibold, the attention
/// move (whose move, why) and the meta line (`.row` at auto height, padding 9 14 9 16), the age
/// at the trailing edge; selected is the sel fill with the 1 pt ink bar. Terminal:
/// [`terminal_row`].
fn row(app: AppState, item: NeedYouItem) -> AnyElement {
    let selected = item.selected;
    let label = row_label(&item);
    let on_press = press(app, item.action.clone());
    let action = item.action.clone();
    let question = item.state.is_some();
    if app.form == Form::Terminal {
        let drawn = terminal_row(app, item, label, on_press);
        return with_verbs(app, &action, question, drawn);
    }
    let move_line = item
        .attention
        .as_ref()
        .map(|line| attention::summary(app, line, 12.0, 1).truncate());
    let tag = project(app, &item);
    let drawn = view()
        .relative()
        .flex_row()
        .items_start()
        .gap(px(8.0))
        .pl(px(16.0))
        .pr(px(14.0))
        .py(px(9.0))
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(label)
        .accessibility_state(AccessibilityState {
            selected,
            ..AccessibilityState::default()
        })
        .test_id(&item.test_id)
        .on_press(on_press)
        .when(selected, |v| v.bg(color(Token::Sel)))
        .when(!selected, |v| v.hover(|s| s.bg(color(Token::Hover))))
        .focus_visible(ring)
        .when(selected, |v| {
            v.child(
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
                .pt(px(4.0))
                .flex_shrink_0()
                .child(dot(app, item.hue)),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(1.0))
                .child(rich_at(app, &item.heading, 11.5, color(Token::Ink3)).number_of_lines(1u32))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .min_w(px(0.0))
                        .child(
                            words(app, item.title, 13.0, Token::Ink)
                                .flex_shrink(1.0)
                                .min_w(px(0.0))
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD),
                        )
                        .children(tag),
                )
                .children(move_line)
                .child(words(app, item.meta, 12.0, Token::Ink3).truncate()),
        )
        .child(code(app, item.age, 11.0, Token::Ink3).flex_shrink_0())
        .into_element();
    with_verbs(app, &action, question, drawn)
}

/// A needs-you row in Cells, on two rows so the title keeps the width: the lead line (lead and
/// id) quiet above, then the dot, the title taking the room and the age; the attention move on
/// a third row when corrald records one. A one-Cell bar marks the selected row; NSH bands it. The
/// project chip is not drawn here (the title keeps the width); the row's label names the project.
fn terminal_row(
    app: AppState,
    item: NeedYouItem,
    label: String,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    let selected = item.selected;
    let rows = if item.attention.is_some() { 3 } else { 2 };
    let bar = if selected {
        vec!["▌"; rows].join("\n")
    } else {
        " ".to_owned()
    };
    view()
        .flex_row()
        .flex_shrink_0()
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(label)
        .accessibility_state(AccessibilityState {
            selected,
            ..AccessibilityState::default()
        })
        .test_id(&item.test_id)
        .on_press(on_press)
        .child(
            view()
                .w(term::cells(1.0))
                .flex_shrink_0()
                .accessibility_elements_hidden(true)
                .child(text(bar).text_color(color(Token::Ink))),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(
                    view().flex_row().child(term::space(2.0)).child(
                        term::quiet(flat(&item.heading))
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate(),
                    ),
                )
                .child(
                    view()
                        .flex_row()
                        .child(dot(app, item.hue))
                        .child(term::space(1.0))
                        .child(
                            text(item.title)
                                .flex_1()
                                .min_w(px(0.0))
                                .truncate()
                                .text_color(color(Token::Ink)),
                        )
                        .child(term::space(1.0))
                        .child(term::quiet(item.age).flex_shrink_0()),
                )
                .children(item.attention.as_ref().map(|line| {
                    view()
                        .flex_row()
                        .child(term::space(2.0))
                        .child(attention::summary(app, line, 12.0, 1).flex_1().truncate())
                })),
        )
        .into_element()
}

/// A phone needs-you card (`.pcard-q`): the dot and the bold lead line in the item's hue, the
/// 17 pt title, the meta line, the attention move under it (two lines at most); pressing it
/// opens the item, holding it offers a served question's verbs.
fn card(app: AppState, item: NeedYouItem) -> AnyElement {
    let lead_ink = match item.hue {
        Hue::Neutral => color(Token::Ink2),
        hue => hue_ink(hue),
    };
    let heading: Rich = item.heading.iter().cloned().map(|run| run.bold()).collect();
    let move_line = item.attention.as_ref().map(|line| {
        attention::summary(app, line, 13.5, 2)
            .line_height(px(19.0))
            .mt(px(6.0))
    });
    let action = item.action.clone();
    let question = item.state.is_some();
    let drawn = view()
        .mx(px(14.0))
        .mt(px(6.0))
        .mb(px(4.0))
        .px(px(16.0))
        .py(px(14.0))
        .rounded(px(14.0))
        .bg(color(Token::Surface))
        .border_1()
        .border_color(color(Token::Rule2))
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(row_label(&item))
        .test_id(&item.test_id)
        .on_press(press(app, item.action.clone()))
        .active(|s| s.bg(color(Token::Hover)))
        .focus_visible(ring)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(dot(app, item.hue))
                .child(rich_at(app, &heading, 12.5, lead_ink).number_of_lines(1u32))
                .child(view().flex_1())
                .children(project(app, &item)),
        )
        .child(
            words(app, item.title, 17.0, Token::Ink)
                .font_weight(FontWeight::BOLD)
                .line_height(px(22.0))
                .letter_spacing(px(-0.17))
                .mt(px(6.0))
                .mb(px(6.0)),
        )
        .child(words(app, item.meta, 13.5, Token::Ink2).line_height(px(19.0)))
        .children(move_line)
        .into_element();
    with_verbs(app, &action, question, drawn)
}

/// The padding of a preview pane's body, per form.
fn pane_body(app: AppState) -> View {
    view()
        .when(app.form == Form::Desktop, |v| v.px(px(24.0)).py(px(16.0)))
        .when(app.form == Form::Terminal, |v| v.px(term::cells(1.0)))
}

/// The attention move at the top of a pane's body, a rule's distance from what follows.
fn move_block(app: AppState, line: Option<inbox::AttentionLine>) -> Option<AnyElement> {
    line.map(|line| {
        view()
            .when(app.form != Form::Terminal, |v| v.mb(px(16.0)))
            .child(attention::block(app, line))
            .when(app.form == Form::Terminal, |v| v.child(term::blank()))
            .into_element()
    })
}

/// The panes other than the handover and the assistant's prompts.
fn other_pane(app: AppState, shown: InboxPreview) -> AnyElement {
    match shown {
        InboxPreview::Loading {
            skeleton_count,
            caption,
        } => view()
            .p(px(if app.form == Form::Terminal {
                0.0
            } else {
                24.0
            }))
            .child(loading(
                app,
                caption,
                &PANE_BARS[..usize::from(skeleton_count).min(PANE_BARS.len())],
            ))
            .into_element(),
        InboxPreview::Empty(state) => view()
            .when(app.form == Form::Desktop, |v| v.px(px(12.0)).py(px(20.0)))
            .child(empty_state(app, state))
            .into_element(),
        InboxPreview::Failed(failure) => pane_body(app)
            .child(notice(
                app,
                failure.hue,
                &failure.body,
                failure.retry.map(|retry| button(app, &retry)),
            ))
            .into_element(),
        InboxPreview::Plan(plan) => plan_pane(app, plan),
        InboxPreview::Item(item) => item_pane(app, item),
        InboxPreview::Question(_) | InboxPreview::Prompt { .. } => view().into_element(),
    }
}

/// A plan awaiting acceptance: the head with its chips, the attention move, the goal when known,
/// what accepting does, Open plan review.
fn plan_pane(app: AppState, plan: PlanPreview) -> AnyElement {
    let mut meta: Vec<AnyElement> = plan
        .chips
        .iter()
        .map(|state| chip(app, state.label, state.hue))
        .collect();
    meta.push(code(app, plan.record, 12.0, Token::Ink3).into_element());
    view()
        .child(page::qhead(app, meta, plan.title, None, Vec::new()))
        .child(
            pane_body(app)
                .children(move_block(app, plan.attention))
                .children(plan.goal.map(|goal| {
                    words(app, goal, 13.0, Token::Ink)
                        .line_height(px(20.0))
                        .max_w(px(640.0))
                }))
                .child(h3(app, plan.consequence_title, None))
                .child(view().max_w(px(640.0)).child(rich(app, &plan.consequence)))
                .child(
                    view()
                        .flex_row()
                        .mt(px(16.0))
                        .child(button(app, &plan.open)),
                ),
        )
        .into_element()
}

/// A served review, stuck task or escalation: its head, why it waits and what comes next, its
/// recorded facts, its own page.
fn item_pane(app: AppState, item: ItemPreview) -> AnyElement {
    let mut meta: Vec<AnyElement> = item
        .chips
        .iter()
        .map(|state| chip(app, state.label, state.hue))
        .collect();
    meta.push(code(app, item.record, 12.0, Token::Ink3).into_element());
    view()
        .child(page::qhead(app, meta, item.title, None, Vec::new()))
        .child(
            pane_body(app)
                .gap(px(4.0))
                .children(move_block(app, item.attention))
                .children(
                    item.facts
                        .into_iter()
                        .map(|fact| words(app, fact, 13.0, Token::Ink2).line_height(px(20.0))),
                )
                .children(
                    item.open
                        .map(|open| view().flex_row().mt(px(16.0)).child(button(app, &open))),
                ),
        )
        .into_element()
}
