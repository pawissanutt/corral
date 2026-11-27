use super::*;

fn commit(repo: &Path, message: &str) -> String {
    crate::git(repo, &["commit", "--allow-empty", "-q", "-m", message]).unwrap();
    crate::rev_sha(repo, "HEAD").unwrap()
}

/// Pages from one tip concatenate to Git's own topological walk, a merge keeps both parents,
/// and a parent past a page's end is still found further down rather than being a root.
#[test]
fn pages_follow_git_ancestry_across_a_merge() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    crate::git(repo, &["init", "-q", "-b", "main"]).unwrap();
    crate::git(repo, &["config", "user.name", "Fixture"]).unwrap();
    crate::git(repo, &["config", "user.email", "fixture@example.test"]).unwrap();
    let root = commit(repo, "root");
    crate::git(repo, &["checkout", "-q", "-b", "feature"]).unwrap();
    let side = commit(repo, "side");
    crate::git(repo, &["checkout", "-q", "main"]).unwrap();
    let main = commit(repo, "main");
    crate::git(repo, &["merge", "-q", "--no-ff", "-m", "merge", "feature"]).unwrap();
    let merge = crate::rev_sha(repo, "HEAD").unwrap();
    crate::git(repo, &["tag", "-a", "-m", "v1", "v1", &main]).unwrap();

    assert_eq!(
        resolve_tip(repo, Some("main")).unwrap(),
        ("refs/heads/main".into(), Some(merge.clone()))
    );
    assert_eq!(resolve_tip(repo, Some("gone")).unwrap().1, None);
    assert!(resolve_tip(repo, Some("-bad")).is_err());

    let expected: Vec<String> =
        crate::git(repo, &["rev-list", "--topo-order", "--parents", &merge])
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
    let mut walked = Vec::new();
    for skip in (0..).step_by(2) {
        let page = log_page(repo, &merge, skip, 2).unwrap();
        if page.is_empty() {
            break;
        }
        walked.extend(page);
    }
    let lines: Vec<String> = walked
        .iter()
        .map(|c| {
            std::iter::once(c.sha.clone())
                .chain(c.parents.clone())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    assert_eq!(lines, expected);
    assert_eq!(walked[0].parents, [main.clone(), side.clone()]);
    let first_page: BTreeSet<_> = walked[..2].iter().map(|c| c.sha.clone()).collect();
    let later: BTreeSet<_> = walked[2..].iter().map(|c| c.sha.clone()).collect();
    for parent in walked[..2].iter().flat_map(|c| &c.parents) {
        assert!(first_page.contains(parent) || later.contains(parent));
    }
    assert!(
        walked
            .last()
            .is_some_and(|c| c.sha == root && c.parents.is_empty())
    );

    let labels = ref_labels(repo, &walked.iter().map(|c| c.sha.clone()).collect()).unwrap();
    assert_eq!(labels[&merge], ["HEAD", "main"]);
    assert_eq!(labels[&side], ["feature"]);
    assert_eq!(labels[&main], ["tag: v1"]);
    assert!(!labels.contains_key(&root));
}
