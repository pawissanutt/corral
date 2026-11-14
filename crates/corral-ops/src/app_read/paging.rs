//! Deterministic typed continuation tokens and JSON-envelope byte budgeting.
use anyhow::{Context, Result, bail};
use corral_protocol::{
    app::APP_CONTROL_MAX,
    detail::{DetailBody, DetailReply},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// Reserve room for the RPC envelope and cursor, including escaped JSON text.
const BODY_BUDGET: usize = APP_CONTROL_MAX - 4096;

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Cursor {
    version: u8,
    selector: String,
    etag: String,
    pub offset: usize,
    pub as_of: String,
    #[serde(default)]
    pub source: Option<String>,
}

pub(super) fn decode(raw: Option<&str>, selector: &str) -> Result<Option<Cursor>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let cursor: Cursor = serde_json::from_str(raw).context("invalid app detail page token")?;
    if cursor.version != 1
        || cursor.selector != selector
        || cursor.etag.len() != 64
        || cursor.as_of.len() > 64
        || (cursor.offset == 0 && cursor.source.is_none())
        || cursor
            .source
            .as_ref()
            .is_some_and(|source| source.is_empty() || source.len() > 128)
        || cursor.as_of.parse::<jiff::Timestamp>().is_err()
    {
        bail!("invalid app detail page token");
    }
    Ok(Some(cursor))
}

pub(super) fn check(cursor: Option<&Cursor>, etag: &str) -> Result<()> {
    if cursor.is_some_and(|cursor| cursor.etag != etag) {
        bail!("app detail page is stale; reload the first page");
    }
    Ok(())
}

pub(super) fn reply(
    body: DetailBody,
    next: Option<usize>,
    etag: String,
    selector: String,
    as_of: String,
    seq: u64,
) -> Result<DetailReply> {
    let next = next
        .map(|offset| {
            serde_json::to_string(&Cursor {
                version: 1,
                selector,
                etag: etag.clone(),
                offset,
                as_of: as_of.clone(),
                source: None,
            })
        })
        .transpose()?;
    let reply = DetailReply {
        etag,
        as_of,
        seq,
        next,
        body: Some(body),
    };
    // The gateway wraps the result in RPC; this reserve exceeds its fixed envelope.
    if json_len(&reply)? > APP_CONTROL_MAX - 2048 {
        bail!("app detail page exceeds CONTROL budget");
    }
    Ok(reply)
}

pub(super) fn source_reply(
    body: DetailBody,
    next: Option<String>,
    etag: String,
    selector: String,
    as_of: String,
    seq: u64,
) -> Result<DetailReply> {
    let next = next
        .map(|source| {
            serde_json::to_string(&Cursor {
                version: 1,
                selector,
                etag: etag.clone(),
                offset: 0,
                as_of: as_of.clone(),
                source: Some(source),
            })
        })
        .transpose()?;
    let reply = DetailReply {
        etag,
        as_of,
        seq,
        next,
        body: Some(body),
    };
    if json_len(&reply)? > APP_CONTROL_MAX - 2048 {
        bail!("app detail page exceeds CONTROL budget");
    }
    Ok(reply)
}

struct Group {
    path: Vec<String>,
    values: Vec<Value>,
}

/// Split arrays without changing the protocol type. Objects containing arrays
/// (for example plan.document) retain their scalar metadata on every page.
/// An element of an array is an atomic domain unit; reader-specific limits keep
/// evidence blocks, messages and timeline lanes individually bounded.
pub(super) fn body_page(body: DetailBody, offset: usize) -> Result<(DetailBody, Option<usize>)> {
    let mut skeleton = serde_json::to_value(body)?;
    let mut groups = Vec::new();
    take_arrays(&mut skeleton, &mut Vec::new(), &mut groups);
    let metadata = json_len(&skeleton)?;
    if metadata >= BODY_BUDGET {
        bail!("app detail scalar metadata exceeds page budget");
    }
    for group in &mut groups {
        let values = std::mem::take(&mut group.values);
        for value in values {
            let budget = BODY_BUDGET - metadata - 1;
            if json_len(&value)? <= budget {
                group.values.push(value);
            } else {
                group.values.extend(chunks(value, budget)?);
            }
        }
    }
    let total = groups.iter().try_fold(0usize, |total, group| {
        total
            .checked_add(group.values.len())
            .context("app detail item count overflow")
    })?;
    if offset > total || (offset != 0 && offset == total) {
        bail!("invalid app detail page offset");
    }
    let mut bytes = json_len(&skeleton)?;
    if bytes > BODY_BUDGET {
        bail!("app detail scalar metadata exceeds page budget");
    }
    let mut index = 0usize;
    let mut accepted = 0usize;
    let mut next = None;
    'groups: for group in groups {
        for value in group.values {
            let current = index;
            index += 1;
            if current < offset {
                continue;
            }
            let size = json_len(&value)?
                .checked_add(1)
                .context("app detail size overflow")?;
            if bytes.saturating_add(size) > BODY_BUDGET {
                if accepted == 0 {
                    bail!("app detail item exceeds page budget");
                }
                next = Some(current);
                break 'groups;
            }
            array_at(&mut skeleton, &group.path)?.push(value);
            bytes += size;
            accepted += 1;
        }
    }
    Ok((
        serde_json::from_value(skeleton).context("assembling typed app detail page")?,
        next,
    ))
}

fn take_arrays(value: &mut Value, path: &mut Vec<String>, groups: &mut Vec<Group>) {
    match value {
        // Four task rails form a fixed-size tuple, not a paged collection.
        Value::Array(_) if path.last().is_some_and(|name| name == "stages") => {}
        Value::Array(values) => groups.push(Group {
            path: path.clone(),
            values: std::mem::take(values),
        }),
        Value::Object(fields) => {
            for (name, value) in fields {
                path.push(name.clone());
                take_arrays(value, path, groups);
                path.pop();
            }
        }
        _ => {}
    }
}

/// Canonical-ID/date parents may span pages. Clients merge these objects by ID
/// (or UsageDay.day) before appending their nested collections.
fn chunks(value: Value, budget: usize) -> Result<Vec<Value>> {
    if json_len(&value)? <= budget {
        return Ok(vec![value]);
    }
    if value
        .get("id")
        .or_else(|| value.get("day"))
        .and_then(Value::as_str)
        .is_none()
    {
        bail!("app detail item exceeds page budget without a canonical merge identity");
    }
    let mut base = value;
    let mut groups = Vec::new();
    take_arrays(&mut base, &mut Vec::new(), &mut groups);
    let metadata = json_len(&base)?;
    if metadata >= budget || groups.is_empty() {
        bail!("app detail item metadata exceeds page budget");
    }
    let mut result = Vec::new();
    let mut current = base.clone();
    let mut bytes = metadata;
    let mut accepted = false;
    let mut nested = Vec::new();
    for group in groups {
        for value in group.values {
            if json_len(&value)? <= budget - metadata - 1 {
                nested.push(value);
            } else {
                nested.extend(chunks(value, budget - metadata - 1)?);
            }
            for value in nested.drain(..) {
                let size = json_len(&value)? + 1;
                if bytes + size > budget {
                    if !accepted {
                        bail!("app detail nested item exceeds page budget");
                    }
                    result.push(current);
                    current = base.clone();
                    bytes = metadata;
                }
                array_at(&mut current, &group.path)?.push(value);
                bytes += size;
                accepted = true;
            }
        }
    }
    if accepted {
        result.push(current);
    }
    if result.is_empty() {
        bail!("app detail item exceeds page budget");
    }
    Ok(result)
}

fn array_at<'a>(value: &'a mut Value, path: &[String]) -> Result<&'a mut Vec<Value>> {
    let mut value = value;
    for name in path {
        value = value
            .get_mut(name)
            .context("invalid app detail array path")?;
    }
    value.as_array_mut().context("invalid app detail array")
}

pub(super) fn json_len(value: &impl Serialize) -> Result<usize> {
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value)?;
    Ok(counter.0)
}

struct Counter(usize);
impl std::io::Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .ok_or_else(|| std::io::Error::other("JSON size overflow"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
