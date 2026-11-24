//! Rust source declarations, not expanded macros or resolved imports.

use std::collections::BTreeSet;
use std::thread;

use proc_macro2::{Delimiter, Spacing, TokenStream, TokenTree};
use quote::ToTokens;
use syn::ext::IdentExt;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Attribute, Fields, GenericArgument, Item, Path, PathArguments, Token, Type};

use super::{Declaration, MAX_FIELDS, MAX_NAME_BYTES, MAX_TYPES, Target};
use crate::{DataField, DataKind, DataSource, DataType};

const MAX_BYTES: usize = 1024 * 1024;

pub(super) fn read(path: &str, text: &str) -> Option<Vec<Declaration>> {
    if !path.ends_with(".rs") || text.len() > MAX_BYTES {
        return None;
    }
    if !text.contains("derive") || !(text.contains("Serialize") || text.contains("Deserialize")) {
        return Some(Vec::new());
    }
    // proc-macro2's span map retains the entire source in thread-local storage. Keep
    // the tokens, AST and span map on a short-lived thread with a known stack size.
    thread::scope(|scope| {
        thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn_scoped(scope, || parse(path, text))
            .ok()?
            .join()
            .ok()?
    })
}

fn parse(path: &str, text: &str) -> Option<Vec<Declaration>> {
    let mut text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if let Some(shebang) = text.strip_prefix("#!")
        && !shebang.trim_start().starts_with('[')
    {
        // Leave the newline so span line numbers still refer to the original file.
        text = text.find('\n').map_or("", |newline| &text[newline..]);
    }
    let tokens = text.parse::<TokenStream>().ok()?;
    bounded(tokens.clone(), 0, &mut 96_000)?;
    let file: syn::File = syn::parse2(tokens).ok()?;
    let mut declarations = Vec::new();
    read_items(path, "", file.items, &mut declarations)?;
    Some(declarations)
}

/// proc-macro2's fallback lexer and token destructor are iterative, but syn's parser
/// and AST destructor are not. Bound both group nesting and ungrouped chains (such
/// as `&&&T`, nested generics or `else if`): delimiter depth alone is insufficient.
fn bounded(tokens: TokenStream, depth: usize, remaining: &mut usize) -> Option<usize> {
    if depth > 24 {
        return None;
    }
    let (mut chain, mut child, mut peak, mut angles, mut pipes) = (0usize, 0, 0, 0usize, false);
    let (mut after_block, mut after_arrow_dash) = (false, false);
    for token in tokens {
        *remaining = remaining.checked_sub(1)?;
        let (next_block, next_dash) = match token {
            TokenTree::Group(group) => {
                child = child.max(bounded(group.stream(), depth + 1, remaining)?);
                chain += 1;
                (group.delimiter() == Delimiter::Brace, false)
            }
            TokenTree::Punct(punct) => {
                let symbol = punct.as_char();
                match symbol {
                    ';' => (chain, child, angles, pipes) = (0, 0, 0, false),
                    '#' if after_block => (chain, child, angles, pipes) = (1, 0, 0, false),
                    ',' if angles == 0 && !pipes => (chain, child) = (0, 0),
                    '<' => {
                        angles += 1;
                        chain += 1;
                    }
                    '>' if !after_arrow_dash => {
                        angles = angles.saturating_sub(1);
                        chain += 1;
                    }
                    '|' => {
                        pipes = true;
                        chain += 1;
                    }
                    // Path separators don't add recursive branches to syn's AST.
                    ':' | '\'' => {}
                    _ => chain += 1,
                }
                (
                    false,
                    punct.as_char() == '-' && punct.spacing() == Spacing::Joint,
                )
            }
            TokenTree::Ident(ident) => {
                let ident = ident.to_string();
                if ident.len() > MAX_NAME_BYTES {
                    return None;
                }
                match ident.as_str() {
                    "async" | "as" | "become" | "break" | "else" | "for" | "if" | "let"
                    | "loop" | "match" | "move" | "return" | "while" | "yield" => chain += 1,
                    "const" | "enum" | "fn" | "impl" | "mod" | "pub" | "static" | "struct"
                    | "trait" | "type" | "union" | "use"
                        if after_block =>
                    {
                        (chain, child, angles, pipes) = (0, 0, 0, false);
                    }
                    _ => {}
                }
                (false, false)
            }
            TokenTree::Literal(_) => (false, false),
        };
        peak = peak.max(chain + child);
        if peak > 96 || angles > 32 {
            return None;
        }
        (after_block, after_arrow_dash) = (next_block, next_dash);
    }
    Some(peak)
}

fn read_items(
    path: &str,
    module: &str,
    items: Vec<Item>,
    declarations: &mut Vec<Declaration>,
) -> Option<()> {
    for item in items {
        let (ident, kind, fields, links) = match item {
            Item::Mod(item) => {
                if let Some((_, items)) = item.content {
                    let module = qualified(module, &item.ident.unraw().to_string());
                    if module.len() > MAX_NAME_BYTES {
                        return None;
                    }
                    read_items(path, &module, items, declarations)?;
                }
                continue;
            }
            Item::Struct(item) if serde_derive(&item.attrs) => {
                if item.fields.len() > MAX_FIELDS {
                    return None;
                }
                let mut links = Vec::new();
                let fields = item
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(index, field)| {
                        let (name, line) = field.ident.as_ref().map_or_else(
                            || (index.to_string(), field.ty.span().start().line),
                            |ident| (ident.unraw().to_string(), ident.span().start().line),
                        );
                        add_links(&mut links, &name, module, [&field.ty]);
                        DataField {
                            name,
                            ty: type_text(&field.ty),
                            line: line as u64,
                            optional: is_option(&field.ty),
                        }
                    })
                    .collect();
                (item.ident, DataKind::Record, fields, links)
            }
            Item::Enum(item) if serde_derive(&item.attrs) => {
                if item.variants.len() > MAX_FIELDS {
                    return None;
                }
                let mut links = Vec::new();
                let fields = item
                    .variants
                    .iter()
                    .map(|variant| {
                        let name = variant.ident.unraw().to_string();
                        add_links(
                            &mut links,
                            &name,
                            module,
                            variant.fields.iter().map(|field| &field.ty),
                        );
                        DataField {
                            name,
                            ty: variant_text(&variant.fields),
                            line: variant.ident.span().start().line as u64,
                            optional: false,
                        }
                    })
                    .collect();
                (item.ident, DataKind::Enum, fields, links)
            }
            _ => continue,
        };
        let name = qualified(module, &ident.unraw().to_string());
        if name.len() > MAX_NAME_BYTES || declarations.len() == MAX_TYPES {
            return None;
        }
        declarations.push(Declaration {
            fact: DataType {
                path: path.to_owned(),
                name,
                line: ident.span().start().line as u64,
                kind,
                source: DataSource::RustSerde,
                fields,
            },
            links,
            schema_pointer: None,
        });
    }
    Some(())
}

fn serde_derive(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("derive")
            && attr
                .parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)
                .is_ok_and(|paths| {
                    paths.iter().any(|path| {
                        path.segments.last().is_some_and(|segment| {
                            segment.ident == "Serialize" || segment.ident == "Deserialize"
                        })
                    })
                })
    })
}

// Treat spacing and comments as trivia, so reformatting cannot retype a field.
fn type_text(ty: &Type) -> String {
    ty.to_token_stream().to_string()
}

fn variant_text(fields: &Fields) -> String {
    match fields {
        Fields::Named(fields) => {
            let payload = fields
                .named
                .iter()
                .filter_map(|field| {
                    Some(format!(
                        "{}: {}",
                        field.ident.as_ref()?.unraw(),
                        type_text(&field.ty)
                    ))
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{ {payload} }}")
        }
        Fields::Unnamed(fields) => {
            let payload = fields
                .unnamed
                .iter()
                .map(|field| type_text(&field.ty))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({payload})")
        }
        Fields::Unit => "()".to_owned(),
    }
}

fn qualified(module: &str, name: &str) -> String {
    if module.is_empty() {
        name.to_owned()
    } else {
        format!("{module}::{name}")
    }
}

fn is_option(ty: &Type) -> bool {
    match ty {
        Type::Path(ty) if ty.qself.is_none() => ty
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "Option"),
        Type::Paren(ty) => is_option(&ty.elem),
        Type::Group(ty) => is_option(&ty.elem),
        _ => false,
    }
}

fn add_links<'a>(
    links: &mut Vec<(String, Target)>,
    field: &str,
    module: &str,
    types: impl IntoIterator<Item = &'a Type>,
) {
    let mut names = BTreeSet::new();
    for ty in types {
        collect_names(&mut names, module, ty);
    }
    links.extend(
        names
            .into_iter()
            .map(|name| (field.to_owned(), Target::Name(name))),
    );
}

fn collect_names(names: &mut BTreeSet<String>, module: &str, ty: &Type) {
    match ty {
        Type::Path(ty) if ty.qself.is_none() => {
            let path = &ty.path;
            if let Some(last) = path.segments.last() {
                // Unresolved imports and trait-associated types are deliberately not followed.
                let last_name = last.ident.unraw().to_string();
                let wrapper = matches!(
                    last_name.as_str(),
                    "Arc"
                        | "Box"
                        | "Cow"
                        | "Option"
                        | "Rc"
                        | "Result"
                        | "Vec"
                        | "HashMap"
                        | "BTreeMap"
                        | "HashSet"
                        | "BTreeSet"
                        | "VecDeque"
                );
                if !wrapper || path.segments.len() > 1 && !standard_path(path) {
                    names.insert(type_name(module, path));
                }
                for segment in &path.segments {
                    if let PathArguments::AngleBracketed(args) = &segment.arguments {
                        for arg in &args.args {
                            if let GenericArgument::Type(ty) = arg {
                                collect_names(names, module, ty);
                            }
                        }
                    }
                }
            }
        }
        Type::Array(ty) => collect_names(names, module, &ty.elem),
        Type::Group(ty) => collect_names(names, module, &ty.elem),
        Type::Paren(ty) => collect_names(names, module, &ty.elem),
        Type::Reference(ty) => collect_names(names, module, &ty.elem),
        Type::Slice(ty) => collect_names(names, module, &ty.elem),
        Type::Tuple(ty) => {
            for ty in &ty.elems {
                collect_names(names, module, ty);
            }
        }
        _ => {}
    }
}

fn standard_path(path: &Path) -> bool {
    path.segments.first().is_some_and(|segment| {
        segment.ident == "std" || segment.ident == "core" || segment.ident == "alloc"
    })
}

fn type_name(module: &str, path: &Path) -> String {
    let mut parts = path
        .segments
        .iter()
        .map(|part| part.ident.unraw().to_string())
        .peekable();
    let mut module = module;
    let relative = match parts.peek().map(String::as_str) {
        Some("crate") => {
            parts.next();
            false
        }
        Some("self") => {
            parts.next();
            true
        }
        Some("super") => {
            while parts.peek().is_some_and(|part| part == "super") {
                parts.next();
                module = module.rsplit_once("::").map_or("", |(parent, _)| parent);
            }
            true
        }
        _ => false,
    };
    let name = parts.collect::<Vec<_>>().join("::");
    if relative {
        qualified(module, &name)
    } else {
        name
    }
}
