//! Full-facade container projection decoding, shared by the task's real split rows.
use super::super::codec::Rows;
use super::super::head::projection;
use super::super::{AttemptHead, PlanHead, PlanRunHead, RunHead};
use crate::backend::TaskPart;
use anyhow::{Context, Result};
use serde::de::DeserializeOwned;

pub(super) trait WithFacts {
    fn with_facts(&mut self, facts: crate::ContainerFacts);
}

macro_rules! with_facts {
    ($($ty:ty),*) => {
        $(impl WithFacts for $ty {
            fn with_facts(&mut self, facts: crate::ContainerFacts) {
                self.facts = facts;
            }
        })*
    };
}
with_facts!(RunHead, PlanRunHead, PlanHead, AttemptHead);

pub(super) fn decode<T: DeserializeOwned + WithFacts>(
    rows: &Rows,
    part: TaskPart,
    key: &str,
) -> Result<T> {
    let row = rows
        .0
        .get(&(part, key.to_owned()))
        .with_context(|| format!("a split container has no {part:?} row {key}"))?;
    let (mut value, facts): (T, crate::ContainerFacts) = projection(&row.doc)?;
    value.with_facts(facts);
    Ok(value)
}
