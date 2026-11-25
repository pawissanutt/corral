use super::*;
use corral_protocol::rows::*;

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
impl Keyed for corral_types::SystemRow {
    fn key(&self) -> String {
        self.project.clone()
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
        let mut parts = BTreeSet::new();
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
            SubDelta::System { delta } => {
                feed.update(|feed| {
                    changed = apply(&mut feed.systems, delta.rows, delta.left, replacement)
                });
                Topic::System
            }
            SubDelta::Agents { delta } => {
                feed.update(|feed| {
                    changed = apply(&mut feed.agents, delta.rows, delta.left, replacement)
                });
                Topic::Agents
            }
            SubDelta::Assistant { mut delta } => {
                feed.update(|feed| {
                    // A snapshot prepared before the End receipt cannot reopen that thread.
                    for row in &mut delta.rows {
                        if row.state != AssistantState::Ended
                            && let Some(ended) =
                                feed.assistants.get(row.id.as_str()).filter(|old| {
                                    old.kind != AssistantKind::Build
                                        && old.state == AssistantState::Ended
                                })
                        {
                            *row = ended.clone();
                        }
                    }
                    changed = apply(&mut feed.assistants, delta.rows, delta.left, replacement);
                    feed.forget_ended_quick();
                });
                self.retire_quick_readers(feed);
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
            SubDelta::Session {
                scope,
                generation,
                events,
                gap,
                fleet,
                state,
                ..
            } => {
                if !self.session_scopes.contains_key(&scope) {
                    return Ok(vec![]);
                }
                scope_key = Some(scope.clone());
                // Without the capability nothing is said about children or the host's state,
                // whatever the page holds.
                let fleet =
                    fleet.filter(|_| feed.with_untracked(|feed| feed.capable(SESSION_FLEET_V1)));
                let state =
                    state.filter(|_| feed.with_untracked(|feed| feed.capable(SESSION_STATE_V1)));
                let reset = gap.is_some();
                feed.update(|feed| {
                    let live = feed.live.entry(Id::from(&scope)).or_default();
                    let before = live.parts.clone();
                    let generation_changed = live.generation != generation;
                    if scope == QUICK_THREAD && generation.is_none() && !reset {
                        // Empty quick scope, not an unchanged state on an existing host.
                        live.clear_host();
                    } else {
                        live.apply_page(generation, events, reset);
                        live.replace_fleet(fleet);
                        live.replace_state(state);
                    }
                    // After a gap or a new generation no part event can be trusted to have
                    // arrived, so every part is read again.
                    parts = if reset || generation_changed {
                        super::details::PART_NAMES
                            .iter()
                            .map(|part| (*part).to_owned())
                            .collect()
                    } else {
                        live.parts
                            .iter()
                            .filter(|(part, count)| before.get(*part) != Some(*count))
                            .map(|(part, _)| part.clone())
                            .collect()
                    };
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
        let mut outgoing = self.invalidate(feed, topic, &changed);
        if let Some(scope) = scope_key.as_deref().filter(|_| !parts.is_empty()) {
            outgoing.extend(self.session_parts(feed, scope, &parts));
        }
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
}

fn affected(target: &DetailRef, topic: Topic, changed: &BTreeSet<String>) -> bool {
    match target {
        DetailRef::Closed { .. } | DetailRef::Artifacts { .. } => topic == Topic::Tasks,
        DetailRef::Task { id } => topic == Topic::Tasks && changed.contains(id),
        DetailRef::Plan { id } => {
            matches!(topic, Topic::Plans | Topic::Tasks) && changed.contains(id)
        }
        DetailRef::Review { task } | DetailRef::ReviewFile { task, .. } => {
            topic == Topic::Tasks && changed.contains(task)
        }
        DetailRef::Session { id } | DetailRef::Terminal { id } => {
            matches!(topic, Topic::Agents | Topic::Assistant) && changed.contains(id)
        }
        DetailRef::Timeline { .. } | DetailRef::Burnup { .. } => {
            matches!(topic, Topic::Tasks | Topic::Plans | Topic::Agents)
        }
        DetailRef::UsageSeries { .. } => matches!(topic, Topic::Usage | Topic::Settings),
        // A project's row moves its revision whenever any of its map's details changes.
        DetailRef::SystemMap { project, .. }
        | DetailRef::SystemRuntime { project }
        | DetailRef::SystemData { project, .. }
        | DetailRef::SystemLenses { project }
        | DetailRef::SystemLens { project, .. } => {
            topic == Topic::System && changed.contains(project)
        }
        // The impact's facts land on the project's row, which this target does not name.
        DetailRef::SystemReview { task, .. } => {
            topic == Topic::System || (topic == Topic::Tasks && changed.contains(task))
        }
        _ => false,
    }
}

pub(super) fn topic_affects(target: &DetailRef, topic: Topic) -> bool {
    match target {
        DetailRef::Closed { .. } | DetailRef::Artifacts { .. } => topic == Topic::Tasks,
        DetailRef::Task { .. } | DetailRef::Review { .. } | DetailRef::ReviewFile { .. } => {
            topic == Topic::Tasks
        }
        DetailRef::Plan { .. } => matches!(topic, Topic::Plans | Topic::Tasks),
        DetailRef::Session { .. } | DetailRef::Terminal { .. } => {
            matches!(topic, Topic::Agents | Topic::Assistant)
        }
        DetailRef::Timeline { .. } | DetailRef::Burnup { .. } => {
            matches!(topic, Topic::Tasks | Topic::Plans | Topic::Agents)
        }
        DetailRef::UsageSeries { .. } => matches!(topic, Topic::Usage | Topic::Settings),
        DetailRef::SystemMap { .. }
        | DetailRef::SystemRuntime { .. }
        | DetailRef::SystemData { .. }
        | DetailRef::SystemLenses { .. }
        | DetailRef::SystemLens { .. } => topic == Topic::System,
        DetailRef::SystemReview { .. } => matches!(topic, Topic::System | Topic::Tasks),
        _ => false,
    }
}

#[cfg(test)]
mod usage_invalidation_tests {
    use super::*;

    #[test]
    fn settings_changes_invalidate_grouped_usage_even_when_totals_match() {
        let target = DetailRef::UsageSeries {
            from: "2026-01-01".into(),
            to: "2026-01-02".into(),
            by: Some(corral_types::UsageBy::Model),
            utc_offset_minutes: 0,
            time_zone: None,
        };
        assert!(topic_affects(&target, Topic::Settings));
        assert!(affected(&target, Topic::Settings, &BTreeSet::new()));
    }

    /// A Runtime view first read while its project's facts were building kept that answer:
    /// the project's System row moving (building to ready, a new head) must read it again, and
    /// so must a stale copy after a reconnect.
    #[test]
    fn a_project_system_update_rereads_its_runtime() {
        let target = DetailRef::SystemRuntime {
            project: "corral".into(),
        };
        let changed = BTreeSet::from(["corral".to_owned()]);
        assert!(affected(&target, Topic::System, &changed));
        assert!(!affected(
            &target,
            Topic::System,
            &BTreeSet::from(["site".to_owned()])
        ));
        assert!(topic_affects(&target, Topic::System));
    }
}

#[cfg(test)]
#[path = "fleet_tests.rs"]
mod fleet_tests;
