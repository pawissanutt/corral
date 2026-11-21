use super::*;
use crate::client::durable_native::windows::TestState;
use std::fs;

fn credential() -> WebCredential {
    WebCredential {
        v: 1,
        device: "dev-test".into(),
        name: "Corral on Windows".into(),
        private: "42".repeat(32),
        daemon: "31".repeat(32),
        port: 7841,
        endpoint: Some("ws://127.0.0.1:7841".into()),
    }
}

#[test]
fn dpapi_pairing_and_forget_survive_reopening_the_store() {
    let TestState(root) = &TestState::new();
    let credential = credential();
    let mut plaintext = encode(&credential).unwrap();
    assert!(matches!(
        load_locked(&open(root).unwrap()),
        Ok(Stored::Missing)
    ));
    save_locked(&open(root).unwrap(), &mut plaintext).unwrap();
    let path = root.join("corral-app").join(NAME);
    let protected = fs::read(&path).unwrap();
    assert!(
        !protected
            .windows(credential.private.len())
            .any(|bytes| bytes == credential.private.as_bytes())
    );
    let Stored::Credential(restored) = load_locked(&open(root).unwrap()).unwrap() else {
        panic!("the approved pairing did not restore");
    };
    assert_eq!(restored.device, credential.device);
    assert!(restored.private == credential.private);
    assert_eq!(restored.endpoint, credential.endpoint);

    begin_forget_locked(&open(root).unwrap()).unwrap();
    assert_eq!(fs::read(&path).unwrap(), FORGET_PENDING);
    assert!(matches!(
        load_locked(&open(root).unwrap()),
        Ok(Stored::ForgetPending)
    ));
    assert!(save_locked(&open(root).unwrap(), &mut plaintext).is_err());
    clear_locked(&open(root).unwrap()).unwrap();
    assert!(matches!(
        load_locked(&open(root).unwrap()),
        Ok(Stored::ForgetPending)
    ));
    finish_forget_locked(&open(root).unwrap()).unwrap();
    assert!(matches!(
        load_locked(&open(root).unwrap()),
        Ok(Stored::Missing)
    ));
    save_locked(&open(root).unwrap(), &mut plaintext).unwrap();
    assert!(matches!(
        load_locked(&open(root).unwrap()),
        Ok(Stored::Credential(_))
    ));
    assert!(clear_locked(&open(root).unwrap()).is_err());
    assert!(finish_forget_locked(&open(root).unwrap()).is_err());
}

#[test]
fn corrupt_or_noncanonical_windows_pairing_fails_closed() {
    let TestState(root) = &TestState::new();
    let mut credential = credential();
    credential.endpoint = Some("wss://EXAMPLE.test".into());
    assert!(encode(&credential).is_err());
    credential.endpoint = Some("wss://example.test".into());
    credential.daemon.pop();
    assert!(encode(&credential).is_err());

    // A truncated tombstone isn't an empty store: it blocks pairing until explicit Forget.
    let file = open(root).unwrap();
    file.replace(&FORGET_PENDING[..FORGET_PENDING.len() - 1])
        .unwrap();
    assert!(load_locked(&file).is_err());
    let mut plaintext = encode(&self::credential()).unwrap();
    assert!(save_locked(&file, &mut plaintext).is_err());
    begin_forget_locked(&file).unwrap();
    assert!(matches!(load_locked(&file), Ok(Stored::ForgetPending)));
}
