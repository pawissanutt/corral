//! Serialize preparation of a keyed executor authorization, not the board's history.
use anyhow::Result;

use crate::Store;
use crate::store::StoreLock;

impl Store {
    /// Hold through reading, preparing and publishing one executor claim.
    ///
    /// The lease prevents competing owner approvals from reaching authorization
    /// preparation together; record version preconditions still protect publication.
    pub fn operating_claim_lock(&self) -> Result<StoreLock> {
        self.lock("operating-claims")
    }
}
