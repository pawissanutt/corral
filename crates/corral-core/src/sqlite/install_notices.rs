//! One-time upgrade of pre-index install notices. Imports derive the same keyed records
//! while reading legacy events; hot landing actions only read/insert a single record.
use anyhow::{Context, Result};
use rusqlite::{Connection, TransactionBehavior};

pub(super) fn init(db: &mut Connection) -> Result<()> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("begin install notice index initialization")?;
    let ready: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key='install_notice_metadata' AND value='1')",
            [],
            |row| row.get(0),
        )
        .context("check install notice index version")?;
    if ready {
        return Ok(());
    }
    tx.execute(
        "INSERT OR IGNORE INTO records(area,key,version,doc,updated)
         SELECT 'install-notices',json_extract(detail,'$.operation'),1,'true',ts
         FROM events WHERE kind='install_due' AND malformed=0
           AND json_type(detail,'$.operation')='text'
           AND json_extract(detail,'$.operation')<>'' ORDER BY seq",
        [],
    )
    .context("backfill install notice records")?;
    tx.execute(
        "INSERT INTO meta(key,value) VALUES('install_notice_metadata','1')
         ON CONFLICT(key) DO UPDATE SET value='1'",
        [],
    )
    .context("record install notice index version")?;
    tx.commit()
        .context("commit install notice index initialization")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrade_backfills_once_without_changing_exported_events() {
        let mut db = Connection::open_in_memory().unwrap();
        super::super::schema::init(&mut db, "install-notice-upgrade").unwrap();
        db.execute_batch(
            "INSERT INTO events(seq,ts,kind,detail) VALUES
            (1,'2026-10-30T00:00:00Z','install_due','{\"operation\":\"one\"}'),
            (2,'2026-10-30T00:00:01Z','install_due','{\"operation\":\"one\"}'),
            (3,'2026-10-30T00:00:02Z','other','{\"operation\":\"two\"}'),
            (4,'2026-10-30T00:00:03Z','install_due','{\"operation\":\"\"}');",
        )
        .unwrap();
        let before: String = db
            .query_row(
                "SELECT json_group_array(json_object('seq',seq,'detail',detail)) FROM events",
                [],
                |r| r.get(0),
            )
            .unwrap();
        init(&mut db).unwrap();
        let rows: i64 = db.query_row("SELECT count(*) FROM records WHERE area='install-notices' AND key='one' AND doc='true'", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 1);
        let after: String = db
            .query_row(
                "SELECT json_group_array(json_object('seq',seq,'detail',detail)) FROM events",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(after, before);
        // The version guard, not a historical rescan, serves subsequent opens.
        db.execute("DELETE FROM records WHERE area='install-notices'", [])
            .unwrap();
        init(&mut db).unwrap();
        let rows: i64 = db
            .query_row(
                "SELECT count(*) FROM records WHERE area='install-notices'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 0);
    }
}
