use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use super::*;

const ROOT_ENV: &str = "CORRAL_INBOX_TEST_PROCESS_ROOT";
const READY: &str = "CORRAL_INBOX_POSTER_READY";

struct Poster(Child);

impl Drop for Poster {
    fn drop(&mut self) {
        // A failed parent assertion must not leave a worker waiting for stdin.
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
        }
        self.0.wait().unwrap();
    }
}

#[test]
fn subprocess_poster() {
    // Given a dedicated child invocation, never the ordinary test-suite run.
    let Some(root) = std::env::var_os(ROOT_ENV) else {
        return;
    };
    let store = Store::open(std::path::Path::new(&root)).unwrap();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    println!("{READY}");
    std::io::stdout().flush().unwrap();
    let mut token = [0_u8; 1];

    // When the parent releases all ready processes.
    std::io::stdin().read_exact(&mut token).unwrap();
    assert_eq!(token, [1]);
    for _ in 0..4 {
        inbox.post(post("process poster", Urgency::Wake)).unwrap();
    }

    // Then this process's independent store handle completed all durable writes.
    assert!(inbox.state().unwrap().next_seq >= 5);
}

#[test]
fn independent_process_posters_preserve_sequences_and_persistent_state() {
    // Given an initialized fixture and four separately executed copies of this test binary.
    let (root, store) = fixture();
    let executable = std::env::current_exe().unwrap();
    let (ready_tx, ready_rx) = mpsc::sync_channel(4);
    thread::scope(|scope| {
        let mut posters = Vec::new();
        for _ in 0..4 {
            let mut child = Poster(
                Command::new(&executable)
                    .args([
                        "--exact",
                        "inbox::tests::process::subprocess_poster",
                        "--nocapture",
                    ])
                    .env(ROOT_ENV, root.path())
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .unwrap(),
            );
            let stdout = child.0.stdout.take().unwrap();
            let ready_tx = ready_tx.clone();
            scope.spawn(move || {
                let mut ready = false;
                for line in BufReader::new(stdout).lines() {
                    if line.unwrap() == READY {
                        assert!(!ready);
                        ready = true;
                        ready_tx.send(()).unwrap();
                    }
                }
                assert!(ready, "child never reached the posting barrier");
            });
            posters.push(child);
        }
        drop(ready_tx);

        // When every process is ready, release their posting operations together.
        for _ in 0..4 {
            ready_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        for child in &mut posters {
            child.0.stdin.take().unwrap().write_all(&[1]).unwrap();
        }
        for child in &mut posters {
            assert!(child.0.wait().unwrap().success());
        }
    });

    // Then cross-process locking preserved every message and the next durable sequence.
    drop(store);
    let reopened = Store::open(root.path()).unwrap();
    let inbox = Inbox::new(&reopened, Recipient::Pm).unwrap();
    assert_eq!(
        inbox
            .messages()
            .unwrap()
            .iter()
            .map(|message| message.seq)
            .collect::<Vec<_>>(),
        (1..=16).collect::<Vec<_>>()
    );
    assert_eq!(inbox.state().unwrap().next_seq, 17);
    assert_eq!(inbox.post(post("next", Urgency::Wake)).unwrap().seq, 17);
}
