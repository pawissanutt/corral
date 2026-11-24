//! References in task text. Project library artifact addresses are not citations.

use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Reference<'a> {
    text: &'a str,
    task: &'a str,
    ordinal: Option<&'a str>,
    section: Option<&'a str>,
}

impl<'a> Reference<'a> {
    /// Parse a single complete reference, not a substring of a larger token.
    pub fn parse(text: &'a str) -> Option<Self> {
        Self::prefix(text).filter(|reference| reference.text == text)
    }

    fn prefix(input: &'a str) -> Option<Self> {
        let digits = input.strip_prefix("T-")?;
        let task_end = 2 + digits.bytes().take_while(u8::is_ascii_digit).count();
        if task_end == 2 {
            return None;
        }
        let mut end = task_end;
        let mut ordinal = None;
        let mut section = None;
        if let Some(tail) = input[end..].strip_prefix(':') {
            let n = tail.bytes().take_while(u8::is_ascii_digit).count();
            if n > 0 {
                ordinal = Some(&tail[..n]);
                end += 1 + n;
                if let Some(slug) = input[end..].strip_prefix('#') {
                    let len = slug
                        .char_indices()
                        .find(|&(_, c)| !section_char(c))
                        .map_or(slug.len(), |(i, _)| i);
                    if len > 0 {
                        section = Some(&slug[..len]);
                        end += 1 + len;
                    }
                }
            }
        }
        if input[end..].chars().next().is_some_and(word_char) {
            return None;
        }
        Some(Self {
            text: &input[..end],
            task: &input[..task_end],
            ordinal,
            section,
        })
    }

    pub fn as_str(self) -> &'a str {
        self.text
    }

    pub fn task(self) -> &'a str {
        self.task
    }

    pub fn artifact(self) -> Option<&'a str> {
        self.ordinal
            .map(|n| &self.text[..self.task.len() + 1 + n.len()])
    }

    pub fn section(self) -> Option<&'a str> {
        self.section
    }
}

/// Collect distinct reference spellings in order of first appearance.
/// Delimiters such as backticks are not part of a reference.
pub fn references_in(text: &str) -> Vec<Reference<'_>> {
    let mut seen = HashSet::new();
    let mut references = Vec::new();
    let mut end = 0;
    for (start, _) in text.match_indices("T-") {
        if start < end || text[..start].chars().next_back().is_some_and(word_char) {
            continue;
        }
        if let Some(reference) = Reference::prefix(&text[start..]) {
            end = start + reference.as_str().len();
            if seen.insert(reference.as_str()) {
                references.push(reference);
            }
        }
    }
    references
}

/// The slug for a rendered Markdown heading. Only literal spaces become hyphens.
pub fn section_slug(heading: &str) -> String {
    heading
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if section_char(c) => Some(c),
            _ => None,
        })
        .collect()
}

fn section_char(c: char) -> bool {
    c.is_alphanumeric() || c == '-'
}

fn word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_complete_distinct_citations_in_text_order() {
        for (text, expected) in [
            ("nothing", &[][..]),
            ("(T-12), T-2; T-12.", &["T-12", "T-2"]),
            (
                "`T-12:3` `T-12:3#4-fact-model` T-12:3",
                &["T-12:3", "T-12:3#4-fact-model"],
            ),
            ("XT-12 éT-12 _T-12 T-12a T-12_ T-12:3x", &[]),
            ("T-12: without a number", &["T-12"]),
            (
                "T-01:2#écriture-T-1, T-01",
                &["T-01:2#écriture-T-1", "T-01"],
            ),
        ] {
            let found: Vec<_> = references_in(text)
                .into_iter()
                .map(Reference::as_str)
                .collect();
            assert_eq!(found, expected, "{text}");
        }
        let reference = Reference::parse("T-12:3#4-fact-model").unwrap();
        assert_eq!(reference.task(), "T-12");
        assert_eq!(reference.artifact(), Some("T-12:3"));
        assert_eq!(reference.section(), Some("4-fact-model"));
        assert!(Reference::parse("T-12:").is_none());
        assert_eq!(section_slug("4. Fact model"), "4-fact-model");
    }
}
