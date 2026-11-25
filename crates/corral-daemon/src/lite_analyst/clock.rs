//! Same-boot UTC mapping for deadlines, budget cutoff and UTC-day accounting.
use anyhow::{Context, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use rustix::time::{DynamicClockId, clock_gettime_dynamic};
use std::{
    fs::File,
    io::Read as _,
    time::{SystemTime, UNIX_EPOCH},
};

use super::wire::MAX_SAFE;
pub(super) const DAY_MS: u64 = 86_400_000;
const UNCERTAINTY_MS: u64 = 100;

pub(super) struct Clock {
    wall_ms: u64,
    boot_ms: u64,
}

impl Clock {
    pub(super) fn load(db: &Connection) -> anyhow::Result<Self> {
        let mut id = String::new();
        File::open("/proc/sys/kernel/random/boot_id")?
            .take(128)
            .read_to_string(&mut id)?;
        let id = id.trim();
        ensure!(
            id.len() == 36 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'),
            "fixture boot identity unavailable"
        );
        let before = boot_ms()?;
        let wall = wall_ms()?;
        let after = boot_ms()?;
        ensure!(
            after.saturating_sub(before) <= 10,
            "fixture clock sample interrupted"
        );
        Self::load_at(db, id, wall, after)
    }

    pub(super) fn load_at(
        db: &Connection,
        id: &str,
        wall_ms: u64,
        boot_ms: u64,
    ) -> anyhow::Result<Self> {
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS clock_anchor(
            singleton INTEGER PRIMARY KEY CHECK(singleton=1),
            boot_id TEXT NOT NULL, wall_ms INTEGER NOT NULL, boot_ms INTEGER NOT NULL
        ) STRICT;",
        )?;
        let saved: Option<(String, u64, u64)> = db
            .query_row(
                "SELECT boot_id, wall_ms, boot_ms FROM clock_anchor WHERE singleton=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if let Some((old_id, old_wall, old_boot)) = saved {
            ensure!(old_id == id, "fixture ledger cannot move to another boot");
            let clock = Self {
                wall_ms: old_wall,
                boot_ms: old_boot,
            };
            clock.bounds(wall_ms, boot_ms)?;
            Ok(clock)
        } else {
            let clock = Self { wall_ms, boot_ms };
            clock.bounds(wall_ms, boot_ms)?;
            db.execute(
                "INSERT INTO clock_anchor VALUES(1,?1,?2,?3)",
                params![id, wall_ms, boot_ms],
            )?;
            Ok(clock)
        }
    }

    #[cfg(test)]
    pub(super) fn new() -> anyhow::Result<Self> {
        Ok(Self {
            wall_ms: wall_ms()?,
            boot_ms: boot_ms()?,
        })
    }

    fn bounds(&self, wall: u64, boot: u64) -> anyhow::Result<(u64, u64)> {
        let projected = boot
            .checked_sub(self.boot_ms)
            .and_then(|elapsed| self.wall_ms.checked_add(elapsed))
            .context("fixture boot clock moved backwards or out of range")?;
        ensure!(
            projected.abs_diff(wall) <= UNCERTAINTY_MS && projected <= MAX_SAFE - UNCERTAINTY_MS,
            "fixture clock outside safe bounds"
        );
        Ok((
            projected.saturating_sub(UNCERTAINTY_MS),
            projected + UNCERTAINTY_MS,
        ))
    }

    /// Use the conservative upper bound: never extend a deadline or credit a hold late.
    pub(super) fn now(&self) -> anyhow::Result<u64> {
        self.times().map(|(_, upper)| upper)
    }

    pub(super) fn times(&self) -> anyhow::Result<(u64, u64)> {
        self.bounds(wall_ms()?, boot_ms()?)
    }

    pub(super) fn reserve_time(&self) -> anyhow::Result<u64> {
        self.reserve_time_at(wall_ms()?, boot_ms()?)
    }

    fn reserve_time_at(&self, wall: u64, boot: u64) -> anyhow::Result<u64> {
        let (lower, upper) = self.bounds(wall, boot)?;
        ensure!(
            lower / DAY_MS == upper / DAY_MS,
            "fixture UTC day uncertain"
        );
        Ok(upper)
    }
}

pub(super) fn wall_ms() -> anyhow::Result<u64> {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("fixture clock before epoch")?
            .as_millis(),
    )
    .context("fixture clock exceeds range")
}

fn boot_ms() -> anyhow::Result<u64> {
    let ts = clock_gettime_dynamic(DynamicClockId::Boottime).context("read fixture boot clock")?;
    u64::try_from(ts.tv_sec)
        .ok()
        .and_then(|secs| secs.checked_mul(1000))
        .and_then(|millis| millis.checked_add((ts.tv_nsec as u64) / 1_000_000))
        .context("invalid fixture boot time")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reexec_preserves_time_and_refuses_ambiguous_utc_buckets() {
        let db = Connection::open_in_memory().unwrap();
        let live = Clock::load_at(&db, "same boot", DAY_MS - 200, 1_000).unwrap();
        assert_eq!(
            live.reserve_time_at(DAY_MS - 200, 1_000).unwrap(),
            DAY_MS - 100
        );
        assert!(live.reserve_time_at(DAY_MS - 100, 1_100).is_err());
        let old = live.bounds(DAY_MS - 5, 1_150).unwrap().1;
        let reopened = Clock::load_at(&db, "same boot", DAY_MS - 90, 1_151).unwrap();
        assert!(reopened.bounds(DAY_MS - 90, 1_151).unwrap().1 > old);
        assert!(Clock::load_at(&db, "same boot", DAY_MS - 200, 1_400).is_err());
        assert!(Clock::load_at(&db, "new boot", DAY_MS + 200, 1_400).is_err());
        assert_eq!(
            reopened.reserve_time_at(DAY_MS + 200, 1_400).unwrap(),
            DAY_MS + 300
        );
    }
}
