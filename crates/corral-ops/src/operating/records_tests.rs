use super::*;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

fn fixture() -> (tempfile::TempDir, Ctx, Task) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("app/.git")).unwrap();
    let store = Store::init(root.path()).unwrap();
    let task = store
        .create(
            "Operating records",
            Some("app"),
            corral_core::Status::Ready,
            "Spec",
        )
        .unwrap();
    (root, Ctx::from_store(store), task)
}

#[test]
fn claim_reuses_owner_authority_and_refuses_automatic_conversion() {
    // Given a prepared owner action stored in the keyed records area.
    let (_root, ctx, task) = fixture();
    let action = identity(&task.id, "R1", OperatingActionKind::Dispatch, "launch");
    let owner = authorize(&ctx, action.clone(), Origin::Owner).unwrap();
    let key = claim_key(&task.id, "R1", action.kind, "launch").unwrap();
    let before = ctx.store.record_row(CLAIMS, &key).unwrap().unwrap();

    // When the action is resumed by the owner or requested automatically.
    assert_eq!(
        authorize(&ctx, action.clone(), Origin::Owner).unwrap(),
        owner
    );
    assert!(authorize(&ctx, action.clone(), Origin::Automatic).is_err());

    // Then the exact owner provenance survives, without rewriting the claim.
    assert_eq!(
        load(&ctx, &task.id, "R1", action.kind, "launch").unwrap(),
        owner
    );
    assert_eq!(ctx.store.record_row(CLAIMS, &key).unwrap().unwrap(), before);
    assert!(!ctx.store.state_dir().join(CLAIMS).exists());
}

#[test]
fn start_references_are_snapshot_consistent_and_direct_starts_are_new_actions() {
    // Given a direct owner start and a snapshot containing its reference.
    let (_root, ctx, task) = fixture();
    let action = start_identity(&task, false);
    let first = authorize_start(&ctx, action.clone(), Origin::Owner, false).unwrap();
    let snapshot = Ctx::from_store(ctx.store.read_only_snapshot().unwrap());

    // When another explicit direct start replaces that historical reference.
    let second = authorize_start(&ctx, action.clone(), Origin::Owner, false).unwrap();

    // Then fresh readers see the new action while the snapshot retains the old one.
    assert_ne!(first.id, second.id);
    assert_eq!(load_start(&ctx, &action).unwrap(), Some(second));
    assert_eq!(load_start(&snapshot, &action).unwrap(), Some(first));
    assert!(!ctx.store.state_dir().join("operating-starts").exists());
}

#[test]
fn missing_or_mismatched_start_reference_never_grants_authority() {
    // Given no start reference, followed by an owner reference for one exact round.
    let (_root, ctx, task) = fixture();
    let action = start_identity(&task, false);
    assert!(load_start(&ctx, &action).unwrap().is_none());
    authorize_start(&ctx, action.clone(), Origin::Owner, false).unwrap();

    // When the caller changes that round or requests automatic reuse without a grant.
    let mut changed = action.clone();
    changed.round = Some(1);

    // Then neither identity mismatch nor historical owner intent grants authority.
    assert!(load_start(&ctx, &changed).is_err());
    assert!(authorize_start(&ctx, action, Origin::Automatic, true).is_err());
}

struct Processes(Vec<Child>);

impl Drop for Processes {
    fn drop(&mut self) {
        for child in &mut self.0 {
            if child.try_wait().unwrap().is_none() {
                child.kill().unwrap();
            }
            child.wait().unwrap();
        }
    }
}

const ROUNDS: usize = 1_000;
const WORKERS: usize = 8;
const CHILD_ROOT: &str = "CORRAL_OPERATING_CLAIM_TEST_ROOT";
const CHILD_ADDR: &str = "CORRAL_OPERATING_CLAIM_TEST_ADDR";

fn channel(stream: TcpStream) -> BufReader<TcpStream> {
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    BufReader::new(stream)
}

fn receive(peer: &mut BufReader<TcpStream>) -> String {
    let mut line = String::new();
    assert_ne!(peer.read_line(&mut line).unwrap(), 0, "claim worker exited");
    line
}

#[test]
fn claim_race_process() {
    let Some(root) = std::env::var_os(CHILD_ROOT) else {
        return;
    };
    // Given an independent Store and a socket registered before every claim.
    let store = Store::open_unobserved(std::path::Path::new(&root)).unwrap();
    let ctx = Ctx::from_store(store);
    let mut peer = channel(TcpStream::connect(std::env::var(CHILD_ADDR).unwrap()).unwrap());
    peer.get_mut().write_all(b"ready\n").unwrap();
    for round in 0..ROUNDS {
        assert_eq!(receive(&mut peer), "go\n");
        let run = format!("R{}", round + 1);
        let key = claim_key("T-001", &run, OperatingActionKind::Dispatch, "cas").unwrap();
        let proposed = std::process::id().to_string();
        // When eight processes race the same absent row without holding the process lease.
        let inserted = ctx.store.insert_record(CLAIMS, &key, &proposed).unwrap();
        let winner = ctx.store.record::<String>(CLAIMS, &key).unwrap().unwrap();
        // Also exercise the real executor path, including owner provenance and its leases.
        let action = identity("T-001", &run, OperatingActionKind::Dispatch, "launch");
        let authorization = authorize(&ctx, action.clone(), Origin::Owner).unwrap();
        assert_eq!(
            load(&ctx, "T-001", &run, action.kind, "launch").unwrap(),
            authorization
        );
        let key = claim_key("T-001", &run, action.kind, "launch").unwrap();
        assert_eq!(
            ctx.store.record_row(CLAIMS, &key).unwrap().unwrap().version,
            1
        );
        let reply = serde_json::to_string(&(inserted, winner, authorization)).unwrap();
        writeln!(peer.get_mut(), "{reply}").unwrap();
    }
}

#[test]
fn one_winner_per_thousand_claims_raced_by_eight_independent_processes() {
    // Given eight independent Store openers blocked on a per-round socket barrier.
    let (_root, ctx, task) = fixture();
    assert_eq!(task.id, "T-001");
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let mut children = Processes(Vec::new());
    for _ in 0..WORKERS {
        children.0.push(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "operating::records_tests::claim_race_process",
                    "--nocapture",
                ])
                .env(CHILD_ROOT, ctx.store.root())
                .env(CHILD_ADDR, listener.local_addr().unwrap().to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        );
    }
    let mut peers = Vec::new();
    for _ in 0..WORKERS {
        #[cfg(unix)]
        {
            use rustix::event::{PollFd, PollFlags, Timespec, poll};
            let mut ready = [PollFd::new(&listener, PollFlags::IN)];
            let timeout = Timespec {
                tv_sec: 60,
                tv_nsec: 0,
            };
            assert_eq!(
                poll(&mut ready, Some(&timeout)).unwrap(),
                1,
                "claim worker did not connect"
            );
        }
        let mut peer = channel(listener.accept().unwrap().0);
        assert_eq!(receive(&mut peer), "ready\n");
        peers.push(peer);
    }

    // When all processes contend for each of 1,000 distinct claims.
    for round in 0..ROUNDS {
        for peer in &mut peers {
            peer.get_mut().write_all(b"go\n").unwrap();
        }
        let replies = peers
            .iter_mut()
            .map(|peer| {
                serde_json::from_str::<(bool, String, ActionAuthorization)>(&receive(peer)).unwrap()
            })
            .collect::<Vec<_>>();

        // Then exactly one insert wins, losers read that row, and owner provenance is shared.
        assert_eq!(
            replies.iter().filter(|reply| reply.0).count(),
            1,
            "round {round}"
        );
        for reply in &replies {
            assert_eq!(reply.1, replies[0].1, "round {round}");
            assert_eq!(reply.2, replies[0].2, "round {round}");
        }
    }
    for child in &mut children.0 {
        assert!(child.wait().unwrap().success());
    }
    assert_eq!(ctx.store.record_rows(CLAIMS).unwrap().len(), ROUNDS * 2);
}
