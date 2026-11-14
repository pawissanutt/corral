//! Monotone financial history checks at the CAS writer. No replacement/reload/backfill can
//! drop a request, revise an already-settled bill, restore an epoch or reset an allowance.
use super::*;
impl PaidEpisode {
    pub(super) fn follows(&self, before: &Self) -> Result<()> {
        before.validate()?;
        ensure!(
            self.id == before.id && self.root == before.root && self.approval == before.approval,
            "paid root/approval/allowance is immutable; a reload is not spending approval"
        );
        ensure!(
            !before.legacy_usage_gap || self.legacy_usage_gap,
            "paid legacy accounting gap cannot be erased"
        );
        ensure!(
            self.ledger.requests.len() >= before.ledger.requests.len()
                && self.owners.starts_with(&before.owners),
            "paid reservation history cannot be removed or rewritten"
        );
        for (old, new) in before.ledger.requests.iter().zip(&self.ledger.requests) {
            ensure!(
                old.request == new.request
                    && old.generation == new.generation
                    && old.connection == new.connection
                    && old.priced == new.priced
                    && old.unbounded == new.unbounded,
                "paid reservation identity changed"
            );
            if old.response.is_some() || new.response.is_none() {
                ensure!(
                    old == new,
                    "uncertain or settled paid reservation was rewritten"
                );
            } else {
                ensure!(
                    new.tokens <= old.tokens && new.usd_nanos <= old.usd_nanos,
                    "paid completion exceeds reserved bound"
                );
            }
        }
        if self.epoch == before.epoch {
            ensure!(
                self.switches == before.switches,
                "paid switch count changed outside transfer"
            );
            let activation = matches!((&before.phase, &self.phase),
                (PaidPhase::Prepared { successor, .. }, PaidPhase::Active { owner }) if successor == owner);
            ensure!(
                self.phase == before.phase || activation,
                "paid owner changed without prepare/activate"
            );
            if self.ledger != before.ledger || self.owners != before.owners {
                ensure!(
                    matches!(&self.phase, PaidPhase::Active { .. }) && self.phase == before.phase,
                    "only the active spend owner can change requests"
                );
            }
        } else {
            ensure!(
                Some(self.epoch) == before.epoch.checked_add(1)
                    && self.ledger == before.ledger
                    && self.owners == before.owners,
                "paid transfer must retain all requests and advance exactly one epoch"
            );
            let PaidPhase::Active { owner } = &before.phase else {
                anyhow::bail!("only an active paid owner can transfer/close");
            };
            match &self.phase {
                PaidPhase::Prepared { successor, carry } => {
                    ensure!(
                        carry.is_some() && successor.binding.connection.api_key(),
                        "transfer must carry existing safety history"
                    );
                    let switches = before.switches + u32::from(!owner.binding.connection.api_key());
                    ensure!(
                        self.switches == switches
                            && (owner
                                .binding
                                .connection
                                .api_key()
                                .then_some(&owner.binding)
                                .is_none_or(|b| b == &successor.binding)),
                        "paid route switch/reset is not authorized"
                    );
                }
                PaidPhase::Closed => ensure!(
                    self.switches == before.switches,
                    "closing cannot change switch count"
                ),
                PaidPhase::Active { .. } => {
                    anyhow::bail!("paid successor must be prepared before activation")
                }
            }
        }
        Ok(())
    }
}
