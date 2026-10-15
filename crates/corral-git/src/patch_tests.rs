use super::*;
use crate::tests::{init, sh, wt};

#[test]
fn review_patch_ignores_shifted_hunks_and_function_context() {
    // Given a reviewed change and main inserting lines well before it.
    let (tmp, repo) = init();
    let original = (0..30).map(|i| format!("line {i}\n")).collect::<String>();
    std::fs::write(repo.join("a.txt"), &original).unwrap();
    commit_all(&repo, "baseline").unwrap();
    let worktree = wt(&tmp, &repo, "candidate");
    let base = rev_sha(&repo, "HEAD").unwrap();
    std::fs::write(
        worktree.join("a.txt"),
        original.replace("line 25", "changed 25"),
    )
    .unwrap();
    commit_all(&worktree, "reviewed").unwrap();
    let old = review_patch(&repo, &base, &rev_sha(&worktree, "HEAD").unwrap()).unwrap();
    std::fs::write(repo.join("a.txt"), format!("new context\n{original}")).unwrap();
    commit_all(&repo, "main context").unwrap();
    let new_base = rev_sha(&repo, "HEAD").unwrap();

    // When the real replay relocates the hunk.
    assert_eq!(
        rebase(&worktree, &new_base).unwrap(),
        RebaseOutcome::Rebased
    );

    // Then patch equivalence retains the unchanged edit.
    assert_eq!(
        old,
        review_patch(&repo, &new_base, &rev_sha(&worktree, "HEAD").unwrap()).unwrap()
    );
}

#[test]
fn review_patch_detects_whitespace_content_mode_and_binary_changes() {
    for change in ["whitespace", "content", "mode", "binary"] {
        // Given a committed patch.
        let (_tmp, repo) = init();
        let base = rev_sha(&repo, "HEAD").unwrap();
        std::fs::write(repo.join("a.txt"), "reviewed\n").unwrap();
        commit_all(&repo, "reviewed").unwrap();
        let old = review_patch(&repo, &base, &rev_sha(&repo, "HEAD").unwrap()).unwrap();

        // When any content or mode changes.
        match change {
            "whitespace" => std::fs::write(repo.join("a.txt"), "reviewed \n").unwrap(),
            "content" => std::fs::write(repo.join("a.txt"), "different\n").unwrap(),
            "mode" => sh(&repo, &["update-index", "--chmod=+x", "a.txt"]),
            "binary" => std::fs::write(repo.join("a.txt"), b"binary\0payload").unwrap(),
            _ => unreachable!(),
        }
        if change == "mode" {
            sh(&repo, &["commit", "-m", "changed mode"]);
        } else {
            commit_all(&repo, "changed content").unwrap();
        }

        // Then normalization never hides that change.
        assert_ne!(
            old,
            review_patch(&repo, &base, &rev_sha(&repo, "HEAD").unwrap()).unwrap()
        );
    }
}
