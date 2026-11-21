use std::process::Command;

use corral_engine::UsageSummary;

#[test]
fn usage_add_distinguishes_omitted_counters_from_explicit_zeroes() {
    let root = tempfile::tempdir().unwrap();
    let cli = |args: &[&str]| {
        let result = Command::new(env!("CARGO_BIN_EXE_corral-next"))
            .current_dir(root.path())
            .arg("--root")
            .arg(root.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        result.stdout
    };
    let summary = || serde_json::from_slice::<UsageSummary>(&cli(&["usage", "--json"])).unwrap();
    cli(&["init"]);
    cli(&[
        "usage",
        "add",
        "--model",
        "anthropic-api/claude-sonnet-4-6",
        "--billing",
        "api",
        "--usd",
        "0.125",
    ]);
    let model = "openai-codex/gpt-6.2-sol";
    cli(&[
        "usage",
        "add",
        "--model",
        model,
        "--billing",
        "subscription",
        "--input",
        "1",
        "--output",
        "0",
        "--cache-read",
        "0",
    ]);
    let total = summary().total;
    assert_eq!(total.requests, 2);
    assert_eq!(total.input, 1);
    assert_eq!(total.estimate_missing_token_requests, 2);
    assert_eq!((total.estimate_usd, total.estimated_requests), (None, 0));
    assert_eq!((total.usd, total.priced_requests), (Some(0.125), 1));

    cli(&[
        "usage",
        "add",
        "--model",
        model,
        "--billing",
        "subscription",
        "--input",
        "0",
        "--output",
        "0",
        "--cache-read",
        "0",
        "--cache-write",
        "0",
    ]);
    let total = summary().total;
    assert_eq!(total.requests, 3);
    assert_eq!(total.estimate_missing_token_requests, 2);
    assert_eq!(
        (total.estimate_usd, total.estimated_requests),
        (Some(0.0), 1)
    );
    assert_eq!((total.usd, total.priced_requests), (Some(0.125), 1));
}
