//! Conservative enum-variant coverage for lenses, using the entry scanner's lexer
//! and variant grammar. An unsupported or ambiguous declaration supplies no list.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    eat,
    entries::{Lexer, enum_body, take_group, tokens, variant_tail},
    take_id,
};

pub(crate) struct RustEnum {
    pub declaration: usize,
    pub variants: BTreeMap<String, usize>,
    pub variant_bytes: usize,
}

pub(crate) fn rust_enums(text: &str, wanted: &BTreeSet<&str>) -> BTreeMap<String, RustEnum> {
    if wanted.is_empty() {
        return BTreeMap::new();
    }
    let tokens = tokens(Lexer::Rust, text);
    let mut rest = tokens.as_slice();
    let mut found = BTreeMap::new();
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
        if token != "enum" {
            continue;
        }
        let mut tail = tail;
        let Some(name) = take_id(&mut tail).filter(|name| wanted.contains(name)) else {
            continue;
        };
        if !seen.insert(name) {
            found.remove(name);
        } else if let Some(body) = enum_body(tail)
            && let Some(variants) = enum_variants(body, start)
        {
            found.insert(
                name.to_owned(),
                RustEnum {
                    declaration: token.as_ptr() as usize - start,
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
