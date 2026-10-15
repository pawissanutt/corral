use super::*;

#[test]
fn engine_versions_compare_numerically_with_prereleases_below_stable() {
    // Given the stable plugin minimum.
    let minimum = version_numbers(MIN_HERDR_VERSION).unwrap();
    // When parsing released, preview and metadata versions.
    let old = version_numbers("herdr 0.9.2").unwrap();
    let newer = version_numbers("herdr v0.10.0").unwrap();
    let preview = version_numbers("herdr 0.9.3-preview.1");
    let metadata = version_numbers("herdr 0.9.3+build");
    // Then comparisons are numerical and equal-minimum previews cannot satisfy the stable minimum.
    assert!(old < minimum);
    assert!(newer > minimum);
    assert_eq!(preview, Some((0, 9, 3, false)));
    assert!(preview.unwrap() < minimum);
    assert_eq!(metadata, Some(minimum));
}

#[test]
fn friendly_errors_keep_application_errors_and_hide_engine_transport_details() {
    // Given an application failure and a wrapped engine transport failure.
    let application = anyhow::anyhow!("task has no project").context("dispatch");
    let transport: anyhow::Error = HerdrError {
        args: "workspace list".into(),
        code: "server_not_running".into(),
        message: "private socket details".into(),
    }
    .into();
    let transport = transport.context("opening board");
    // When displayed through the application boundary helper.
    let application_text = friendly_error(&application);
    let transport_text = friendly_error(&transport);
    // Then only the transport error is replaced by the actionable engine fix.
    assert_eq!(application_text, "dispatch: task has no project");
    assert_eq!(transport_text, ENGINE_DOWN);
}
