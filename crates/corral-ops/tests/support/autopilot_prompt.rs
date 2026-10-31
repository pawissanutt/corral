/// The materialized dispatch prompt of `task`'s run `run`, from the run's prompt record.
pub(super) fn dispatch_prompt(
    store: &corral_core::Store,
    task: &str,
    run: &str,
) -> std::path::PathBuf {
    let record = store
        .record_row("run-prompts", &format!("{task}/{run}"))
        .unwrap()
        .expect("dispatch rendered no prompt");
    record.doc["file"].as_str().unwrap().into()
}
