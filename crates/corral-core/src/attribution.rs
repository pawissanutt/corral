//! Event authorship defaults and scoped overrides for a Store.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result, anyhow};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{Event, Store};

/// Who performed an event, independently of the surface that delivered it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    Owner,
    Pm,
    Agent,
    Assistant,
    Autopilot,
    Reviewer,
    System,
}

#[derive(Debug)]
struct Attribution {
    actor: Actor,
    via: String,
}

/// Shared by Store clones, but never persisted as workspace configuration.
#[derive(Debug, Default)]
pub(crate) struct AttributionContext {
    default: OnceLock<Attribution>,
}

struct Scope {
    context: Arc<AttributionContext>,
    actor: Actor,
}

thread_local! {
    // A worker's own action must not relabel events from the TUI's owner thread.
    static SCOPES: RefCell<Vec<Rc<Scope>>> = const { RefCell::new(Vec::new()) };
}

/// Keeps an actor override active on this thread until dropped.
///
/// Scopes apply to clones of the same Store, preserve its default `via`, and
/// cannot move between threads. The innermost remaining scope wins.
#[must_use = "the attribution override ends when this guard is dropped"]
pub struct AttributionGuard(Rc<Scope>);

impl Drop for AttributionGuard {
    fn drop(&mut self) {
        SCOPES.with(|scopes| {
            scopes
                .borrow_mut()
                .retain(|scope| !Rc::ptr_eq(scope, &self.0));
        });
    }
}

impl Store {
    /// Sets this process's default attribution for this Store and its clones.
    /// Stores opened independently start without attribution.
    ///
    /// # Errors
    /// Returns an error if the default has already been set.
    pub fn set_attribution(&self, actor: Actor, via: &str) -> Result<()> {
        self.attribution
            .default
            .set(Attribution {
                actor,
                via: via.to_owned(),
            })
            .map_err(|_| anyhow!("store attribution default is already set"))
    }

    /// Overrides the actor for operations on this thread, restoring it on drop.
    pub fn attribute(&self, actor: Actor) -> AttributionGuard {
        let scope = Rc::new(Scope {
            context: Arc::clone(&self.attribution),
            actor,
        });
        SCOPES.with(|scopes| scopes.borrow_mut().push(Rc::clone(&scope)));
        AttributionGuard(scope)
    }

    pub(crate) fn attributed_event(
        &self,
        task: Option<&str>,
        kind: &str,
        detail: serde_json::Value,
    ) -> Result<Event> {
        let default = self.attribution.default.get();
        let scoped = SCOPES.with(|scopes| {
            scopes
                .borrow()
                .iter()
                .rev()
                .find(|scope| Arc::ptr_eq(&scope.context, &self.attribution))
                .map(|scope| scope.actor)
        });
        let explicit = detail
            .get("actor")
            .filter(|actor| !actor.is_null())
            .map(|actor| serde_json::from_value::<Actor>(actor.clone()))
            .transpose()
            .context("parsing explicit event actor")?;
        Ok(Event {
            ts: Timestamp::now(),
            task: task.map(str::to_owned),
            kind: kind.to_owned(),
            actor: explicit.or(scoped).or_else(|| default.map(|a| a.actor)),
            via: default.map(|a| a.via.clone()),
            detail,
        })
    }
}
