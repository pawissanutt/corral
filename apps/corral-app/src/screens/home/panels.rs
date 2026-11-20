//! The dashboard's right half: what Corral is doing. Working as lanes over today, then tasks by
//! status, the projects (or the scoped project's facts), today's cost, the latest events, what
//! just finished and the burn-up. Every panel links to the page holding its detail; a project
//! row scopes every page. Each panel follows its own facts.

use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

use crate::model::home::{self, Lane};
use crate::model::hue::Hue;
use crate::model::projects::Scope;
use crate::model::status::status_look;
use crate::model::timeline::BurnupScope;
use crate::model::{Destination, Form};
use crate::state::AppState;
use crate::ui::kit::marks::hue_fill;
use crate::ui::kit::status::look_mark;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

use super::parts::{Link, bar, empty, figure, line, panel, words};

/// How many events the Latest events panel lists.
const LATEST: usize = 6;

fn go(app: AppState, words: impl Into<String>, to: Destination) -> Link {
    Link {
        words: words.into(),
        test_id: format!("link-{}", to.slug()),
        press: Box::new(move || app.go(to)),
    }
}

/// Keeps a detail the dashboard reads requested while it shows: again when the request changes
/// (a new day, the clock arriving).
fn keep(app: AppState, request: Memo<Option<DetailRef>>) {
    effect(move || {
        if app.daemon()
            && let Some(request) = request.get()
        {
            untracked(|| app.load(request));
        }
    });
}

/// A panel drawn again whenever what it shows changes.
fn following<T: Clone + PartialEq + 'static>(
    shown: Memo<T>,
    draw: impl Fn(T) -> AnyElement + 'static,
) -> AnyElement {
    dynamic(move || {
        let value = shown.get();
        untracked(|| draw(value))
    })
    .into_element()
}

/// Working as lanes over today: each working thing's runs, checks and waits on one track.
pub(super) fn working(app: AppState, scope: Memo<Scope>) -> AnyElement {
    let request = computed(move || app.feed.with(home::lanes_request));
    keep(app, request);
    let shown = computed(move || {
        let scope = scope.get();
        app.feed.with(|feed| home::lanes(feed, &scope))
    });
    following(shown, move |(lanes, from)| {
        let count = lanes.len();
        let body = if lanes.is_empty() {
            empty(app, "Nothing is working.")
        } else {
            view()
                .children(
                    lanes
                        .into_iter()
                        .map(|lane| lane_row(app, lane))
                        .collect::<Vec<_>>(),
                )
                .into_element()
        };
        panel(
            app,
            "panel-working",
            format!("{} {count}", home::WORKING),
            from.map(|from| format!("today, {from} to now")),
            Some(go(app, "Timeline", Destination::Timeline)),
            body,
        )
    })
}

fn lane_row(app: AppState, lane: Lane) -> AnyElement {
    let form = app.form;
    let live = lane.live.clone();
    let spoken = format!("{} {}, {}, {}", lane.id, lane.title, lane.word, lane.now);
    let track_width = match form {
        Form::Terminal => term::cells(16.0),
        Form::Desktop | Form::Phone => px(150.0),
    };
    let names = view()
        .flex_1()
        .min_w(px(0.0))
        .child(words(form, lane.title, 12.5, Token::Ink).font_weight(FontWeight::SEMIBOLD))
        .when(form != Form::Terminal && !lane.now.is_empty(), |names| {
            names.child(words(form, lane.now.clone(), 11.5, Token::Ink3))
        });
    line(
        app,
        crate::model::list::row_id(&lane.live),
        spoken,
        vec![
            look_mark(app, lane.look, lane.word),
            figure(form, lane.id.to_string(), 12.0, Token::Ink2).into_element(),
            names.into_element(),
            track(form, &lane.track, track_width),
            figure(form, lane.age, 11.5, Token::Ink3).into_element(),
        ],
        move || app.open_live(live.clone()),
    )
}

/// A lane's track over the plotted part of today: each span in its state's hue; an instant
/// keeps a sliver.
fn track(form: Form, segments: &[home::Segment], width: Points) -> AnyElement {
    if form == Form::Terminal {
        return cell_track(segments);
    }
    let sliver = px(2.0);
    view()
        .relative()
        .flex_shrink_0()
        .w(width)
        .h(match form {
            Form::Terminal => term::rows(1.0),
            Form::Desktop | Form::Phone => px(8.0),
        })
        .when(form != Form::Terminal, |track| {
            track.rounded(px(2.0)).bg(color(Token::BgSunk))
        })
        .role(AccessibilityRole::Image)
        .label(
            segments
                .iter()
                .map(|segment| segment.label.clone())
                .collect::<Vec<_>>()
                .join(", "),
        )
        .children(
            segments
                .iter()
                .map(|segment| {
                    view()
                        .absolute()
                        .top(px(0.0))
                        .bottom(px(0.0))
                        .left(relative(segment.from))
                        .w(relative((segment.to - segment.from).max(0.0)))
                        .min_w(sliver)
                        .bg(hue_fill(segment.hue))
                        .into_element()
                })
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// How many Cells a terminal's track spans.
const TRACK_CELLS: usize = 16;

/// A lane's track in a terminal: one Cell per sixteenth of the plotted day, `━` in the hue of
/// the span covering it (the last one wins), `·` where nothing ran.
fn cell_track(segments: &[home::Segment]) -> AnyElement {
    let mut cells: Vec<Option<Hue>> = vec![None; TRACK_CELLS];
    for segment in segments {
        let first = (segment.from * TRACK_CELLS as f32).floor() as usize;
        let last = ((segment.to * TRACK_CELLS as f32).ceil() as usize).max(first + 1);
        for cell in cells.iter_mut().take(last.min(TRACK_CELLS)).skip(first) {
            *cell = Some(segment.hue);
        }
    }
    let mut runs: Vec<(Option<Hue>, usize)> = Vec::new();
    for cell in cells {
        match runs.last_mut() {
            Some((hue, count)) if *hue == cell => *count += 1,
            _ => runs.push((cell, 1)),
        }
    }
    view()
        .flex_row()
        .flex_shrink_0()
        .accessibility_elements_hidden(true)
        .children(runs.into_iter().map(|(hue, count)| {
            let (glyph, ink) = match hue {
                Some(hue) => ("━", crate::ui::kit::marks::hue_ink(hue)),
                None => ("·", color(Token::Ink4)),
            };
            text(glyph.repeat(count))
                .number_of_lines(1u32)
                .flex_shrink_0()
                .text_color(ink)
                .into_element()
        }))
        .into_element()
}

/// The open tasks in scope by status, each opening Tasks.
pub(super) fn by_status(app: AppState, scope: Memo<Scope>) -> AnyElement {
    let form = app.form;
    let shown = computed(move || {
        let scope = scope.get();
        app.feed.with(|feed| home::by_status(feed, &scope))
    });
    following(shown, move |(open, bars)| {
        let width = match form {
            Form::Terminal => term::cells(12.0),
            Form::Desktop | Form::Phone => px(90.0),
        };
        let rows: Vec<AnyElement> = bars
            .into_iter()
            .map(|status| {
                let look = status_look(status.status);
                line(
                    app,
                    format!("status-{}", status.status.as_str()),
                    format!("{} {}", status.count, status.word),
                    vec![
                        look_mark(app, look, status.word),
                        words(form, status.word, 12.5, Token::Ink2)
                            .flex_1()
                            .into_element(),
                        bar(form, status.fraction, look.hue, width),
                        figure(form, status.count.to_string(), 12.0, Token::Ink)
                            .min_w(px(18.0))
                            .text_right()
                            .into_element(),
                    ],
                    move || app.go(Destination::Tasks),
                )
            })
            .collect();
        panel(
            app,
            "panel-status",
            "Tasks by status".into(),
            None,
            Some(go(app, format!("{open} open"), Destination::Tasks)),
            view().children(rows).into_element(),
        )
    })
}

/// Every project with what waits and works in it (a press scopes every page to it), or the
/// scoped project's facts.
pub(super) fn projects(app: AppState, scope: Memo<Scope>) -> AnyElement {
    let form = app.form;
    let shown = computed(move || {
        let scope = scope.get();
        app.feed.with(|feed| match scope.project() {
            Some(id) => (Some(id.clone()), Vec::new(), home::project_facts(feed, id)),
            None => (None, home::projects(feed), Vec::new()),
        })
    });
    following(shown, move |(scoped, lines, facts)| {
        if scoped.is_some() {
            let rows: Vec<AnyElement> = facts
                .into_iter()
                .map(|(label, value)| {
                    view()
                        .flex_row()
                        .gap(px(8.0))
                        .min_w(px(0.0))
                        .when(form != Form::Terminal, |row| row.px(px(12.0)).py(px(3.0)))
                        .child(words(form, label, 12.5, Token::Ink3).w(px(96.0)).flex_shrink_0())
                        .child(words(form, value, 12.5, Token::Ink).flex_1())
                        .into_element()
                })
                .collect();
            return panel(
                app,
                "panel-project",
                "Project".into(),
                None,
                Some(go(app, "Settings", Destination::Settings)),
                view().pb(px(6.0)).children(rows).into_element(),
            );
        }
        let rows: Vec<AnyElement> = if lines.is_empty() {
            vec![empty(app, "corrald lists no project.")]
        } else {
            lines
                .into_iter()
                .map(|project| {
                    let id = project.id.clone();
                    let count = |n: usize| if n == 0 { "·".to_owned() } else { n.to_string() };
                    line(
                        app,
                        format!("project-{}", project.id),
                        format!(
                            "{}: {} need you, {} working, {} open; scope every page to it",
                            project.id, project.you, project.working, project.open
                        ),
                        vec![
                            words(form, project.id.to_string(), 12.5, Token::Ink)
                                .flex_1()
                                .into_element(),
                            figure(form, count(project.you), 12.0, if project.you > 0 {
                                Token::You
                            } else {
                                Token::Ink3
                            })
                            .into_element(),
                            figure(form, count(project.working), 12.0, Token::Ink2).into_element(),
                            figure(form, project.open.to_string(), 12.0, Token::Ink2)
                                .into_element(),
                        ],
                        move || app.set_scope(Scope::Project(id.clone())),
                    )
                })
                .collect()
        };
        panel(
            app,
            "panel-projects",
            "Projects".into(),
            Some("you · work · open".into()),
            None,
            view().children(rows).into_element(),
        )
    })
}

/// Today's cost by profile, as the agent hosts reported it.
pub(super) fn cost(app: AppState) -> AnyElement {
    let form = app.form;
    let request = computed(move || app.feed.with(home::cost_request));
    keep(app, request);
    let shown = computed(move || app.feed.with(home::cost));
    following(shown, move |cost| {
        let width = match form {
            Form::Terminal => term::cells(10.0),
            Form::Desktop | Form::Phone => px(80.0),
        };
        let body = match cost.empty {
            Some(said) if cost.rows.is_empty() => empty(app, said),
            _ => view()
                .children(
                    cost.rows
                        .into_iter()
                        .map(|row| {
                            line(
                                app,
                                format!("cost-{}", row.key),
                                format!("{} {}", row.key, row.cost),
                                vec![
                                    words(form, row.key, 12.5, Token::Ink2)
                                        .flex_1()
                                        .into_element(),
                                    bar(form, row.fraction, Hue::Neutral, width),
                                    figure(form, row.cost, 12.0, Token::Ink).into_element(),
                                ],
                                move || app.go(Destination::Usage),
                            )
                        })
                        .collect::<Vec<_>>(),
                )
                .into_element(),
        };
        panel(
            app,
            "panel-cost",
            format!("Cost today {}", cost.today),
            Some(cost.note),
            Some(go(app, "Usage", Destination::Usage)),
            body,
        )
    })
}

/// The newest events in scope, each opening its task.
pub(super) fn events(app: AppState, scope: Memo<Scope>) -> AnyElement {
    let form = app.form;
    let shown = computed(move || {
        let scope = scope.get();
        app.feed.with(|feed| home::latest(feed, &scope, LATEST))
    });
    following(shown, move |events| {
        let body = if events.is_empty() {
            empty(app, "No events loaded.")
        } else {
            view()
                .children(
                    events
                        .into_iter()
                        .enumerate()
                        .map(|(at, event)| {
                            let live = event.live.clone();
                            line(
                                app,
                                format!("event-{at}"),
                                format!("{} {} {}", event.at, event.id, event.text),
                                vec![
                                    figure(form, event.at, 11.5, Token::Ink3).into_element(),
                                    figure(form, event.id, 12.0, Token::Ink2).into_element(),
                                    words(form, event.text, 12.5, Token::Ink2)
                                        .flex_1()
                                        .into_element(),
                                ],
                                move || match &live {
                                    Some(live) => app.open_live(live.clone()),
                                    None => app.go(Destination::Feed),
                                },
                            )
                        })
                        .collect::<Vec<_>>(),
                )
                .into_element()
        };
        panel(
            app,
            "panel-events",
            "Latest events".into(),
            None,
            Some(go(app, "Feed", Destination::Feed)),
            body,
        )
    })
}

/// What landed or was dropped since the last visit.
pub(super) fn finished(app: AppState, scope: Memo<Scope>) -> AnyElement {
    let form = app.form;
    let shown = computed(move || {
        let scope = scope.get();
        app.feed.with(|feed| {
            let since = home::since(feed, &scope);
            let rows: Vec<_> = since
                .finished
                .iter()
                .filter_map(|id| home::row(feed, id))
                .collect();
            (since.at, rows)
        })
    });
    following(shown, move |(at, rows)| {
        let count = rows.len();
        let body = if rows.is_empty() {
            empty(app, "Nothing finished.")
        } else {
            view()
                .children(
                    rows.into_iter()
                        .map(|row| {
                            let live = row.live.clone();
                            let said = if row.title.is_empty() {
                                row.now.clone()
                            } else {
                                format!("{} · {}", row.title, row.now)
                            };
                            line(
                                app,
                                crate::model::list::row_id(&row.live),
                                row.spoken(),
                                vec![
                                    look_mark(app, row.look, row.word),
                                    figure(form, row.id.to_string(), 12.0, Token::Ink2)
                                        .into_element(),
                                    words(form, said, 12.5, Token::Ink2).flex_1().into_element(),
                                ],
                                move || app.open_live(live.clone()),
                            )
                        })
                        .collect::<Vec<_>>(),
                )
                .into_element()
        };
        panel(
            app,
            "panel-finished",
            format!("{} {count}", home::FINISHED),
            Some(at.map_or_else(
                || "since your last visit".to_owned(),
                |at| format!("since {at}"),
            )),
            None,
            body,
        )
    })
}

/// The burn-up of the scope: created against landed over the last days, as Activity ›
/// Timeline draws it.
pub(super) fn burnup(app: AppState, scope: Memo<Scope>) -> AnyElement {
    let request = computed(move || {
        let scope = match scope.get() {
            Scope::Project(id) => BurnupScope::Project(id.to_string()),
            Scope::All => BurnupScope::Workspace,
        };
        DetailRef::Burnup { scope: scope.key() }
    });
    following(request, move |request| {
        panel(
            app,
            "panel-burnup",
            "Burn-up".into(),
            None,
            Some(go(app, "Timeline", Destination::Timeline)),
            view()
                .when(app.form != Form::Terminal, |chart| chart.px(px(12.0)).pb(px(10.0)))
                .child(crate::screens::timeline::scoped_burnup(app, request))
                .into_element(),
        )
    })
}
