use super::*;

#[test]
fn new_idea_persists_each_difficulty_independently_of_size() {
    // Given an initialized workspace.
    let ws = init_ws();
    for difficulty in ["routine", "standard", "hard"] {
        // When an idea is filed with assessed reasoning difficulty.
        let id = ok(
            &ws.root,
            &[
                "new",
                "Idea",
                "--idea",
                "--size",
                "xs",
                "--difficulty",
                difficulty,
            ],
        );
        // Then each assessment survives without changing work volume or idea status.
        let task: serde_json::Value =
            serde_json::from_str(&ok(&ws.root, &["show", id.trim(), "--json"])).unwrap();
        assert_eq!(task["difficulty"], difficulty);
        assert_eq!(task["size"], "xs");
        assert_eq!(task["status"], "idea");
    }
}

#[test]
fn difficulty_routes_large_routine_and_small_hard_tasks() {
    // Given reasoning rules before a legacy large-task rule.
    let ws = routing_ws(
        r#"
[[routing.rules]]
difficulty = "routine"
profile = "task"
[[routing.rules]]
difficulty = "hard"
profile = "rule"
[[routing.rules]]
size = "xl"
profile = "rule"
"#,
    );
    for (size, difficulty, expected) in [("xl", "routine", "task"), ("xs", "hard", "rule")] {
        // When a classified task is filed and routed through the real CLI.
        let id = ok(
            &ws.root,
            &[
                "new",
                "Implementation",
                "-p",
                "app",
                "--size",
                size,
                "--difficulty",
                difficulty,
            ],
        );
        let route: serde_json::Value =
            serde_json::from_str(&ok(&ws.root, &["route", id.trim(), "--json"])).unwrap();
        // Then difficulty selects the preset independently of volume.
        assert_eq!(route["profile"], expected);
        assert_eq!(route["route"]["source"], "rule");
    }
}
