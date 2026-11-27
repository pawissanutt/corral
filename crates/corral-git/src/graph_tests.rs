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

    let labels = ref_labels(repo, &walked.iter().map(|c| c.sha.clone()).collect())
        .unwrap()
        .labels;
    assert_eq!(labels[&merge], ["HEAD", "main"]);
    assert_eq!(labels[&side], ["feature"]);
    assert_eq!(labels[&main], ["tag: v1"]);
    assert!(!labels.contains_key(&root));
}

/// A shallow clone's boundary commit keeps its recorded parent (absent here, never a root).
#[test]
fn shallow_boundary_keeps_its_real_parent() {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin");
    std::fs::create_dir(&origin).unwrap();
    crate::git(&origin, &["init", "-q", "-b", "main"]).unwrap();
    crate::git(&origin, &["config", "user.name", "Fixture"]).unwrap();
    crate::git(&origin, &["config", "user.email", "fixture@example.test"]).unwrap();
    let root = commit(&origin, "root");
    let tip = commit(&origin, "tip");
    let url = format!("file://{}", origin.display());
    let clone = dir.path().join("clone");
    let clone_arg = clone.to_string_lossy().into_owned();
    crate::git(
        dir.path(),
        &["clone", "-q", "--depth", "1", &url, &clone_arg],
    )
    .unwrap();
    let page = log_page(&clone, &tip, 0, 10).unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].parents, [root]);
}

fn fixture(repo: &Path) {
    crate::git(repo, &["init", "-q", "-b", "main"]).unwrap();
    crate::git(repo, &["config", "user.name", "Fixture"]).unwrap();
    crate::git(repo, &["config", "user.email", "fixture@example.test"]).unwrap();
}

/// A graft hiding a commit's parent makes `git log` report it as a root; the page walks the
/// recorded parents instead.
#[test]
fn a_graft_does_not_turn_a_child_into_a_root() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    fixture(repo);
    let parent = commit(repo, "parent");
    let child = commit(repo, "child");
    std::fs::write(repo.join(".git/info/grafts"), format!("{child}\n")).unwrap();
    let page = log_page(repo, &child, 0, 10).unwrap();
    let walked: Vec<_> = page
        .iter()
        .map(|c| (c.sha.clone(), c.parents.clone()))
        .collect();
    assert_eq!(walked, [(child, vec![parent.clone()]), (parent, vec![])]);
}

/// A commit with more refs than a page keeps still shows the first ones and counts the rest.
#[test]
fn labels_past_the_cap_are_counted_not_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    fixture(repo);
    let tip = commit(repo, "tip");
    for n in 0..20 {
        crate::git(repo, &["branch", &format!("b{n:02}")]).unwrap();
    }
    let found = ref_labels(repo, &BTreeSet::from([tip.clone()])).unwrap();
    assert_eq!(found.labels[&tip].len(), LABELS_MAX);
    // HEAD, main and 20 branches.
    assert_eq!(found.omitted[&tip], 22 - LABELS_MAX as u64);
    assert!(!found.incomplete);
}
