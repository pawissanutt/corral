//! Assistant sends share the device command namespace and admission budget. No second
//! cancellation index exists: durable abandoned entries prevent a late send or action.
use super::*;
use corral_protocol::assistant::AssistantSend;

const SEND_PREFIX: &str = "assistant.send.v1:";

fn key(device: &str, cmd_id: &str) -> app::AppResult<String> {
    if cmd_id.is_empty()
        || cmd_id.len() > 128
        || !cmd_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
    {
        return Err((ErrorCode::InvalidParams, "invalid_cmd_id".into()));
    }
    Ok(format!("dev-{device}/{cmd_id}"))
}

fn unavailable(problem: anyhow::Error) -> (ErrorCode, String) {
    eprintln!("corrald assistant journal: {problem:#}");
    (ErrorCode::Unavailable, "journal_unavailable".into())
}

/// Called only after checking the canonical send key. A reservation may survive a
/// failed send; only its exact payload can retry, never an unrelated device action.
pub(crate) fn reserve_send(
    shared: &Shared,
    device: &str,
    request: &AssistantSend,
) -> app::AppResult<()> {
    let key = key(device, &request.cmd_id)?;
    let digest = corral_core::agent::paths::sha256_hex(
        &serde_json::to_vec(request).map_err(|_| (ErrorCode::Internal, "encode_failed".into()))?,
    );
    let digest = format!("{SEND_PREFIX}{digest}");
    let service = service(shared).ok_or((ErrorCode::Unavailable, "unavailable".into()))?;
    let mut index = lock(&service.index);
    let store = lock(&service.journal);
    if let Some(journal) = store
        .record::<Journal>(COMMANDS_AREA, &key)
        .map_err(unavailable)?
    {
        return if journal.state == JournalState::Abandoned {
            Err((ErrorCode::Refused, "cancelled".into()))
        } else if journal.digest == digest {
            Ok(())
        } else if journal.digest.starts_with(SEND_PREFIX) {
            Err((ErrorCode::Conflict, "payload_mismatch".into()))
        } else {
            Err((ErrorCode::Conflict, "cmd_id_in_use".into()))
        };
    }
    service.evict(&store, &key, &mut index);
    if index.full(&key) {
        return Err((ErrorCode::Busy, "queue_full".into()));
    }
    let now = corral_core::clock::now().to_string();
    let journal = Journal {
        digest,
        state: JournalState::Reserved,
        issued_at: now.clone(),
        expires_at: now,
        result: None,
        retain_until: app::retention(&key),
    };
    if !store
        .insert_record(COMMANDS_AREA, &key, &journal)
        .map_err(unavailable)?
    {
        return Err((ErrorCode::Conflict, "cmd_id_in_use".into()));
    }
    index.done(&key, journal::eviction_time(&key, &journal));
    Ok(())
}

/// The caller's serialized input worker has already checked that no accepted send
/// exists. Only our own unused reservation can be changed; real commands conflict.
pub(crate) fn cancel(shared: &Shared, device: &str, cmd_id: &str) -> app::AppResult<()> {
    let key = key(device, cmd_id)?;
    let service = service(shared).ok_or((ErrorCode::Unavailable, "unavailable".into()))?;
    {
        let mut index = lock(&service.index);
        let store = lock(&service.journal);
        if let Some(mut journal) = store
            .record::<Journal>(COMMANDS_AREA, &key)
            .map_err(unavailable)?
        {
            if journal.state == JournalState::Abandoned {
                return Ok(());
            }
            if !journal.digest.starts_with(SEND_PREFIX) {
                return Err((ErrorCode::Conflict, "cmd_id_in_use".into()));
            }
            journal.digest.clear();
            journal.state = JournalState::Abandoned;
            journal.result = None;
            journal.retain_until = app::retention(&key);
            store
                .save_record(COMMANDS_AREA, &key, &journal)
                .map_err(unavailable)?;
            index.done(&key, journal::eviction_time(&key, &journal));
            return Ok(());
        }
    }
    // query owns atomic admission, bounded expiry eviction and tombstone retention.
    // A real action may win the race between the read above and this admission.
    let answer = service
        .query(&key, Instant::now(), true)
        .map_err(|problem| {
            let (code, message) = problem.wire();
            (code, message.into())
        })?;
    if answer.state == CmdState::Abandoned {
        Ok(())
    } else {
        Err((ErrorCode::Conflict, "cmd_id_in_use".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_protocol::assistant::AskTarget;

    #[test]
    fn cancellations_share_bounded_device_admission_and_preserve_real_commands() {
        let fixture = crate::tests::Fixture::new();
        let shared = Arc::new(Shared::new());
        let (jobs, _) = mpsc::sync_channel(1);
        let service = Arc::new(Service {
            jobs,
            flights: Mutex::default(),
            index: Mutex::new(Index::with_limit(2)),
            journal: Mutex::new(Store::open_unobserved(fixture.ctx.store.root()).unwrap()),
            previews: Mutex::default(),
        });
        *lock(&shared.commands) = Some(Arc::clone(&service));
        let request = AssistantSend {
            cmd_id: "send".into(),
            thread: "quick".into(),
            target: AskTarget::Assistant,
            text: "typed".into(),
            typed_at: corral_core::clock::now().to_string(),
            context: Vec::new(),
            attachments: Vec::new(),
        };
        reserve_send(&shared, "one", &request).unwrap();
        let real_key = key("one", "real").unwrap();
        let now = corral_core::clock::now().to_string();
        let real = Journal {
            digest: "real-command-digest".into(),
            state: JournalState::Reserved,
            issued_at: now.clone(),
            expires_at: now,
            result: None,
            retain_until: app::retention(&real_key),
        };
        fixture
            .ctx
            .store
            .save_record(COMMANDS_AREA, &real_key, &real)
            .unwrap();
        lock(&service.index).done(&real_key, journal::eviction_time(&real_key, &real));
        assert_eq!(
            cancel(&shared, "one", "fresh").unwrap_err().0,
            ErrorCode::Busy
        );
        assert!(
            fixture
                .ctx
                .store
                .record::<Journal>(COMMANDS_AREA, &key("one", "fresh").unwrap())
                .unwrap()
                .is_none()
        );
        assert_eq!(
            cancel(&shared, "one", "real").unwrap_err(),
            (ErrorCode::Conflict, "cmd_id_in_use".into())
        );
        cancel(&shared, "one", "send").unwrap();
        cancel(&shared, "one", "send").unwrap();
        assert_eq!(
            reserve_send(&shared, "one", &request).unwrap_err(),
            (ErrorCode::Refused, "cancelled".into())
        );
        reserve_send(&shared, "two", &request).unwrap();
        // Two threads contend for the last slot of a different device, not a global budget.
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let attempts: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|id| {
                let shared = Arc::clone(&shared);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    cancel(&shared, "two", id)
                })
            })
            .collect();
        barrier.wait();
        let outcomes: Vec<_> = attempts
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            outcomes
                .iter()
                .filter(|result| matches!(result, Err((ErrorCode::Busy, _))))
                .count(),
            1
        );
        let preserved = fixture
            .ctx
            .store
            .record::<Journal>(COMMANDS_AREA, &real_key)
            .unwrap()
            .unwrap();
        assert_eq!(preserved.digest, real.digest);
        assert_eq!(preserved.state, JournalState::Reserved);
        shared.close();
    }
}
