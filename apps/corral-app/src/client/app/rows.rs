use super::*;
use corral_protocol::{assistant::ThreadEntry, rows::*};

/// A topic row's identity, as `RowsDelta.left` names it.
trait Keyed {
    fn key(&self) -> String;
}
impl Keyed for PlanRow {
    fn key(&self) -> String {
        self.parent.id.clone()
    }
}
impl Keyed for ProjectRow {
    fn key(&self) -> String {
        self.id.clone()
    }
}
impl Keyed for AgentRow {
    fn key(&self) -> String {
        format!("{}_{}", self.run.task, self.run.id)
    }
}
impl Keyed for AssistantRow {
    fn key(&self) -> String {
        self.id.clone()
    }
}
impl Keyed for ThreadEntry {
    fn key(&self) -> String {
        self.id.clone()
    }
}
fn apply<T: Keyed + PartialEq>(
    map: &mut BTreeMap<Id, T>,
    rows: Vec<T>,
    left: Vec<String>,
    replacement: bool,
) -> BTreeSet<String> {
    let mut changed = BTreeSet::new();
    let keyed: Vec<(String, T)> = rows.into_iter().map(|row| (row.key(), row)).collect();
    if replacement {
        let next: BTreeSet<_> = keyed.iter().map(|(key, _)| key.as_str()).collect();
        changed.extend(
            map.keys()
                .filter(|key| !next.contains(key.as_str()))
                .map(ToString::to_string),
        );
        map.clear();
    }
    for (key, row) in keyed {
        if map.get(key.as_str()) != Some(&row) {
            changed.insert(key.clone());
        }
        map.insert(Id::from(key), row);
    }
    for id in left {
        if map.remove(id.as_str()).is_some() {
            changed.insert(id);
        }
    }
    changed
}
impl AppState {
    pub(super) fn rows(
        &mut self,
        feed: Signal<Feed>,
        delta: SubDelta,
        replacement: bool,
    ) -> Result<Vec<Outgoing>, String> {
        let mut changed = BTreeSet::new();
        let mut scope_key = None;
        let topic = match delta {
            SubDelta::Tasks { delta } => {
                return Ok(self.tasks(feed, delta.epoch, delta.rows, delta.left, replacement));
            }
            SubDelta::Plans { delta } => {
                feed.update(|feed| {
                    changed = apply(&mut feed.plans, delta.rows, delta.left, replacement)
                });
                Topic::Plans
            }
            SubDelta::Projects { delta } => {
                feed.update(|feed| {
                    changed = apply(&mut feed.projects, delta.rows, delta.left, replacement)
                });
                Topic::Projects
            }
            SubDelta::Agents { delta } => {
                feed.update(|feed| {
                    changed = apply(&mut feed.agents, delta.rows, delta.left, replacement)
                });
                Topic::Agents
            }
            SubDelta::Assistant { delta } => {
                feed.update(|feed| {
                    changed = apply(&mut feed.assistants, delta.rows, delta.left, replacement)
                });
                Topic::Assistant
            }
            // Single-row topics: the last row of a complete page is the current snapshot.
            SubDelta::Usage { delta } => {
                feed.update(|feed| {
                    if let Some(row) = delta.rows.into_iter().last() {
                        feed.usage = Some(row);
                    }
                });
                Topic::Usage
            }
            SubDelta::Settings { delta } => {
                feed.update(|feed| {
                    if let Some(row) = delta.rows.into_iter().last() {
                        feed.settings = Some(row);
                    }
                });
                Topic::Settings
            }
            SubDelta::Thread { scope, delta, live } => {
                scope_key = Some(scope.clone());
                feed.update(|feed| {
                    let thread = feed.threads.entry(Id::from(&scope)).or_default();
                    changed = apply(&mut thread.entries, delta.rows, delta.left, replacement);
                    thread.stale = false;
                    thread.loading = false;
                    feed.retire_echoed_sends();
                });
                if let Some(live) = live {
                    Self::live_update(feed, &scope, live)?;
                }
                self.touch_thread(feed, &scope);
                Topic::Thread
            }
            SubDelta::Session {
                scope,
                generation,
                events,
                gap,
                fleet,
                ..
            } => {
                if !self.session_scopes.contains_key(&scope) {
                    return Ok(vec![]);
                }
                scope_key = Some(scope.clone());
                // Without the capability nothing is said about children, whatever the page holds.
                let fleet =
                    fleet.filter(|_| feed.with_untracked(|feed| feed.capable(SESSION_FLEET_V1)));
                feed.update(|feed| {
                    let live = feed.live.entry(Id::from(&scope)).or_default();
                    live.apply_page(generation, events, gap.is_some());
                    live.replace_fleet(fleet);
                });
                self.touch_session(feed, &scope);
                Topic::Session
            }
            SubDelta::Terminal {
                scope,
                snapshot,
                error,
                ..
            } => {
                if self.terminal_scopes.contains_key(&scope) {
                    self.terminal_capture(feed, scope, snapshot, error);
                }
                return Ok(vec![]);
            }
            _ => return Ok(vec![]),
        };
        feed.update(|feed| {
            feed.topics.insert(
                topic_key(topic, scope_key.as_deref()),
                TopicState {
                    loading: false,
                    stale: false,
                    error: None,
                },
            );
            feed.synced = Some(feed.clock.clone());
        });
        let outgoing = self.invalidate(feed, topic, &changed);
        self.trim_retained(feed);
        Ok(outgoing)
    }
    pub(super) fn tasks(
        &mut self,
        feed: Signal<Feed>,
        epoch: u64,
        rows: Vec<TaskRow>,
        left: Vec<String>,
        replacement: bool,
    ) -> Vec<Outgoing> {
        let mut changed: BTreeSet<String> = left.iter().cloned().collect();
        feed.with_untracked(|feed| {
            for row in &rows {
                if feed
                    .board
                    .task(&row.task.id)
                    .is_none_or(|old| old.row != *row)
                {
                    changed.insert(row.task.id.clone());
                }
            }
            if replacement {
                let next: BTreeSet<_> = rows.iter().map(|row| row.task.id.as_str()).collect();
                changed.extend(
                    feed.board
                        .tasks()
                        .iter()
                        .filter(|task| !next.contains(task.id.as_str()))
                        .map(|task| task.id.to_string()),
                );
            }
        });
        feed.update(|feed| {
            feed.board_received(
                epoch,
                rows.into_iter()
                    .map(crate::model::daemon::Task::new)
                    .collect(),
                &left,
                replacement,
            );
            feed.topics
                .insert(topic_key(Topic::Tasks, None), TopicState::default());
        });
        self.invalidate(feed, Topic::Tasks, &changed)
    }
    fn invalidate(
        &mut self,
        feed: Signal<Feed>,
        topic: Topic,
        changed: &BTreeSet<String>,
    ) -> Vec<Outgoing> {
        let targets = feed.with_untracked(|feed| {
            feed.details
                .values()
                .filter(|cache| {
                    !matches!(&cache.target, DetailRef::Terminal { id }
                        if self.terminal_scopes.contains_key(id)
                            && feed.capable(STREAM_TERMINAL_V1))
                        && (affected(&cache.target, topic, changed)
                            || (cache.stale
                                && !cache.loading
                                && topic_affects(&cache.target, topic)))
                })
                .map(|cache| cache.target.clone())
                .collect::<Vec<_>>()
        });
        let mut outgoing = vec![];
        for target in targets {
            outgoing.extend(self.detail_request(
                feed,
                requests::new_id(),
                self.retained.selected(target),
            ));
        }
        outgoing
    }
    pub(super) fn live(
        &mut self,
        feed: Signal<Feed>,
        scope: String,
        fragment: LiveText,
    ) -> Result<(), String> {
        let result = Self::live_update(feed, &scope, fragment);
        self.touch_thread(feed, &scope);
        self.trim_retained(feed);
        result
    }

    fn live_update(feed: Signal<Feed>, scope: &str, fragment: LiveText) -> Result<(), String> {
        let mut problem = None;
        feed.update(|feed| {
            let thread = feed.threads.entry(Id::new(scope)).or_default();
            let replace = thread
                .live
                .as_ref()
                .is_none_or(|live| live.entry != fragment.entry || live.turn != fragment.turn);
            if replace {
                if fragment.offset != 0 {
                    problem = Some("assistant stream starts at a nonzero offset".to_owned());
                    return;
                }
                thread.live = Some(LiveText {
                    text: String::new(),
                    ..fragment.clone()
                });
            }
            let Some(live) = &mut thread.live else {
                return;
            };
            let Ok(offset) = usize::try_from(fragment.offset) else {
                problem = Some("assistant stream offset overflow".into());
                return;
            };
            if offset == live.text.len() {
                live.text.push_str(&fragment.text);
            } else if live
                .text
                .get(offset..offset.saturating_add(fragment.text.len()))
                != Some(fragment.text.as_str())
            {
                problem = Some("assistant stream has a gap or inconsistent replay".into());
                return;
            }
            if live.text.len() > TURN_TEXT_MAX {
                problem = Some("assistant stream exceeds 256 KiB".into());
                return;
            }
            live.offset = 0;
            live.done = fragment.done;
        });
        problem.map_or(Ok(()), Err)
    }
}

fn affected(target: &DetailRef, topic: Topic, changed: &BTreeSet<String>) -> bool {
    match target {
        DetailRef::Closed { .. } | DetailRef::Artifacts { .. } => topic == Topic::Tasks,
        DetailRef::Attachment { entry, .. } => topic == Topic::Thread && changed.contains(entry),
        DetailRef::Task { id } => topic == Topic::Tasks && changed.contains(id),
        DetailRef::Plan { id } => {
            matches!(topic, Topic::Plans | Topic::Tasks) && changed.contains(id)
        }
        DetailRef::Review { task } | DetailRef::ReviewFile { task, .. } => {
            topic == Topic::Tasks && changed.contains(task)
        }
        DetailRef::Session { id } | DetailRef::Terminal { id } => {
            topic == Topic::Agents && changed.contains(id)
        }
        DetailRef::Timeline { .. } | DetailRef::Burnup { .. } => {
            matches!(topic, Topic::Tasks | Topic::Plans | Topic::Agents)
        }
        DetailRef::UsageSeries { .. } => topic == Topic::Usage,
        _ => false,
    }
}

pub(super) fn topic_affects(target: &DetailRef, topic: Topic) -> bool {
    match target {
        DetailRef::Closed { .. } | DetailRef::Artifacts { .. } => topic == Topic::Tasks,
        DetailRef::Attachment { .. } => topic == Topic::Thread,
        DetailRef::Task { .. } | DetailRef::Review { .. } | DetailRef::ReviewFile { .. } => {
            topic == Topic::Tasks
        }
        DetailRef::Plan { .. } => matches!(topic, Topic::Plans | Topic::Tasks),
        DetailRef::Session { .. } | DetailRef::Terminal { .. } => topic == Topic::Agents,
        DetailRef::Timeline { .. } | DetailRef::Burnup { .. } => {
            matches!(topic, Topic::Tasks | Topic::Plans | Topic::Agents)
        }
        DetailRef::UsageSeries { .. } => topic == Topic::Usage,
        _ => false,
    }
}

#[cfg(test)]
#[path = "fleet_tests.rs"]
mod fleet_tests;
