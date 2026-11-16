//! Incremental closed-file accounting; only resumable session identities retain dedup keys.
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use super::*;
use crate::agent::usage::read::Entry;

#[derive(Serialize, Deserialize)]
struct Folded {
    identity: String,
    sessions: BTreeSet<String>,
    slot: u32,
}

#[derive(Serialize, Deserialize)]
struct Closed {
    totals: Saved,
    folded: BTreeMap<PathBuf, Folded>,
    files: Vec<PathBuf>,
    // A SHA-256 key plus a file-table index, never a response row or repeated path.
    keys: BTreeMap<String, u32>,
    kept_sessions: BTreeSet<String>,
    excluded: BTreeSet<String>,
    reason: Option<String>,
}

pub(super) struct Fold {
    pub facts: Aggregate,
    keys: BTreeMap<String, u32>,
    files: Vec<PathBuf>,
}

impl Fold {
    pub fn precedes(&self, entry: &Entry) -> bool {
        self.keys
            .get(&entry.key)
            .and_then(|slot| self.files.get(*slot as usize))
            .is_some_and(|path| *path < entry.path)
    }
}

fn sessions<'a>(bound: impl IntoIterator<Item = &'a str>, entries: &[Entry]) -> BTreeSet<String> {
    bound
        .into_iter()
        .map(|session| Sha256::digest(session.as_bytes()))
        .chain(entries.iter().map(|entry| entry.session))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|hash| format!("{hash:x}"))
        .collect()
}

fn file_ids(inputs: &Inputs) -> Result<BTreeMap<PathBuf, String>> {
    stamps(inputs)?
        .into_iter()
        .map(|(path, stamp)| {
            let input = &inputs.files[path];
            let bindings: Vec<_> = input
                .owners
                .iter()
                .map(|n| (n, inputs.bindings.get(n)))
                .collect();
            Ok((
                path.to_path_buf(),
                fingerprint(&(&input.session, &input.model, bindings, stamp))?,
            ))
        })
        .collect()
}

fn exclusions(saved: &Closed, live: &[Entry]) -> BTreeSet<String> {
    live.iter()
        .filter(|entry| {
            saved
                .keys
                .get(&entry.key)
                .and_then(|slot| saved.files.get(*slot as usize))
                .is_some_and(|path| entry.path < *path)
        })
        .map(|entry| entry.key.clone())
        .collect()
}

fn reusable(
    saved: &Closed,
    ids: &BTreeMap<PathBuf, String>,
    live_sessions: &BTreeSet<String>,
    now: Timestamp,
) -> bool {
    saved.totals.valid(now)
        && saved
            .keys
            .values()
            .all(|slot| (*slot as usize) < saved.files.len())
        && saved.folded.iter().all(|(path, folded)| {
            ids.get(path) == Some(&folded.identity)
                && saved.files.get(folded.slot as usize) == Some(path)
                && folded
                    .sessions
                    .intersection(live_sessions)
                    .all(|id| saved.kept_sessions.contains(id))
        })
}

pub(super) fn fold(
    ctx: &Ctx,
    key: &str,
    closed: &Inputs,
    live: &Inputs,
    live_entries: &[Entry],
    profile: &Option<String>,
    now: Timestamp,
) -> Result<Fold> {
    let ids = file_ids(closed)?;
    let live_sessions = sessions(
        live.files.values().map(|input| input.session.as_str()),
        live_entries,
    );
    let live_keys: BTreeMap<_, _> = live_entries
        .iter()
        .map(|entry| (&entry.key, &entry.path))
        .collect();
    let identity = fingerprint(&(profile, pricing(ctx)?))?;
    let key = format!("closed:{key}");
    let candidate = load::<Closed>(ctx, &key, &identity).filter(|saved| {
        reusable(saved, &ids, &live_sessions, now)
            && saved.excluded == exclusions(saved, live_entries)
    });
    let mut rebuilding = candidate.is_none();
    let mut saved = candidate.unwrap_or_else(|| Closed {
        totals: Saved::new(&Aggregate::new(usize::MAX)),
        folded: BTreeMap::new(),
        files: Vec::new(),
        keys: BTreeMap::new(),
        kept_sessions: BTreeSet::new(),
        excluded: BTreeSet::new(),
        reason: None,
    });
    let mut scanned = 0;
    loop {
        let mut facts = saved.totals.restore(Timestamp::MIN);
        let prior_sessions: BTreeSet<_> = saved
            .folded
            .values()
            .flat_map(|file| file.sessions.iter().cloned())
            .collect();
        let mut restart = false;
        for (path, file_identity) in &ids {
            if saved.folded.contains_key(path) {
                continue;
            }
            let input = &closed.files[path];
            let (entries, coverage) =
                read::collect_files(std::iter::once((path, input)), &closed.bindings, None)?;
            scanned += coverage.scanned_rows as u64;
            let file_sessions = sessions(std::iter::once(input.session.as_str()), &entries);
            // A new file can resume a session whose old keys were deliberately pruned.
            // Rebuild only that exceptional overlap, never every new generation.
            if !rebuilding
                && file_sessions
                    .intersection(&prior_sessions)
                    .any(|id| !saved.kept_sessions.contains(id))
            {
                restart = true;
                break;
            }
            if entries.iter().any(|entry| {
                saved
                    .keys
                    .get(&entry.key)
                    .and_then(|slot| saved.files.get(*slot as usize))
                    .is_some_and(|old| path < old)
            }) {
                restart = true;
                break;
            }
            if coverage.partial {
                saved.reason = Some(
                    coverage
                        .reason
                        .unwrap_or("partial_closed_transcript")
                        .to_owned(),
                );
            }
            let slot = u32::try_from(saved.files.len()).context("too many closed usage files")?;
            saved.files.push(path.clone());
            for entry in entries {
                if saved.keys.contains_key(&entry.key) {
                    continue;
                }
                if live_keys
                    .get(&entry.key)
                    .is_some_and(|path| **path < entry.path)
                {
                    saved.excluded.insert(entry.key.clone());
                } else {
                    add(ctx, &mut facts, profile, entry.usage, now);
                }
                saved.keys.insert(entry.key, slot);
            }
            saved.folded.insert(
                path.clone(),
                Folded {
                    identity: file_identity.clone(),
                    sessions: file_sessions,
                    slot,
                },
            );
        }
        if restart {
            saved = Closed {
                totals: Saved::new(&Aggregate::new(usize::MAX)),
                folded: BTreeMap::new(),
                files: Vec::new(),
                keys: BTreeMap::new(),
                kept_sessions: BTreeSet::new(),
                excluded: BTreeSet::new(),
                reason: None,
            };
            rebuilding = true;
            continue;
        }
        saved.totals = Saved::new(&facts);
        // Keep keys only for closed files whose session can dedup against a live file.
        // If an older session is resumed later, its retained session hash forces a rebuild.
        let keep_slots: BTreeSet<_> = saved
            .folded
            .values()
            .filter(|file| !file.sessions.is_disjoint(&live_sessions))
            .map(|file| file.slot)
            .collect();
        saved.keys.retain(|_, slot| keep_slots.contains(slot));
        saved.kept_sessions = live_sessions
            .intersection(
                &saved
                    .folded
                    .values()
                    .flat_map(|file| file.sessions.iter().cloned())
                    .collect(),
            )
            .cloned()
            .collect();
        if !facts.future && ids == file_ids(closed)? && !live.files.is_empty() {
            save(ctx, &key, &identity, &saved);
        }
        facts.budget.coverage.scanned_rows = scanned;
        if let Some(reason) = &saved.reason {
            facts.budget.partial(reason);
        }
        return Ok(Fold {
            facts,
            keys: saved.keys,
            files: saved.files,
        });
    }
}
