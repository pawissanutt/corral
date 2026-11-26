use super::*;

impl Engine {
    /// Snapshot a gate log into the content store without loading it in memory or
    /// holding SQLite's writer lock. Only the bytes present at command exit count.
    pub(crate) fn prepare_gate_log(
        &self,
        log: File,
        bytes: u64,
        mut heartbeat: impl FnMut(),
    ) -> Result<(String, u64)> {
        let blobs = self.state_dir.join("blobs/sha256");
        fs::create_dir_all(&blobs).context("create gate log blob directory")?;
        let mut draft = tempfile::NamedTempFile::new_in(blobs).context("stage gate log blob")?;
        let mut hash = Sha256::new();
        let mut reader = log.take(bytes);
        let mut buffer = [0u8; 16 * 1024];
        loop {
            let n = reader
                .read(&mut buffer)
                .context("read completed gate log")?;
            if n == 0 {
                break;
            }
            draft
                .write_all(&buffer[..n])
                .context("stage gate log bytes")?;
            hash.update(&buffer[..n]);
            heartbeat();
        }
        anyhow::ensure!(reader.limit() == 0, "completed gate log was shortened");
        let hash = format!("{:x}", hash.finalize());
        let path = self.blob_path(&hash);
        fs::create_dir_all(path.parent().context("gate blob has no parent")?)
            .context("create gate log blob shard")?;
        match draft.persist_noclobber(path) {
            Ok(_) => (),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error.error).context("publish gate log blob"),
        }
        Ok((hash, bytes))
    }

    pub(crate) fn add_gate_log_in(
        &self,
        store: &Store,
        target: ArtifactTarget<'_>,
        blob: (String, u64),
        title: &str,
    ) -> Result<Artifact> {
        self.record_artifact_in(store, target, blob, None, ArtifactKind::Gate, title)
    }
}
