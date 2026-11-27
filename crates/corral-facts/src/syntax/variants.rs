//! Conservative enum-variant coverage for lenses, using the entry scanner's lexer
//! and variant grammar. An unsupported or ambiguous declaration supplies no list.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    eat,
    entries::{Lexer, enum_body, take_group, tokens, variant_tail},
    take_id,
};

pub(crate) struct RustEnum {
    pub variants: BTreeMap<String, usize>,
    pub variant_bytes: usize,
}

#[derive(Default)]
pub(crate) struct Declarations {
    pub enums: BTreeMap<String, RustEnum>,
    // Only requested names; None means more than one declaration, not missing.
    pub symbols: BTreeMap<String, Option<usize>>,
}

pub(crate) fn rust_declarations(text: &str, wanted: &BTreeSet<&str>) -> Declarations {
    if wanted.is_empty() {
        return Declarations::default();
    }
    let tokens = tokens(Lexer::Rust, text);
    let mut rest = tokens.as_slice();
    let mut found = Declarations::default();
    let mut seen = BTreeSet::new();
    let start = text.as_ptr() as usize;
    while let Some((&token, tail)) = rest.split_first() {
        let before = rest;
        rest = tail;
        // Do not gather declarations from attributes or opaque macro input.
        let group = match before {
            ["#", "[", ..] => Some(tail),
            ["macro_rules", "!", _, group @ ..] | ["!", group @ ..] => Some(group),
            _ => None,
        };
        if let Some(mut group) = group
            && let Some(&opener @ ("[" | "(" | "{")) = group.first()
            && take_group(&mut group, opener).is_some()
        {
            rest = group;
            continue;
        }
        if matches!(
            token,
            "fn" | "struct" | "enum" | "trait" | "type" | "mod" | "const" | "static" | "union"
        ) && let Some(name) = take_id(&mut &tail[..]).filter(|name| wanted.contains(name))
        {
            found
                .symbols
                .entry(name.to_owned())
                .and_modify(|offset| *offset = None)
                .or_insert(Some(token.as_ptr() as usize - start));
        }
        if token != "enum" {
            continue;
        }
        let mut tail = tail;
        let Some(name) = take_id(&mut tail).filter(|name| wanted.contains(name)) else {
            continue;
        };
        if !seen.insert(name) {
            found.enums.remove(name);
        } else if let Some(body) = enum_body(tail)
            && let Some(variants) = enum_variants(body, start)
        {
            found.enums.insert(
                name.to_owned(),
                RustEnum {
                    variant_bytes: variants.keys().map(|name| name.len() + 64).sum(),
                    variants,
                },
            );
        }
    }
    found
}

fn enum_variants(mut body: &[&str], start: usize) -> Option<BTreeMap<String, usize>> {
    let mut variants = BTreeMap::new();
    while !body.is_empty() {
        while eat(&mut body, "#") {
            take_group(&mut body, "[")?;
        }
        let token = body.first()?;
        let name = take_id(&mut body)?;
        if !variant_tail(&mut body)
            || variants
                .insert(name.to_owned(), token.as_ptr() as usize - start)
                .is_some()
        {
            return None;
        }
    }
    Some(variants)
}
