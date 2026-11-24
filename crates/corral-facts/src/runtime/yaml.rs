//! The literal subset of YAML needed by the runtime readers. Keep source marks with
//! nodes instead of searching the text for matching keys (which can repeat or be quoted).
use std::borrow::Cow;
use std::collections::BTreeSet;

use saphyr_parser::{Event, Parser, ScalarStyle, ScanError, Span};

type MarkedEvent<'a> = Result<(Event<'a>, Span), ScanError>;

// Repository text has a byte limit at the Git reader. Also bound the number of
// allocations and stack depth here; aliases are never dereferenced.
const MAX_EVENTS: usize = 100_000;
const MAX_DEPTH: usize = 64;

pub(super) struct Node<'a> {
    pub(super) line: u64,
    value: Value<'a>,
}

enum Value<'a> {
    Scalar(Cow<'a, str>),
    Sequence(Vec<Node<'a>>),
    Mapping(Vec<(Node<'a>, Node<'a>)>),
    Unsupported,
}

impl Node<'_> {
    pub(super) fn scalar(&self) -> Option<&str> {
        match &self.value {
            Value::Scalar(text) => Some(text),
            _ => None,
        }
    }

    pub(super) fn sequence(&self) -> Option<&[Self]> {
        match &self.value {
            Value::Sequence(values) => Some(values),
            _ => None,
        }
    }

    pub(super) fn is_mapping(&self) -> bool {
        matches!(&self.value, Value::Mapping(_))
    }

    pub(super) fn fields(&self) -> impl Iterator<Item = (&str, u64, &Self)> {
        let pairs = match &self.value {
            Value::Mapping(pairs) => pairs.as_slice(),
            _ => &[],
        };
        pairs.iter().filter_map(|(key, value)| {
            let name = key.scalar()?;
            // A merge key is not itself a declaration. Never follow it.
            (!name.is_empty() && name != "<<").then_some((name, key.line, value))
        })
    }

    pub(super) fn field(&self, name: &str) -> Option<(u64, &Self)> {
        self.fields()
            .find_map(|(key, line, value)| (key == name).then_some((line, value)))
    }

    pub(super) fn has_merge(&self) -> bool {
        matches!(&self.value, Value::Mapping(pairs) if pairs.iter().any(|(key, _)| key.scalar() == Some("<<")))
    }
}

pub(super) fn parse(text: &str) -> Option<Node<'_>> {
    let mut events = Parser::new_from_str(text).take(MAX_EVENTS);
    if !matches!(events.next()?.ok()?.0, Event::StreamStart)
        || !matches!(events.next()?.ok()?.0, Event::DocumentStart(_))
    {
        return None;
    }
    let root = node(&mut events, 0)?;
    // Compose files and workflows are single documents. Do not expose partial
    // facts if parsing fails or a budget is exceeded.
    if !matches!(events.next()?.ok()?.0, Event::DocumentEnd)
        || !matches!(events.next()?.ok()?.0, Event::StreamEnd)
    {
        return None;
    }
    Some(root)
}

fn node<'a>(events: &mut impl Iterator<Item = MarkedEvent<'a>>, depth: usize) -> Option<Node<'a>> {
    let (event, span) = events.next()?.ok()?;
    from_event(events, event, span, depth)
}

fn from_event<'a>(
    events: &mut impl Iterator<Item = MarkedEvent<'a>>,
    event: Event<'a>,
    span: Span,
    depth: usize,
) -> Option<Node<'a>> {
    if depth > MAX_DEPTH {
        return None;
    }
    let value = match event {
        Event::Scalar(text, style, _, tag) => {
            // No type coercion or expansion. Retain scalar spelling (including
            // numeric port values), except that YAML null is not a string.
            let core_tag = tag.as_ref().is_some_and(|tag| tag.is_yaml_core_schema());
            let is_null = if let Some(tag) = &tag {
                tag.suffix == "null"
            } else {
                style == ScalarStyle::Plain
                    && matches!(text.as_ref(), "" | "~" | "null" | "Null" | "NULL")
            };
            if (tag.is_none() || core_tag) && !is_null {
                Value::Scalar(text)
            } else {
                Value::Unsupported
            }
        }
        Event::Alias(_) => Value::Unsupported,
        Event::SequenceStart(_, tag) => {
            let mut values = Vec::new();
            loop {
                let (event, span) = events.next()?.ok()?;
                if matches!(event, Event::SequenceEnd) {
                    break;
                }
                values.push(from_event(events, event, span, depth + 1)?);
            }
            if tag.is_some_and(|tag| !tag.is_yaml_core_schema()) {
                Value::Unsupported
            } else {
                Value::Sequence(values)
            }
        }
        Event::MappingStart(_, tag) => {
            let mut pairs = Vec::new();
            loop {
                let (event, span) = events.next()?.ok()?;
                if matches!(event, Event::MappingEnd) {
                    break;
                }
                pairs.push((
                    from_event(events, event, span, depth + 1)?,
                    node(events, depth + 1)?,
                ));
            }
            // Duplicate literal keys are ambiguous; never emit two declarations
            // for the same unit ID or arbitrarily choose a command/build spec.
            let mut keys = BTreeSet::new();
            if pairs
                .iter()
                .filter_map(|(key, _)| key.scalar())
                .any(|key| !keys.insert(key))
            {
                return None;
            }
            if tag.is_some_and(|tag| !tag.is_yaml_core_schema()) {
                Value::Unsupported
            } else {
                Value::Mapping(pairs)
            }
        }
        _ => return None,
    };
    Some(Node {
        line: span.start.line() as u64,
        value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_keys_keep_their_marks_and_aliases_are_not_followed() {
        let root = parse("definitions: &base echo\njobs: {\n  'run': {run: *base},\n  worker: {run: \"worker --test\"}\n}\n").unwrap();
        let (_, jobs) = root.field("jobs").unwrap();
        let (line, job) = jobs.field("worker").unwrap();
        assert_eq!(line, 4);
        assert_eq!(job.field("run").unwrap().1.scalar(), Some("worker --test"));
        assert_eq!(
            jobs.field("run")
                .unwrap()
                .1
                .field("run")
                .unwrap()
                .1
                .scalar(),
            None
        );
    }

    #[test]
    fn duplicate_literal_yaml_keys_are_rejected() {
        assert!(parse("services: {api: {}, 'api': {}}\n").is_none());
    }
}
