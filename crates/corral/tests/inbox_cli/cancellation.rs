use std::cell::{Cell, RefCell};
use std::io::{BufRead, BufReader};
use std::process::Child;

use anyhow::Context;

use super::*;

fn publish_unattempted(inbox: &Inbox<'_>) {
    let _deliver = inbox.delivery_lock().unwrap();
    inbox
        .publish(
            &SessionBinding {
                session: "fixture".into(),
                endpoint: None,
                transcript: None,
            },
            "fixture",
            MAX_PAYLOAD,
            false,
        )
        .unwrap()
        .unwrap();
    assert!(inbox.envelope(1).unwrap().attempts.is_empty());
}

struct CancelDuringBind<'a> {
    root: &'a Path,
    message: &'a str,
    cancellation: RefCell<Option<Child>>,
    cancelled_before_send: Cell<bool>,
    sends: Cell<usize>,
}

impl DeliveryAdapter for CancelDuringBind<'_> {
    fn harness(&self) -> &'static str {
        "fixture"
    }

    fn capabilities(&self) -> Capabilities {
        LateUncertainty.capabilities()
    }

    fn readiness(&self, ctx: &Ctx, recipient: &RecipientRef) -> anyhow::Result<Readiness> {
        LateUncertainty.readiness(ctx, recipient)
    }

    fn bind(&self, ctx: &Ctx, recipient: &RecipientRef) -> anyhow::Result<BindOutcome> {
        // Delivery has passed its settled check and owns the first-send boundary.
        let mut child = Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inbox_cli/cancellation_sync.py"))
            .arg(env!("CARGO_BIN_EXE_corral"))
            .arg(self.root)
            .arg(self.message)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("starting synchronized owner cancellation")?;
        let stdout = child.stdout.take().context("cancellation sync output")?;
        *self.cancellation.borrow_mut() = Some(child);
        let mut signal = String::new();
        BufReader::new(stdout)
            .read_line(&mut signal)
            .context("reading cancellation lock boundary")?;
        let signal: Value =
            serde_json::from_str(&signal).context("decoding cancellation lock boundary")?;
        self.cancelled_before_send.set(
            signal["cancelled_before_send"]
                .as_bool()
                .context("cancellation boundary flag")?,
        );
        LateUncertainty.bind(ctx, recipient)
    }

    fn send(&self, _: &Ctx, _: &SessionBinding, _: &corral_core::inbox::Envelope) -> SendOutcome {
        self.sends.set(self.sends.get() + 1);
        SendOutcome::Accepted
    }

    fn observe(
        &self,
        ctx: &Ctx,
        binding: &SessionBinding,
        cursor: &mut corral_core::inbox::ObserveCursor,
    ) -> anyhow::Result<Vec<Observation>> {
        LateUncertainty.observe(ctx, binding, cursor)
    }
}

#[test]
fn cancellation_winning_before_recovered_first_attempt_prevents_send() {
    // Given a persisted publication recovered before its first attempt.
    let dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(dir.path()).unwrap());
    let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
    let message = post(&inbox, "cancel before recovery");
    publish_unattempted(&inbox);

    // When the real owner command wins before delivery acquires its boundary.
    ok(
        dir.path(),
        &["mark", &message.id, "--cancel", "--reason", "withdrawn"],
    );
    let outcome = deliver(&ctx, &Recipient::Pm, &LateUncertainty, None).unwrap();

    // Then cancellation settles the publication without any external attempt.
    assert_eq!(outcome, DeliveryStatus::Idle);
    assert!(inbox.envelope(1).unwrap().attempts.is_empty());
    assert_eq!(
        inbox.terminal(message.seq).unwrap(),
        Some(TerminalState::Cancelled)
    );
}

#[test]
fn cancellation_waits_when_delivery_owns_recovered_first_send_boundary() {
    // Given a recovered publication, with delivery paused at its binding hook.
    let dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(dir.path()).unwrap());
    let inbox = Inbox::new(&ctx.store, Recipient::Pm).unwrap();
    let message = post(&inbox, "cancel during recovery");
    publish_unattempted(&inbox);
    let adapter = CancelDuringBind {
        root: dir.path(),
        message: &message.id,
        cancellation: RefCell::new(None),
        cancelled_before_send: Cell::new(false),
        sends: Cell::new(0),
    };

    // When the actual CLI reaches its lock syscall before the first send.
    let outcome = deliver(&ctx, &Recipient::Pm, &adapter, None);
    let cancellation = adapter
        .cancellation
        .borrow_mut()
        .take()
        .unwrap()
        .wait_with_output()
        .unwrap();

    // Then cancellation cannot commit before delivery releases its boundary.
    assert!(cancellation.status.success(), "{cancellation:?}");
    assert_eq!(outcome.unwrap(), DeliveryStatus::Sent { envelope: 1 });
    assert_eq!(adapter.sends.get(), 1);
    assert!(
        !adapter.cancelled_before_send.get(),
        "owner cancellation completed before a never-attempted publication was sent"
    );
    assert_eq!(inbox.envelope(1).unwrap().attempts.len(), 1);
    assert_eq!(
        inbox.terminal(message.seq).unwrap(),
        Some(TerminalState::Cancelled)
    );
}
