//! Skill metadata uses Pi's `yaml` package's default core scalar schema.
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, LazyLock};

use regex::RegexSet;
use saphyr_parser::{Event, Parser, ScalarStyle, Tag};

// Match the pinned JS yaml core schema by spelling, not by parsing numbers:
// overflowing integers are not strings, but binary integers and signed hex are.
static NON_STRINGS: LazyLock<Result<RegexSet, regex::Error>> = LazyLock::new(|| {
    RegexSet::new([
        r"\A(?:~|[Nn]ull|NULL)?\z",
        r"\A(?:[Tt]rue|TRUE|[Ff]alse|FALSE)\z",
        r"\A(?:0o[0-7]+|[-+]?[0-9]+|0x[0-9a-fA-F]+)\z",
        r"\A(?:[-+]?\.(?:inf|Inf|INF)|\.(?:nan|NaN|NAN)|[-+]?(?:\.[0-9]+|[0-9]+(?:\.[0-9]*)?)[eE][-+]?[0-9]+|[-+]?(?:\.[0-9]+|[0-9]+\.[0-9]*))\z",
    ])
});

fn core_tag(tag: &Tag) -> Option<&str> {
    if tag.is_yaml_core_schema() {
        Some(&tag.suffix)
    } else if tag.handle.is_empty() {
        tag.suffix.strip_prefix("tag:yaml.org,2002:")
    } else {
        None
    }
}

fn is_string(text: &str, style: ScalarStyle, tag: Option<&Tag>) -> Option<bool> {
    let patterns = NON_STRINGS.as_ref().ok()?;
    Some(match tag.and_then(core_tag) {
        Some(tag) => {
            let index = match tag {
                "null" => 0,
                "bool" => 1,
                "int" => 2,
                "float" => 3,
                // Binary is an explicit scalar extension in Pi, not a string.
                "binary" => return Some(false),
                // Timestamp validation is outside core. Fail closed rather
                // than treating malformed explicitly tagged dates as strings.
                "timestamp" => return None,
                _ => return Some(true),
            };
            !patterns.matches(text).matched(index)
        }
        // Explicit string, non-specific and unrecognized tags bypass implicit
        // resolution, as do quoted and block scalars without explicit tags.
        None => tag.is_some() || style != ScalarStyle::Plain || !patterns.is_match(text),
    })
}

fn blank_description(s: &str) -> bool {
    // ECMAScript trim (unlike Rust, includes BOM and excludes NEL).
    s.chars().all(|c| match c {
        '\u{feff}' => true,
        '\u{85}' => false,
        _ => c.is_whitespace(),
    })
}

pub(super) fn skill_name(text: &str, path: &Path) -> Option<String> {
    let mut lines = text.trim_start_matches('\u{feff}').split_inclusive('\n');
    if lines.next()?.trim_end_matches(['\n', '\r']) != "---" {
        return None;
    }
    let mut yaml = String::new();
    for line in lines {
        if yaml.len() + line.len() + 5 > super::HEADER_LIMIT {
            return None;
        }
        if line.trim_end_matches(['\n', '\r']) == "---" {
            let (name, description) = fields(&yaml)?;
            if blank_description(&description?) {
                return None;
            }
            return name
                .filter(|name| !name.is_empty())
                .map(|name| name.to_string())
                .or_else(|| Some(path.parent()?.file_name()?.to_str()?.to_owned()));
        }
        yaml.push_str(line);
    }
    None
}

type Scalar = Option<Arc<str>>;

fn fields(yaml: &str) -> Option<(Scalar, Scalar)> {
    let mut events = Parser::new_from_str(yaml).keep_tags(true);
    let mut next = || events.next()?.ok().map(|(event, _)| event);
    if !matches!(next()?, Event::StreamStart)
        || !matches!(next()?, Event::DocumentStart(_))
        || !matches!(next()?, Event::MappingStart(_, _))
    {
        return None;
    }
    let mut name = None;
    let mut description = None;
    let mut keys = HashSet::new();
    let mut anchors = HashMap::new();
    loop {
        let event = next()?;
        if matches!(event, Event::MappingEnd) {
            break;
        }
        let key = scalar_node(event, &mut next, &mut anchors)?;
        let value = scalar_node(next()?, &mut next, &mut anchors)?;
        if let Some(key) = key {
            match key.as_ref() {
                "name" => name = value,
                "description" => description = value,
                _ => {}
            }
            if !keys.insert(key) {
                return None;
            }
        }
    }
    if !matches!(next()?, Event::DocumentEnd) || !matches!(next()?, Event::StreamEnd) {
        return None;
    }
    Some((name, description))
}

fn scalar_node<'a>(
    mut event: Event<'a>,
    next: &mut impl FnMut() -> Option<Event<'a>>,
    anchors: &mut HashMap<usize, Arc<str>>,
) -> Option<Scalar> {
    // Drain collections without constructing a tree or expanding collection
    // aliases. Scalar aliases share the original string allocation.
    let mut depth = 0usize;
    loop {
        match event {
            Event::Scalar(text, style, anchor, tag) => {
                let value = ((depth == 0 || anchor != 0)
                    && is_string(&text, style, tag.as_deref())?)
                .then(|| Arc::<str>::from(text.as_ref()));
                if let Some(text) = value.as_ref().filter(|_| anchor != 0) {
                    anchors.insert(anchor, Arc::clone(text));
                }
                if depth == 0 {
                    return Some(value);
                }
            }
            Event::Alias(anchor) => {
                if depth == 0 {
                    return Some(anchors.get(&anchor).cloned());
                }
            }
            Event::MappingStart(_, _) | Event::SequenceStart(_, _) => {
                depth += 1;
                if depth > 64 {
                    return None;
                }
            }
            Event::MappingEnd | Event::SequenceEnd if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    return Some(None);
                }
            }
            _ => return None,
        }
        event = next()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_pi_core_strings_in_both_fields() {
        let path = Path::new("fallback/SKILL.md");
        for (yaml, resolved) in [
            ("0b101", Some("0b101")),
            ("-0x10", Some("-0x10")),
            ("1_000", Some("1_000")),
            ("2025-01-01", Some("2025-01-01")),
            ("'FALSE'", Some("FALSE")),
            ("!!str 42", Some("42")),
            ("!!int '42'", None),
            ("!<tag:yaml.org,2002:int> 42", None),
            ("!!int 0b101", Some("0b101")),
            ("0o17", None),
            ("0xBEEF", None),
            ("99999999999999999999999999", None),
            ("+.5e2", None),
            ("-.INF", None),
            ("true", None),
            ("Null", None),
        ] {
            let text = format!("---\n'name': {yaml}\ndescription: usage\n---\nignored");
            assert_eq!(
                skill_name(&text, path).as_deref(),
                resolved.or(Some("fallback")),
                "{yaml}"
            );
            let text = format!("---\nname: skill\ndescription: {yaml}\n---\n");
            assert_eq!(
                skill_name(&text, path).as_deref(),
                resolved.map(|_| "skill"),
                "{yaml}"
            );
        }
    }

    #[test]
    fn keeps_scalar_aliases_and_drains_other_metadata() {
        let path = Path::new("fallback/SKILL.md");
        let text = "---\nmetadata: { nested: [&label 0b101] }\nname: *label\ndescription: >\n  documentation\n---\n";
        assert_eq!(skill_name(text, path).as_deref(), Some("0b101"));
        let duplicate = "---\nname: first\n'name': second\ndescription: docs\n---\n";
        assert_eq!(skill_name(duplicate, path), None);
    }
}
