//! Owned identifiers of daemon records: cheap to clone, freed with the last holder, never leaked.

use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

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
}
