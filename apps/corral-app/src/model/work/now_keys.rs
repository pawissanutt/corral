//! Now's list keys: the walk over its rows in display order, then the task handler's keys.
use super::{NowRow, Press, State};
use crate::model::daemon::Feed;
use crate::model::effect::{Effect, Env};
use crate::model::projects::{NowGroup, NowScope, ProjectFilter};
use crate::model::{Form, Id, Live, Selection};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NowWalk {
    Ask(Id),
    Task(Id),
}

/// Main sections in their rendered order, then open Waiting rows and the expanded Closed page.
pub fn now_walk_order(
    state: &State,
    feed: &Feed,
    form: Form,
    filter: &ProjectFilter,
    grouping: NowGroup,
) -> Vec<NowWalk> {
    let filter = filter.resolved(feed);
    let list =
        super::now_page::now_list(state, feed, &Selection::default(), form, &filter, grouping);
    let mut order: Vec<_> = list
        .sections
        .into_iter()
        .flat_map(|section| section.rows)
        .map(|row| match row {
            NowRow::Ask(row) => NowWalk::Ask(row.key),
            NowRow::Task(row) => NowWalk::Task(row.line.id.into()),
        })
        .collect();
    if state.waiting_open {
        order.extend(
            list.waiting_groups
                .into_iter()
                .flat_map(|group| group.rows)
                .map(|row| NowWalk::Task(row.line.id.into())),
        );
    }
    order.extend(
        super::closed_ids_filtered(&state.closed, feed, &filter)
            .into_iter()
            .map(NowWalk::Task),
    );
    order
}

impl State {
    /// Now's keys: j/k and the arrows walk [`now_walk_order`], Escape returns the side panel to
    /// the digest, Enter on an owner move opens its record; every other key is the task
    /// handler's, with its canonical routing.
    pub fn key_now_page(
        &mut self,
        key: &str,
        top: Option<&Live>,
        selected: &Selection,
        feed: &Feed,
        env: Env,
        scope: NowScope<'_>,
    ) -> Option<Vec<Effect>> {
        if top.is_some_and(|live| !matches!(live, Live::Task(_))) {
            return None;
        }
        match key {
            "escape" | "Escape" | "esc" => Some(self.press(Press::ClearSelection, feed, env)),
            "j" | "down" | "ArrowDown" | "k" | "up" | "ArrowUp" => {
                let order = now_walk_order(self, feed, env.form, scope.filter, scope.grouping);
                let current = self.selected_ask.clone().map(NowWalk::Ask).or_else(|| {
                    (!self.digest_only)
                        .then(|| selected.task.clone().map(NowWalk::Task))
                        .flatten()
                });
                let at = current
                    .as_ref()
                    .and_then(|current| order.iter().position(|row| row == current));
                let next = match at {
                    Some(index) if matches!(key, "j" | "down" | "ArrowDown") => {
                        order.get(index + 1).or(order.get(index))
                    }
                    Some(index) => order.get(index.saturating_sub(1)),
                    None => order.first(),
                };
                Some(match next {
                    Some(NowWalk::Ask(key)) => self.press(Press::SelectAsk(key.clone()), feed, env),
                    Some(NowWalk::Task(id)) => self.press(Press::Select(id.clone()), feed, env),
                    None => Vec::new(),
                })
            }
            "enter" | "Enter" if self.selected_ask.is_some() => {
                let key = self.selected_ask.as_ref()?;
                let filter = scope.filter.resolved(feed);
                let row = feed.inbox.get(key).filter(|row| {
                    crate::model::inbox::is_open(row) && filter.matches(row.project.as_deref())
                })?;
                Some(
                    crate::model::inbox::target(row)
                        .map(Effect::OpenLive)
                        .into_iter()
                        .collect(),
                )
            }
            "." | "enter" | "Enter" if self.selected_ask.is_some() || self.digest_only => None,
            _ => self.key_live_filtered(key, top, selected, feed, env, scope),
        }
    }
}
