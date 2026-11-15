//! Host-side coverage of the same identity policy used by the browser adapter.
#[path = "identity.rs"]
mod identity;

use identity::{ForgetState, Stored};

#[test]
fn missing_paired_marker_does_not_replace_the_saved_identity() {
    // Startup deliberately has no marker argument: IndexedDB owns the identity.
    let saved_device = String::from("previously-paired-device");
    let Stored::Credential(restored) = Stored::Credential(saved_device.clone()).for_startup(false)
    else {
        panic!("ordinary startup must resume its stored credential");
    };
    assert_eq!(restored, saved_device);
    assert!(matches!(
        Stored::Credential(restored).for_startup(true),
        Stored::Missing
    ));
}

#[test]
fn incomplete_forget_keeps_tombstone_retryable_before_any_identity_resume() {
    let mut forget = ForgetState::default();
    forget.begin();
    assert!(
        forget
            .complete(Err("credential deletion failed".into()))
            .is_err()
    );
    assert!(forget.pending());
    // Startup must retry deletion even when an explicit new offer is present.
    for explicit_pairing in [false, true] {
        assert!(matches!(
            Stored::<String>::ForgetPending.for_startup(explicit_pairing),
            Stored::ForgetPending
        ));
    }
    forget.begin();
    assert!(forget.complete(Ok(())).is_ok());
    assert!(!forget.pending());
}
