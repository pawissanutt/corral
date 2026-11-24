//! Read column declarations in CREATE TABLE migrations, not arbitrary SQL plans.

use std::collections::BTreeSet;

use super::{Declaration, MAX_FIELDS, MAX_LINKS, MAX_TYPES, Target};
use crate::{DataField, DataKind, DataSource, DataType};

const MAX_BYTES: usize = 4 * 1024 * 1024;

pub(super) fn read(path: &str, text: &str) -> Option<Vec<Declaration>> {
    if !path.ends_with(".sql") || text.len() > MAX_BYTES {
        return None;
    }
    let tokens = lex(text)?;
    let mut tables = Vec::new();
    for statement in tokens.split(|token| token.symbol(';')) {
        if let Some(table) = create_table(path, statement)? {
            if tables.len() == MAX_TYPES {
                return None;
            }
            tables.push(table);
        }
    }
    Some(tables)
}

fn create_table(path: &str, mut tokens: &[Lexeme<'_>]) -> Option<Option<Declaration>> {
    if !take_word(&mut tokens, "CREATE") {
        return Some(None);
    }
    if at_word(tokens, "TEMP") || at_word(tokens, "TEMPORARY") || at_word(tokens, "UNLOGGED") {
        tokens = &tokens[1..];
    }
    if !take_word(&mut tokens, "TABLE") {
        return Some(None);
    }
    if take_word(&mut tokens, "IF")
        && !(take_word(&mut tokens, "NOT") && take_word(&mut tokens, "EXISTS"))
    {
        return None;
    }
    let line = tokens.first()?.line;
    let name = take_qualified(&mut tokens)?;
    if !tokens.first()?.symbol('(') {
        // CREATE TABLE AS, LIKE, PARTITION OF, etc., do not declare columns here.
        return Some(None);
    }
    let body = parenthesized(&mut tokens)?;
    let mut fields = Vec::new();
    let mut links = Vec::new();
    let mut primary_keys = BTreeSet::new();
    let mut at = 0;
    while at < body.len() {
        let start = at;
        let mut depth = 0;
        while let Some(token) = body.get(at) {
            if depth == 0 && token.symbol(',') {
                break;
            }
            depth = depth_after(depth, token);
            at += 1;
        }
        let mut part = &body[start..at];
        at += 1;
        if part.is_empty() {
            continue;
        }
        if take_word(&mut part, "CONSTRAINT") {
            take_name(&mut part)?;
        }
        if take_word(&mut part, "FOREIGN") && take_word(&mut part, "KEY") {
            let columns = parenthesized(&mut part)?;
            if !take_word(&mut part, "REFERENCES") {
                return None;
            }
            let target = take_qualified(&mut part)?;
            links.extend(
                column_names(columns)
                    .take(MAX_LINKS + 1 - links.len())
                    .map(|field| (field, Target::Name(target.clone()))),
            );
        } else if take_word(&mut part, "PRIMARY") && take_word(&mut part, "KEY") {
            primary_keys.extend(column_names(parenthesized(&mut part)?));
        } else if ![
            "CHECK", "EXCLUDE", "FOREIGN", "INDEX", "KEY", "LIKE", "PRIMARY", "UNIQUE",
        ]
        .iter()
        .any(|keyword| at_word(part, keyword))
        {
            let (field, link) = column(part)?;
            if let Some(target) = link {
                links.push((field.name.clone(), Target::Name(target)));
            }
            fields.push(field);
        }
        if fields.len() > MAX_FIELDS || links.len() > MAX_LINKS {
            return None;
        }
    }
    for field in &mut fields {
        if primary_keys.contains(&field.name) {
            field.optional = false;
        }
    }
    Some(Some(Declaration {
        fact: DataType {
            path: path.to_owned(),
            name,
            line,
            kind: DataKind::Table,
            source: DataSource::Sql,
            fields,
        },
        links,
        schema_pointer: None,
    }))
}

fn column(mut tokens: &[Lexeme<'_>]) -> Option<(DataField, Option<String>)> {
    let line = tokens.first()?.line;
    let name = take_name(&mut tokens)?;
    let mut end = None;
    let mut optional = true;
    let mut link = None;
    let mut depth = 0;
    for (index, token) in tokens.iter().enumerate() {
        if depth == 0 {
            if end.is_none()
                && [
                    "AS",
                    "CHECK",
                    "COLLATE",
                    "COMMENT",
                    "CONSTRAINT",
                    "DEFAULT",
                    "GENERATED",
                    "NOT",
                    "NULL",
                    "PRIMARY",
                    "REFERENCES",
                    "UNIQUE",
                    "AUTO_INCREMENT",
                ]
                .iter()
                .any(|keyword| token.word(keyword))
            {
                end = Some(index);
            }
            if (token.word("NOT") && at_word(&tokens[index + 1..], "NULL"))
                || (token.word("PRIMARY") && at_word(&tokens[index + 1..], "KEY"))
            {
                optional = false;
            }
            if token.word("REFERENCES") {
                link = Some(take_qualified(&mut &tokens[index + 1..])?);
            }
        }
        depth = depth_after(depth, token);
    }
    let ty = column_type(&tokens[..end.unwrap_or(tokens.len())]);
    Some((
        DataField {
            name,
            ty,
            line,
            optional,
        },
        link,
    ))
}

// Strip comments and normalize token spacing, but retain spelling and quoted literals.
fn column_type(tokens: &[Lexeme<'_>]) -> String {
    let mut ty = String::new();
    let mut attach = true;
    for token in tokens {
        if !attach
            && !['(', ')', '.', ',']
                .iter()
                .any(|symbol| token.symbol(*symbol))
            && !token.text.starts_with('[')
        {
            ty.push(' ');
        }
        ty.push_str(token.text);
        attach = token.symbol('(') || token.symbol('.');
    }
    ty
}

fn depth_after(depth: usize, token: &Lexeme<'_>) -> usize {
    if token.symbol('(') {
        depth + 1
    } else if token.symbol(')') {
        depth.saturating_sub(1)
    } else {
        depth
    }
}

fn parenthesized<'a, 'b>(tokens: &mut &'a [Lexeme<'b>]) -> Option<&'a [Lexeme<'b>]> {
    if !tokens.first()?.symbol('(') {
        return None;
    }
    let mut depth = 1;
    for (index, token) in tokens.iter().enumerate().skip(1) {
        depth = depth_after(depth, token);
        if depth == 0 {
            let body = &tokens[1..index];
            *tokens = &tokens[index + 1..];
            return Some(body);
        }
    }
    None
}

fn column_names<'a>(tokens: &'a [Lexeme<'a>]) -> impl Iterator<Item = String> + 'a {
    tokens
        .split(|token| token.symbol(','))
        .filter_map(|mut part| take_name(&mut part))
}

fn at_word(tokens: &[Lexeme<'_>], word: &str) -> bool {
    tokens.first().is_some_and(|token| token.word(word))
}

fn take_word(tokens: &mut &[Lexeme<'_>], word: &str) -> bool {
    if at_word(tokens, word) {
        *tokens = &tokens[1..];
        true
    } else {
        false
    }
}

fn take_name(tokens: &mut &[Lexeme<'_>]) -> Option<String> {
    let (token, rest) = tokens.split_first()?;
    if token.text.len() > 256 {
        return None;
    }
    let name = match token.kind {
        Kind::Word => token.text.to_ascii_lowercase(),
        Kind::Identifier(quote) => {
            let inner = &token.text[1..token.text.len() - 1];
            let quote = char::from(quote);
            inner.replace(&format!("{quote}{quote}"), &quote.to_string())
        }
        _ => return None,
    };
    *tokens = rest;
    (!name.is_empty()).then_some(name)
}

fn take_qualified(tokens: &mut &[Lexeme<'_>]) -> Option<String> {
    let mut name = take_name(tokens)?;
    let mut parts = 1;
    while tokens.first().is_some_and(|token| token.symbol('.')) {
        *tokens = &tokens[1..];
        parts += 1;
        if parts > 3 {
            return None;
        }
        name.push('.');
        name.push_str(&take_name(tokens)?);
    }
    Some(name)
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Word,
    Identifier(u8),
    Symbol,
    Literal,
}

struct Lexeme<'a> {
    kind: Kind,
    text: &'a str,
    line: u64,
}

impl Lexeme<'_> {
    fn symbol(&self, value: char) -> bool {
        matches!(self.kind, Kind::Symbol) && self.text.starts_with(value)
    }

    fn word(&self, value: &str) -> bool {
        matches!(self.kind, Kind::Word) && self.text.eq_ignore_ascii_case(value)
    }
}

fn lex(text: &str) -> Option<Vec<Lexeme<'_>>> {
    let bytes = text.as_bytes();
    let mut at = usize::from(text.starts_with('\u{feff}')) * '\u{feff}'.len_utf8();
    let mut line = 1;
    let mut tokens = Vec::new();
    while let Some(&byte) = bytes.get(at) {
        let start = at;
        let token_line = line;
        let rest = &bytes[start..];
        let (len, kind) = match byte {
            byte if byte.is_ascii_whitespace() => (1, None),
            b'-' if rest.starts_with(b"--") => (
                rest.iter().position(|b| *b == b'\n').unwrap_or(rest.len()),
                None,
            ),
            b'/' if rest.starts_with(b"/*") => (block_comment_len(rest)?, None),
            b'E' | b'e' if rest.get(1) == Some(&b'\'') => (
                1 + quoted_len(&rest[1..], b'\'', true)?,
                Some(Kind::Literal),
            ),
            b'\'' => (quoted_len(rest, byte, false)?, Some(Kind::Literal)),
            b'"' | b'`' => (quoted_len(rest, byte, false)?, Some(Kind::Identifier(byte))),
            b'[' => (quoted_len(rest, b']', false)?, Some(Kind::Identifier(b']'))),
            b'$' => match dollar_tag_len(rest) {
                Some(tag_len) => {
                    let tag = &text[start..start + tag_len];
                    let len = text[start + tag_len..].find(tag)? + tag_len * 2;
                    (len, Some(Kind::Literal))
                }
                None => (1, Some(Kind::Symbol)),
            },
            byte if word_byte(byte) => {
                let len = rest
                    .iter()
                    .position(|byte| !word_byte(*byte))
                    .unwrap_or(rest.len());
                (len, Some(Kind::Word))
            }
            _ => (1, Some(Kind::Symbol)),
        };
        at += len;
        line += rest[..len].iter().filter(|byte| **byte == b'\n').count() as u64;
        if let Some(kind) = kind {
            if tokens.len() == 128_000 {
                return None;
            }
            tokens.push(Lexeme {
                kind,
                text: text.get(start..at)?,
                line: token_line,
            });
        }
    }
    Some(tokens)
}

fn word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$') || byte >= 0x80
}

fn quoted_len(bytes: &[u8], closing: u8, backslash: bool) -> Option<usize> {
    let mut at = 1;
    while let Some(&byte) = bytes.get(at) {
        at += 1;
        if backslash && byte == b'\\' {
            at += 1;
        } else if byte == closing {
            if bytes.get(at) != Some(&closing) {
                return Some(at);
            }
            at += 1;
        }
    }
    None
}

fn dollar_tag_len(bytes: &[u8]) -> Option<usize> {
    match bytes.get(1)? {
        b'$' => Some(2),
        b'_' | b'a'..=b'z' | b'A'..=b'Z' => {
            let end = bytes
                .iter()
                .skip(2)
                .position(|b| !b.is_ascii_alphanumeric() && *b != b'_')?
                + 2;
            (bytes[end] == b'$').then_some(end + 1)
        }
        _ => None,
    }
}

fn block_comment_len(bytes: &[u8]) -> Option<usize> {
    let (mut level, mut at) = (1, 2);
    while let Some(pair) = bytes.get(at..at + 2) {
        match pair {
            b"/*" => {
                level += 1;
                at += 2;
            }
            b"*/" => {
                level -= 1;
                at += 2;
                if level == 0 {
                    return Some(at);
                }
            }
            _ => at += 1,
        }
    }
    None
}
