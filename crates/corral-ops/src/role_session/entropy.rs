//! Fixture input control, not output masking: production always uses OS entropy.
use std::{cell::RefCell, marker::PhantomData, rc::Rc};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

thread_local! {
    static FIXTURE: RefCell<Option<(u64, u64)>> = const { RefCell::new(None) };
}

/// Thread-local fixture input, restored on drop and deliberately not transferable.
#[doc(hidden)]
pub struct FixtureRoleEntropyGuard {
    previous: Option<(u64, u64)>,
    _thread: PhantomData<Rc<()>>,
}

/// Pin the entropy INPUT of session allocation in a disposable fixture. Distinct seeds
/// separate fixture construction from measured actions. Nonces within a scope stay distinct;
/// all session fields and entire emitted documents remain compared without extra masks.
#[doc(hidden)]
pub fn fixture_role_entropy(seed: u64) -> FixtureRoleEntropyGuard {
    let previous = FIXTURE.with(|state| state.replace(Some((seed, 0))));
    FixtureRoleEntropyGuard {
        previous,
        _thread: PhantomData,
    }
}

impl Drop for FixtureRoleEntropyGuard {
    fn drop(&mut self) {
        FIXTURE.with(|state| state.replace(self.previous));
    }
}

pub(super) fn nonce() -> Result<[u8; 16]> {
    let fixed = FIXTURE.with(|state| {
        let mut state = state.borrow_mut();
        let Some((seed, ordinal)) = state.as_mut() else {
            return Ok(None);
        };
        let mut hash = Sha256::new();
        hash.update(b"corral disposable role-session fixture\0");
        hash.update(seed.to_le_bytes());
        hash.update(ordinal.to_le_bytes());
        *ordinal = ordinal
            .checked_add(1)
            .context("fixture entropy exhausted")?;
        let mut bytes = [0; 16];
        bytes.copy_from_slice(&hash.finalize()[..16]);
        Ok::<_, anyhow::Error>(Some(bytes))
    })?;
    if let Some(bytes) = fixed {
        return Ok(bytes);
    }
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| anyhow::anyhow!("allocating role session identity: {error}"))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_inputs_replay_unique_streams_and_restore_nested_or_os_sources() {
        let production = nonce().unwrap();
        assert_ne!(production, nonce().unwrap());
        let expected;
        {
            let _scope = fixture_role_entropy(1);
            expected = [nonce().unwrap(), nonce().unwrap(), nonce().unwrap()];
            assert_ne!(expected[0], expected[1]);
        }
        {
            let _scope = fixture_role_entropy(1);
            assert_eq!(nonce().unwrap(), expected[0]);
            {
                let _inner = fixture_role_entropy(2);
                assert_ne!(nonce().unwrap(), expected[0]);
            }
            assert_eq!(nonce().unwrap(), expected[1]);
            assert_eq!(nonce().unwrap(), expected[2]);
        }
        assert_ne!(nonce().unwrap(), expected[0]);
    }
}
