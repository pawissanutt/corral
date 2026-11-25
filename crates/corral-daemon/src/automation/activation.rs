//! The config rename commits a digest-bound, durable pending baseline.
use super::*;

const LIMIT: u64 = 4 * 1024 * 1024;

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Activation {
    pub(super) effective: String,
    pub(super) generation: u64,
    pub(super) cursor: u64,
    pub(super) minute: i64,
}

#[derive(Deserialize, Serialize)]
pub(super) struct Manifest {
    digest: Option<[u8; 32]>,
    generation: u64,
    pub(super) rules: BTreeMap<String, Activation>,
}

fn recover(fence: &Fence, digest: [u8; 32]) -> Result<Option<Manifest>> {
    let dir = &fence.dir;
    let pending = dir.join(".activation-pending");
    let committed = dir.join(".activation");
    if let Some(manifest) = read::<Manifest>(&pending, LIMIT)? {
        if manifest.digest == Some(digest) {
            // Make the config rename durable before replacing the previous baseline.
            File::open(dir.parent().context("automation state parent")?)?.sync_all()?;
            fs::rename(&pending, &committed)?;
            File::open(dir)?.sync_all()?;
            return Ok(Some(manifest));
        }
        // A candidate interrupted before publication must not become active later.
        fs::remove_file(&pending)?;
        File::open(dir)?.sync_all()?;
    }
    read(&committed, LIMIT)
}

pub(super) fn load(fence: &Fence, digest: [u8; 32]) -> Result<Option<Manifest>> {
    let Some(mut manifest) = recover(fence, digest)? else {
        return Ok(None);
    };
    if manifest.digest == Some(digest) {
        return Ok(Some(manifest));
    }
    // An observed hand edit supersedes the old baseline. Restoring the old
    // bytes later must not resurrect a cursor after record cleanup.
    if manifest.digest.take().is_some() {
        manifest.rules.clear();
        save(&fence.dir, &fence.dir.join(".activation"), &manifest)?;
    }
    Ok(None)
}

pub(crate) fn stage(
    fence: &Fence,
    current: [u8; 32],
    digest: [u8; 32],
    config: &Config,
    cursor: u64,
    now: Timestamp,
) -> Result<()> {
    let previous = recover(fence, current)?;
    if current == digest {
        return Ok(());
    }
    let mut high_generation = previous.as_ref().map_or(0, |m| m.generation);
    let live = previous.as_ref().filter(|m| m.digest == Some(current));
    let mut current_config = None;
    let mut rules = BTreeMap::new();
    let mut changed = Vec::new();
    let minute = now.as_second().div_euclid(60);
    for id in config.automation.rules.keys() {
        ensure!(safe_id(id), "invalid automation rule ID: {id:?}");
        let spec = effective(config, id)?;
        let old = live.and_then(|m| m.rules.get(id));
        if let Some(old) = old.filter(|a| a.effective == spec) {
            rules.insert(id.clone(), old.clone());
            continue;
        }

        let worker = record(&fence.dir.join(format!("{id}.json")))?;
        if let Some(worker) = worker {
            high_generation = high_generation.max(worker.generation);
            // A hand-configured rule may already have a durable cursor, even
            // without a manifest baseline. Compare against the live config too:
            // the record might predate an unobserved disable or removal.
            if old.is_none() && worker.effective == spec {
                let live_config = current_config.get_or_insert_with(|| {
                    let text = fs::read_to_string(fence.dir.parent()?.join("corral.toml")).ok()?;
                    let actual: [u8; 32] = Sha256::digest(text.as_bytes()).into();
                    if actual != current {
                        return None;
                    }
                    Config::parse(&text).ok()
                });
                if live_config
                    .as_ref()
                    .and_then(|c| effective(c, id).ok())
                    .is_some_and(|effective| effective == spec)
                {
                    rules.insert(
                        id.clone(),
                        Activation {
                            effective: spec,
                            generation: worker.generation,
                            cursor: worker.cursor,
                            minute: worker.minute,
                        },
                    );
                    continue;
                }
            }
        }
        changed.push((id, spec, old.map_or(minute, |a| a.minute.max(minute))));
    }
    let generation = high_generation
        .checked_add(1)
        .context("automation generation exhausted")?;
    for (id, effective, minute) in changed {
        rules.insert(
            id.clone(),
            Activation {
                effective,
                generation,
                cursor,
                minute,
            },
        );
    }
    let manifest = Manifest {
        digest: Some(digest),
        generation,
        rules,
    };
    ensure!(
        (serde_json::to_vec(&manifest)?.len() as u64) < LIMIT,
        "oversized automation activation manifest"
    );
    save(
        &fence.dir,
        &fence.dir.join(".activation-pending"),
        &manifest,
    )
}

#[cfg(test)]
#[path = "activation_tests.rs"]
mod tests;
