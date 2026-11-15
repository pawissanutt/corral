//! Owned identifiers of daemon records: cheap to clone, freed with the last holder, never leaked.

use std::borrow::Borrow;
use std::fmt::{self, Write};
use std::ops::Deref;
use std::sync::Arc;

/// Deterministic intent identities supplied to reducers as state, never read from the host.
///
/// The app seeds each domain with its own namespace from one runtime-unique launch identity.
/// Cloning a supply copies its position for pure replay; independently running domains must
/// not share a namespace. Defaults are for isolated model construction, not live dispatch.
/// Keep live namespaces compact and protocol-safe; command IDs have a 64-byte wire limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdSupply {
    namespace: Id,
    next: u64,
    high: Vec<u64>,
}

/// Deterministic isolated snapshots/tests only; runtime owners must inject a unique namespace.
impl Default for IdSupply {
    fn default() -> Self {
        Self::new("local")
    }
}

impl IdSupply {
    pub fn new(namespace: impl Into<Id>) -> Self {
        Self {
            namespace: namespace.into(),
            next: 0,
            high: Vec::new(),
        }
    }

    /// Allocate once for a new intent; retry and recovery retain the returned identity.
    pub fn next_id(&mut self) -> Id {
        let mut text = String::with_capacity(self.namespace.len() + 1 + (self.high.len() + 1) * 16);
        text.push_str(self.namespace.as_str());
        text.push('-');
        if let Some((first, rest)) = self.high.split_last() {
            // String's formatter cannot fail.
            let _ = write!(text, "{first:x}");
            for limb in rest.iter().rev() {
                let _ = write!(text, "{limb:016x}");
            }
        }
        let _ = write!(text, "{:016x}", self.next);
        let (next, carry) = self.next.overflowing_add(1);
        self.next = next;
        if carry {
            let mut extended = true;
            for limb in &mut self.high {
                let (next, carry) = limb.overflowing_add(1);
                *limb = next;
                if !carry {
                    extended = false;
                    break;
                }
            }
            if extended {
                self.high.push(1);
            }
        }
        Id::from(text)
    }
}

/// A task, plan, question key, session, thread or command id received from (or sent to) corrald.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Id(Arc<str>);

impl Id {
    pub fn new(text: &str) -> Id {
        Id(Arc::from(text))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for Id {
    fn from(text: &str) -> Id {
        Id::new(text)
    }
}

impl From<String> for Id {
    fn from(text: String) -> Id {
        Id(Arc::from(text))
    }
}

impl From<&String> for Id {
    fn from(text: &String) -> Id {
        Id::new(text)
    }
}

impl Deref for Id {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Id {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for Id {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.0, f)
    }
}

impl PartialEq<str> for Id {
    fn eq(&self, other: &str) -> bool {
        &*self.0 == other
    }
}

impl PartialEq<&str> for Id {
    fn eq(&self, other: &&str) -> bool {
        &*self.0 == *other
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn maps_look_up_by_borrowed_text_and_ids_compare_with_text() {
        let mut rows = BTreeMap::new();
        rows.insert(Id::from("T-9"), 1);
        assert_eq!(rows.get("T-9"), Some(&1));
        assert_eq!(Id::from(String::from("T-9")), "T-9");
        assert_eq!(format!("{:?}", Id::new("T-9")), "\"T-9\"");
    }

    #[test]
    fn injected_identity_state_replays_without_collisions_at_counter_carry() {
        let mut ids = IdSupply::new("0123456789abcdef0123456789abcdef-a");
        let mut replay = ids.clone();
        let first = ids.next_id();
        assert_eq!(replay.next_id(), first);
        assert_eq!(replay.next_id(), ids.next_id());
        assert_ne!(
            IdSupply::new("0123456789abcdef0123456789abcdef-b").next_id(),
            first
        );

        ids.next = u64::MAX;
        let before_carry = ids.next_id();
        let after_carry = ids.next_id();
        assert_ne!(before_carry, after_carry);
        assert_ne!(first, after_carry);
        assert!(after_carry.len() <= 64);
        assert_eq!(ids.high, vec![1]);
        assert_eq!(ids.next, 1);
    }
}
