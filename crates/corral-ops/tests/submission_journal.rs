//! Keep task changes and their events in the same Store callback.
#[path = "../src/journal_allowlist.rs"]
mod journal_allowlist;
#[path = "support/journal_scan.rs"]
mod journal_scan;

#[test]
fn submission_paths_do_not_split_task_writes_from_events() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut findings = Vec::new();
    journal_scan::scan_tree(&root, &root, &mut findings);
    assert!(
        findings.is_empty(),
        "task writes followed by standalone events (move the event into the callback, or name \
         the exact intervening external effect in journal_allowlist.rs):\n{}",
        findings.join("\n")
    );
}

#[test]
fn submission_paths_scanner_distinguishes_callbacks_and_standalone_events() {
    assert!(
        journal_scan::scan(
            "dispatch.rs",
            r#"fn reply(s: &Store) {
        s.update(id, |_| Ok(()))?;
        let kind = if control { "checkpoint_requested" } else { "replied" };
        deliver(ctx, task, text)?;
        s.log_event(Some(id), kind, details)?;
    }"#
        )
        .is_empty()
    );
    assert_eq!(
        journal_scan::scan(
            "dispatch.rs",
            r#"fn reply(s: &Store) {
        s.update(id, |_| Ok(()))?;
        let kind = if control { "checkpoint_requested" } else { "new_event" };
        deliver(ctx, task, text)?;
        s.log_event(Some(id), kind, details)?;
    }"#
        )
        .len(),
        1,
        "a new selected event must not inherit an existing effect exception"
    );
    for method in [
        "create",
        "create_initialized",
        "create_batch",
        "publish_staged",
    ] {
        assert_eq!(journal_scan::scan("sample.rs", &format!(
            "fn save(s: &Store) {{ s.{method}(args)?; s.log_event(None, \"created\", details)?; }}"
        )).len(), 1, "{method}");
    }
    assert!(
        journal_scan::scan(
            "sample.rs",
            r#"fn save(s: &Store) {
        operating::submit(ctx, auth, |s, task| {
            s.update(id, |_| Ok(()))?;
            s.log_event(Some(id), "buffered", details)?;
            Ok(())
        }, || Ok(()))?;
    }"#
        )
        .is_empty()
    );
    assert!(
        journal_scan::scan(
            "gate.rs",
            r#"fn gate_start_origin(s: &Store) {
        s.update(id, |_| Ok(()))?;
        launch.submit(prepare, || pane_run())?;
        s.log_event(Some(id), "gate_started", details)?;
    }"#
        )
        .is_empty()
    );
    assert_eq!(
        journal_scan::scan(
            "gate.rs",
            r#"fn gate_start_origin(s: &Store) {
        s.update(id, |_| Ok(()))?;
        s.log_event(Some(id), "gate_started", details)?;
    }"#
        )
        .len(),
        1,
        "an allowlisted function still needs its named effect"
    );
    assert_eq!(
        journal_scan::scan(
            "gate.rs",
            r#"fn gate_start_origin(s: &Store) {
        s.update(id, |_| Ok(()))?;
        launch.submit(prepare, || pane_run())?;
        s.log_event(Some(id), "unrelated", details)?;
    }"#
        )
        .len(),
        1,
        "an effect exception only permits its exact event"
    );
    assert!(
        journal_scan::scan(
            "sample.rs",
            r#"fn save(s: &Store) {
            s.update(id, |task| {
                task.body = "not code: .log_event(".into();
                s.log_event(Some(id), "saved", json!({}))?;
                Ok(())
            })?;
        }"#
        )
        .is_empty()
    );
    assert_eq!(
        journal_scan::scan(
            "sample.rs",
            r#"fn save(s: &Store) {
                s.dispatch_update(id, |s, task| {
                    s.log_event(Some(id), "inside", json!({}))?;
                    Ok(())
                })?;
                if changed { s.log_event(Some(id), "outside", json!({}))?; }
            }"#
        )
        .len(),
        1
    );
    assert!(
        journal_scan::scan(
            "sample.rs",
            r#"fn submit(s: &Store) {
            s.submit_operating_action(a, e, |s, task| {
                s.update(id, |t| Ok(()))?;
                s.log_event(Some(id), "buffered", json!({}))?;
                Ok(())
            }, || Ok(()))?;
        }
        #[cfg(test)] mod tests {
            fn fixture(s: &Store) {
                s.update(id, |_| Ok(()));
                s.log_event(Some(id), "fixture", json!({}));
            }
        }"#
        )
        .is_empty()
    );
    assert_eq!(
        journal_scan::scan(
            "sample.rs",
            r##"fn save(s: &Store) {
                /* nested /* .update() */ .log_event() */
                let text = r#".update(); .log_event()"#;
                s.try_update(id, |_| Ok(true))?;
                s.log_event(Some(id), "saved", json!({}))?;
            }"##
        )
        .len(),
        1
    );
}
