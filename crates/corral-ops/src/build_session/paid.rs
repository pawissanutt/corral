//! Capability-authenticated, off-board Build spending. Never grants task/automation authority.
use anyhow::{Context, Result, ensure};
use corral_core::{
    Store,
    agent::{AgentRole, BudgetLedger, BudgetRequest, paths},
    build_paid::BuildPaidLedger,
    connection::ConnectionBinding,
    paid_episode::PaidApproval,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Serialize)]
pub struct PaidLaunch {
    pub session: String,
    pub generation: u32,
    pub cap: String,
}
impl std::fmt::Debug for PaidLaunch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaidLaunch")
            .field("session", &self.session)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}
fn lock(store: &Store) -> Result<std::fs::File> {
    let path = store.dir().join("build-paid.lock");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    let file = options
        .open(&path)
        .context("opening persistent Build financial lock")?;
    ensure!(
        file.metadata()
            .context("reading Build lock metadata")?
            .is_file(),
        "Build financial lock is not a regular file"
    );
    fs2::FileExt::lock_exclusive(&file).context("locking Build financial ledger")?;
    Ok(file)
}
fn secret() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|e| anyhow::anyhow!("generating Build financial capability: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn approved(store: &Store, profile: &str, binding: &ConnectionBinding) -> Result<PaidApproval> {
    let config = store.config_layers()?.config()?;
    let approval = crate::availability::continuation::api_start_approval(
        &config,
        profile,
        AgentRole::Assistant,
    )?;
    ensure!(
        approval.candidates.as_slice() == std::slice::from_ref(binding),
        "PolicyNotApproved: Build requires the exact approved API candidate"
    );
    Ok(approval)
}
/// Called only after launch preflight; the capability rotates on resume, the allowance does not.
pub fn open(
    store: &Store,
    profile: &str,
    binding: &ConnectionBinding,
    session: Option<&str>,
) -> Result<PaidLaunch> {
    let approval = approved(store, profile, binding)?;
    let cap = secret()?;
    let session = match session {
        Some(id) => {
            super::validate_session_id(id)?;
            id.into()
        }
        None => format!("B-{}", &secret()?[..12]),
    };
    let _lock = lock(store)?;
    store.records_transaction(|| {
        let ledger = match store.build_paid_ledger(&session)? {
            Some(mut ledger) => {
                ensure!(
                    ledger.profile == profile && ledger.approval == approval,
                    "paid route is sticky; approval changes cannot refresh its allowance"
                );
                ledger.generation = ledger
                    .generation
                    .checked_add(1)
                    .context("Build generation exhausted")?;
                ledger.cap_sha256 = paths::sha256_hex(cap.as_bytes());
                ledger
            }
            None => BuildPaidLedger {
                version: 1,
                store: paths::store_id(store),
                session: session.clone(),
                profile: profile.into(),
                approval,
                generation: 1,
                cap_sha256: paths::sha256_hex(cap.as_bytes()),
                ledger: BudgetLedger::default(),
                bounds: vec![],
            },
        };
        // Re-read policy under the financial lock, so approval changes cannot become a new balance.
        ensure!(
            approved(store, profile, binding)? == ledger.approval,
            "paid policy changed during launch"
        );
        store.save_build_paid_ledger(&ledger)?;
        Ok(PaidLaunch {
            session,
            generation: ledger.generation,
            cap,
        })
    })
}
pub fn sticky(store: &Store, session: &str) -> Result<Option<String>> {
    super::validate_session_id(session)?;
    Ok(store.build_paid_ledger(session)?.map(|l| l.profile))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Call {
    version: u32,
    session: String,
    generation: u32,
    cap: String,
    kind: String,
    action: String,
    #[serde(default)]
    request: String,
    #[serde(default)]
    response: Option<String>,
    #[serde(default)]
    tokens: u64,
    #[serde(default)]
    usd_nanos: u64,
    #[serde(default)]
    priced: bool,
    #[serde(default)]
    unbounded: bool,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<Value>,
    #[serde(default)]
    purpose: Option<String>,
    #[serde(default)]
    cache_lineage: Option<String>,
}
/// Hidden CLI seam. Capabilities arrive on stdin, never argv/env; all writers share the same lock.
pub fn budget(store: &Store, call: Value) -> Result<Value> {
    let p: Call = serde_json::from_value(call).context("invalid Build budget request")?;
    super::validate_session_id(&p.session)?;
    ensure!(
        p.version == 1
            && p.kind == "budget"
            && p.cap.len() == 64
            && p.generation > 0
            && p.request.len() <= 128
            && p.response
                .as_ref()
                .is_none_or(|r| !r.is_empty() && r.len() <= 256)
            && p.tokens <= 9_007_199_254_740_991
            && p.usd_nanos <= 9_007_199_254_740_991,
        "invalid Build budget identity or bounds"
    );
    let _telemetry = (&p.usage, &p.purpose, &p.cache_lineage);
    let _lock = lock(store)?;
    store.records_transaction(|| {
        let mut ledger = store.build_paid_ledger(&p.session)?.context("Build allowance missing; refusing to initialize from a host request")?;
        ensure!(ledger.generation == p.generation && ledger.cap_sha256 == paths::sha256_hex(p.cap.as_bytes()), "fenced Build financial capability");
        let binding = ledger.route()?.clone();
        ensure!(approved(store, &ledger.profile, &binding)? == ledger.approval, "paid policy changed; explicit approval is required");
        let admitted = match p.action.as_str() {
            "state" => true,
            "reserve" | "settle" => {
                ensure!(!p.request.is_empty() && p.provider.as_deref() == Some(binding.connection.provider()) && p.model.as_deref() == Some(binding.model.as_str()), "Build request changed approved model/provider");
                if p.action == "reserve" {
                    ledger.reserve(BudgetRequest { connection: Some(binding.connection), request: p.request, generation: p.generation,
                        tokens: p.tokens, usd_nanos: p.usd_nanos, priced: p.priced, unbounded: p.unbounded, response: None })?
                } else {
                    ledger.settle(&p.request, p.response.context("Build settlement needs response identity")?, p.tokens, p.usd_nanos)?;
                    true
                }
            },
            _ => anyhow::bail!("unknown Build budget action"),
        };
        let (tokens, usd_nanos) = ledger.ledger.totals();
        let allowance = ledger.approval.allowance_nanos;
        let uncertain = ledger.ledger.requests.iter().filter(|r| r.response.is_none()).count();
        store.save_build_paid_ledger(&ledger)?;
        Ok(json!({"admitted":admitted,"exhausted":usd_nanos >= allowance,"guard_paused":false,
            "paid":{"episode":ledger.session,"epoch":ledger.generation,"allowance_nanos":allowance,"consumed_or_reserved_nanos":usd_nanos,"uncertain_requests":uncertain},
            "connection":binding.connection,"binding_digest":binding.digest,"limits":{"max_tokens":null,"max_usd_nanos":allowance},"tokens":tokens,"usd_nanos":usd_nanos}))
    })
}

#[cfg(test)]
#[path = "paid_tests.rs"]
mod tests;
