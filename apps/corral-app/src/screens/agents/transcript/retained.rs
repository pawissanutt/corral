use super::*;
use crate::model::session::Action;
use crate::model::session::Echo;
use crate::model::session::transcript::{Prose, Thinking};
use std::collections::HashMap;

#[derive(Clone, PartialEq, Eq, Hash)]
enum Source {
    Recorded(String),
    Echo(String),
    Live(String),
}

#[derive(Clone, PartialEq, Eq)]
enum Body {
    Message(Message, usize),
    Echo(Echo),
}

#[derive(Clone, PartialEq, Eq)]
struct Row {
    key: (String, Source, usize),
    body: Body,
}

fn rows(page: &Transcript) -> Vec<Row> {
    let scope = page.scope.to_string();
    let mut first = 0;
    let mut rows = Vec::new();
    let mut push = |source: Source, body| {
        rows.push(Row {
            key: (scope.clone(), source, 0),
            body,
        });
    };
    for message in &page.messages {
        push(
            Source::Recorded(message.anchor.clone()),
            Body::Message(message.clone(), first),
        );
        first += message.items.len() + usize::from(message.footer.is_some());
    }
    for echo in &page.echoes {
        push(Source::Echo(echo.id.clone()), Body::Echo(echo.clone()));
    }
    for message in &page.live {
        push(
            Source::Live(message.anchor.clone()),
            Body::Message(message.clone(), first),
        );
        first += message.items.len() + usize::from(message.footer.is_some());
    }
    // Older daemons can repeat a time/author anchor. Count its occurrences from
    // the tail so a matching legacy row prepended by paging keeps existing keys.
    let mut counts = HashMap::new();
    for row in rows.iter_mut().rev() {
        let count = counts.entry(row.key.1.clone()).or_insert(0);
        row.key.2 = *count;
        *count += 1;
    }
    rows
}

fn bind<T: Clone + PartialEq + 'static, E: IntoElements>(
    shown: Memo<Option<Transcript>>,
    project: impl Fn(&Transcript) -> T + 'static,
    draw: impl Fn(T) -> Option<E> + 'static,
) -> AnyElement {
    let value = computed(move || shown.with(|page| page.as_ref().map(&project)));
    dynamic(move || {
        value
            .get()
            .and_then(|value| untracked(|| draw(value)))
            .map(IntoElements::into_elements)
            .unwrap_or_default()
    })
}

pub(super) fn render(
    app: AppState,
    reader: &Reader,
    shown: Memo<Option<Transcript>>,
) -> AnyElement {
    provide_styles(app);
    let terminal = app.form == Form::Terminal;
    let sides = if app.form == Form::Phone { 16.0 } else { 24.0 };
    let edge = move |content| view().flex_row().px(px(sides)).py(px(4.0)).child(content);
    let stale = bind(
        shown,
        |p| p.stale.clone(),
        move |words| {
            let banner = notice(app, Hue::Held, &vec![span(words?)], None);
            Some(if terminal {
                banner
            } else {
                edge(banner).pb(px(8.0)).into_element()
            })
        },
    );
    let paging_reader = reader.clone();
    let older = bind(
        shown,
        |p| (p.older.clone(), p.older_cursor.clone(), p.paging),
        move |(action, cursor, paging)| {
            let action = action?;
            let reader = paging_reader.clone();
            let button = pager(
                app,
                move || reader.older(cursor.clone(), action.clone()),
                "Show earlier messages",
                "btn-older",
                paging,
            );
            Some(if terminal {
                view().flex_row().child(button).into_element()
            } else {
                edge(button).into_element()
            })
        },
    );
    let fill = bind(shown, |p| p.fill.clone(), move |value| fill(app, &value));
    let after = bind(
        shown,
        |p| {
            (!p.after.is_empty()).then(|| {
                let next = p
                    .messages
                    .iter()
                    .chain(&p.live)
                    .map(|m| m.items.len() + usize::from(m.footer.is_some()))
                    .sum::<usize>();
                (next, p.after.clone())
            })
        },
        move |after| {
            let (next, after) = after?;
            let column = body_column(
                app,
                after
                    .iter()
                    .enumerate()
                    .map(|(i, row)| item(app, row, next + i, None)),
            );
            Some(if terminal {
                view().child(term::blank()).child(column).into_element()
            } else {
                column
            })
        },
    );
    let note = bind(
        shown,
        |p| p.disclaimer.clone(),
        move |note| {
            let note = disclaimer(app, &note)?;
            Some(if terminal {
                vec![term::blank(), note]
            } else {
                vec![
                    view()
                        .px(px(sides))
                        .pt(px(4.0))
                        .pb(px(16.0))
                        .child(note)
                        .into_element(),
                ]
            })
        },
    );
    let paging_reader = reader.clone();
    let newer_button = computed(move || {
        let (scope, fallback, paging) = shown.with(|p| {
            let p = p.as_ref()?;
            Some((p.scope.clone(), p.newer.clone(), p.paging))
        })?;
        let (_, newer, loading) = paging_reader.status(&scope);
        (newer || fallback.is_some()).then_some((scope, fallback, loading || paging))
    });
    let paging_reader = reader.clone();
    let newer = dynamic(move || {
        let (scope, fallback, loading) = newer_button.get()?;
        let reader = paging_reader.clone();
        let button = pager(
            app,
            move || reader.newer(&scope, fallback.clone()),
            "Show newer messages",
            "btn-newer",
            loading,
        );
        Some(if terminal {
            view().flex_row().child(button).into_element()
        } else {
            edge(button).into_element()
        })
    });
    let geometry = reader.clone();
    let listing = computed(move || shown.with(|p| p.as_ref().map_or_else(Vec::new, rows)));
    let messages = each(
        listing,
        |row| row.key.clone(),
        move |row, i| {
            let (scope, body) = row.with_untracked(|r| (r.key.0.clone(), r.body.clone()));
            let scope = geometry.geometry(&scope);
            let block = match body {
                Body::Echo(_) => dynamic(move || {
                    row.with(|r| match &r.body {
                        Body::Echo(echo) => Some(untracked(|| bubbles::echo(app, echo))),
                        Body::Message(..) => None,
                    })
                }),
                Body::Message(message, _) => {
                    let anchor = message.anchor.clone();
                    let shown = computed(move || {
                        row.with(|r| match &r.body {
                            Body::Message(msg, first) => Some((msg.clone(), *first)),
                            Body::Echo(_) => None,
                        })
                    });
                    let block = message_row(app, scope.clone(), message, shown);
                    anchored(&scope, &anchor, block)
                }
            };
            if terminal {
                vec![when(move || i.get() > 0, term::blank).into_element(), block]
            } else {
                vec![block]
            }
        },
    );
    let column = view()
        .test_id("session-transcript-items")
        .child(stale)
        .child(older)
        .child(fill)
        .child(messages)
        .child(after)
        .child(newer)
        .child(note);
    if terminal {
        column.into_element()
    } else {
        view()
            .w_full()
            .items_center()
            .child(
                column
                    .w_full()
                    .max_w(px(MEASURE_CONVERSATION))
                    .pt(px(if app.form == Form::Phone { 8.0 } else { 16.0 }))
                    .pb(px(10.0)),
            )
            .into_element()
    }
}

fn message_row(
    app: AppState,
    scope: String,
    message: Message,
    shown: Memo<Option<(Message, usize)>>,
) -> AnyElement {
    let anchor = message.anchor.clone();
    let column = (message.keys.len() == message.items.len() && message.kind == MessageKind::Agent)
        .then(|| format!("{anchor}#items"));
    let column_of = |items: View| match &column {
        Some(key) => part(&scope, &anchor, key, None, items),
        None => items,
    };
    let items = || item_list(app, shown, scope.clone(), anchor.clone(), column.clone());
    let footer = move || {
        let shown = computed(move || {
            shown.with(|row| {
                let (m, first) = row.as_ref()?;
                Some((m.footer.clone()?, first + m.items.len()))
            })
        });
        dynamic(move || {
            shown
                .get()
                .map(|(f, n)| rows::footer(app, &f, &n.to_string()))
        })
    };
    match message.kind {
        MessageKind::Quiet => return body_column(app, [items(), footer()]),
        MessageKind::Bubble => {
            let metadata = computed(move || {
                shown.with(|row| {
                    row.as_ref().map(|(m, _)| Message {
                        items: Vec::new(),
                        keys: Vec::new(),
                        footer: None,
                        ..m.clone()
                    })
                })
            });
            return dynamic(move || {
                metadata.get().map(|m| {
                    untracked(|| {
                        bubbles::bubble(
                            app,
                            &m,
                            vec![
                                item_list(app, shown, scope.clone(), anchor.clone(), None),
                                footer(),
                            ],
                        )
                    })
                })
            });
        }
        MessageKind::Agent => {}
    }
    let head_words =
        computed(move || shown.with(|r| r.as_ref().map_or_else(Vec::new, |(m, _)| m.head.clone())));
    let live = computed(move || shown.with(|r| r.as_ref().is_some_and(|(m, _)| m.live)));
    agent_message(
        app,
        &message,
        column_of,
        head_words,
        live,
        vec![items(), footer()],
    )
}

/// The body of an item numbered `number`, at `place` (scope, message, key) when it is keyed:
/// made again when anything about it but the words being written changes.
fn body(
    app: AppState,
    item: Memo<MessageItem>,
    words: Memo<String>,
    number: usize,
    place: Option<(String, String, String)>,
) -> AnyElement {
    dynamic(move || {
        let body = item.get();
        let live = written(&body).is_some().then_some(words);
        let spot = place
            .as_ref()
            .map(|(scope, anchor, key)| Spot { scope, anchor, key });
        untracked(|| item_body(app, &body, number, spot, live))
    })
}

/// The `item-<n>` id of words that keep their view when renumbered: a view over them, of
/// their size, that the pointer passes through.
fn tag(n: usize) -> AnyElement {
    view()
        .test_id(&format!("item-{n}"))
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .right(px(0.0))
        .bottom(px(0.0))
        .pointer_events(PointerEvents::None)
        .into_element()
}

/// The words of an item still being written, which each beat adds to.
fn written(item: &MessageItem) -> Option<&str> {
    match item {
        MessageItem::Prose(words) if words.streaming => Some(&words.markdown),
        MessageItem::Thinking(thought) if thought.streaming && thought.open => Some(&thought.text),
        _ => None,
    }
}

/// `item` without what a beat adds to it: all that its view is made from.
fn settled(item: &MessageItem) -> MessageItem {
    match item {
        MessageItem::Prose(words) if words.streaming => MessageItem::Prose(Prose {
            markdown: String::new(),
            more: words.more.clone(),
            streaming: true,
        }),
        MessageItem::Thinking(thought) if thought.streaming && thought.open => {
            MessageItem::Thinking(Thinking {
                text: String::new(),
                // Read only while folded: it counts the lines written so far.
                hidden: String::new(),
                more: thought.more.clone(),
                action: thought.action.clone(),
                ..*thought
            })
        }
        other => other.clone(),
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum ItemKey {
    Recorded(String),
    Fallback(usize, std::mem::Discriminant<MessageItem>, Option<String>),
}

#[derive(Clone, PartialEq, Eq)]
struct ItemRow {
    key: (ItemKey, usize),
    number: usize,
    body: MessageItem,
}

fn item_list(
    app: AppState,
    shown: Memo<Option<(Message, usize)>>,
    scope: String,
    anchor: String,
    column: Option<String>,
) -> AnyElement {
    let listing = computed(move || {
        shown.with(|row| {
            let Some((message, first)) = row else {
                return Vec::new();
            };
            let keyed = message.keys.len() == message.items.len();
            let mut counts = HashMap::new();
            message
                .items
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    let key = if keyed {
                        ItemKey::Recorded(message.keys[i].clone())
                    } else {
                        let action = match item {
                            MessageItem::Tool(tool) => tool.action.as_ref(),
                            MessageItem::Cell(cell) => cell.action.as_ref(),
                            _ => None,
                        };
                        let call = match action {
                            Some(Action::Expand { key, .. }) if key.starts_with("call:") => {
                                Some(key.clone())
                            }
                            _ => None,
                        };
                        ItemKey::Fallback(i, std::mem::discriminant(item), call)
                    };
                    let count = counts.entry(key.clone()).or_insert(0);
                    let row = ItemRow {
                        key: (key, *count),
                        number: first + i,
                        body: item.clone(),
                    };
                    *count += 1;
                    row
                })
                .collect()
        })
    });
    each(
        listing,
        |row| row.key.clone(),
        move |row, _| {
            let key = row.with_untracked(|row| match &row.key.0 {
                ItemKey::Recorded(key) => Some(key.clone()),
                ItemKey::Fallback(..) => None,
            });
            let number = computed(move || row.with(|row| row.number));
            // A beat of a reply being written changes only its words. They are drawn in a
            // view that stays; the item is made again only when something else about it
            // changes, where making it on every beat tore down and rebuilt all its blocks.
            let item = computed(move || row.with(|row| settled(&row.body)));
            let words =
                computed(move || row.with(|row| written(&row.body).unwrap_or_default().to_owned()));
            let stays = computed(move || {
                row.with(|row| matches!(&row.body, MessageItem::Prose(words) if words.streaming))
            });
            let place = key.clone().map(|key| (scope.clone(), anchor.clone(), key));
            let content = dynamic(move || {
                let place = place.clone();
                if stays.get() {
                    // Words being written keep their view whatever number the rows above give
                    // them: a call before them that is recorded, or folds into a run,
                    // renumbers them mid-reply. A view over them carries the number instead.
                    return view()
                        .relative()
                        .min_w(px(0.0))
                        .child(body(app, item, words, number.get_untracked(), place))
                        .child(dynamic(move || tag(number.get())))
                        .into_element();
                }
                // Numbered test IDs are static in NSH; renumber only this inner region on prepend.
                let number = number.get();
                numbered(number, body(app, item, words, number, place))
            });
            match key {
                Some(key) => placed(&scope, &anchor, &key, column.as_deref(), content),
                None => content,
            }
        },
    )
}
