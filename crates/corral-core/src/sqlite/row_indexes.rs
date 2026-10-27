//! Exact row-token cleanup, including the run/path, run/title and run/kind artifact indexes.
use rusqlite::OptionalExtension;

use super::codec::OrIo;
use super::tx::{Applied, Tx};

impl Tx<'_> {
    /// Deletes a row's index tokens by their full primary key, found in the row's own `idx`.
    pub(super) fn delete_index_tokens(&self, task: i64, part: &str, key: &str) -> Applied<()> {
        let stored: Option<String> = self
            .db
            .query_row(
                super::plans::ROW_TOKENS,
                rusqlite::params![task, part, key],
                |row| row.get(0),
            )
            .optional()?;
        let tokens: Vec<String> = match stored {
            Some(idx) => serde_json::from_str(&idx)
                .map_err(anyhow::Error::from)
                .or_io()?,
            None => Vec::new(),
        };
        for token in tokens {
            self.db.execute(
                super::plans::DELETE_TOKEN,
                rusqlite::params![task, part, token, key],
            )?;
        }
        Ok(())
    }
}
