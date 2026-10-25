//! Statements whose access path is part of the bounded-history contract, kept here so a test
//! can check their query plans: each must reach rows by a full primary key, never by a scan of
//! the task's family.

/// A row's own index tokens, read by the row's primary key.
pub(super) const ROW_TOKENS: &str =
    "SELECT idx FROM task_rows WHERE task = ?1 AND part = ?2 AND key = ?3";

/// One index token, deleted by the token table's full primary key.
pub(super) const DELETE_TOKEN: &str =
    "DELETE FROM task_row_index WHERE task = ?1 AND part = ?2 AND token = ?3 AND key = ?4";

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;

    fn plan(conn: &Connection, sql: &str) -> String {
        let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let nulls = vec![rusqlite::types::Null; stmt.parameter_count()];
        let rows = stmt
            .query_map(rusqlite::params_from_iter(nulls), |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows.join("\n")
    }

    #[test]
    fn index_maintenance_reaches_rows_by_their_full_primary_key() {
        let mut conn = Connection::open_in_memory().unwrap();
        super::super::schema::init(&mut conn, "plan-test").unwrap();
        let tokens = plan(&conn, ROW_TOKENS);
        assert!(
            tokens
                .contains("USING INDEX sqlite_autoindex_task_rows_1 (task=? AND part=? AND key=?)"),
            "{tokens}"
        );
        let delete = plan(&conn, DELETE_TOKEN);
        assert!(
            delete.contains(
                "USING PRIMARY KEY (task=? AND part=? AND token=? AND key=?)"
            ) || delete.contains(
                "USING INDEX sqlite_autoindex_task_row_index_1 (task=? AND part=? AND token=? AND key=?)"
            ),
            "{delete}"
        );
    }

    #[test]
    fn event_pages_seek_sequence_task_kind_and_run_indexes() {
        let mut conn = Connection::open_in_memory().unwrap();
        super::super::schema::init(&mut conn, "event-plan-test").unwrap();
        for (query, access) in [
            (
                "SELECT detail FROM events WHERE seq > ?1 ORDER BY seq LIMIT 501",
                "USING INTEGER PRIMARY KEY (rowid>?)",
            ),
            (
                "SELECT detail FROM events WHERE task = ?1 AND seq > ?2 AND malformed = 0 ORDER BY seq LIMIT 501",
                "USING INDEX events_task (task=? AND seq>?)",
            ),
            (
                "SELECT detail FROM events WHERE task = ?1 AND kind = ?2 AND seq > ?3 AND malformed = 0 ORDER BY seq LIMIT 501",
                "USING INDEX events_task_kind (task=? AND kind=? AND seq>?)",
            ),
            (
                "SELECT detail FROM events WHERE task = ?1 AND kind = ?2 AND run = ?3 AND seq > ?4 AND malformed = 0 ORDER BY seq LIMIT 501",
                "USING INDEX events_task_kind_run (task=? AND kind=? AND run=? AND seq>?)",
            ),
        ] {
            let actual = plan(&conn, query);
            assert!(actual.contains(access), "{actual}");
            assert!(
                !actual.contains("SCAN ") && !actual.contains("TEMP B-TREE"),
                "{actual}"
            );
        }
    }
}
