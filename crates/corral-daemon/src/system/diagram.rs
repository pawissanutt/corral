//! Immutable facts prepared by the historical snapshot worker, not by element reads.
use std::collections::{BTreeMap, BTreeSet};

use corral_facts::{Coverage, FileFact, ReadStatus, Reference, TreeFacts};
use corral_protocol::{detail::DiagramRelated, diagram::valid_path};

const RELATED_MAX: usize = 16;

pub(crate) struct DiagramFacts {
    // The worker uses this tree for the optional provider overlay. Views only
    // see the file lookup, coverage and bounded inbound projection below.
    pub(super) tree: TreeFacts,
    inbound: Inbound,
    /// Files read completely by whatever produced `tree.file_references`: the built-in scan,
    /// or a provider that supplied references and reported its own file reads. The merged
    /// `read_status` can mix the two, so it is no evidence for the other's references.
    referrers_read: BTreeSet<String>,
}

impl DiagramFacts {
    /// Built-in facts: the scan that found the references read these files.
    pub(super) fn new(tree: TreeFacts) -> Self {
        let read = tree
            .files
            .iter()
            .filter(|file| file.read_status == ReadStatus::Complete)
            .map(|file| file.path.clone())
            .collect();
        Self::with_referrers(tree, read)
    }

    /// A provider-enriched tree. References the provider replaced count only its own explicit
    /// reads; references it left built-in keep the built-in scan's reads from before overlay.
    pub(super) fn enriched(
        &self,
        tree: TreeFacts,
        provider_references: bool,
        provider_read: BTreeSet<String>,
    ) -> Self {
        let read = if provider_references {
            provider_read
        } else {
            self.referrers_read.clone()
        };
        Self::with_referrers(tree, read)
    }

    fn with_referrers(tree: TreeFacts, referrers_read: BTreeSet<String>) -> Self {
        let inbound = Inbound::new(&tree.file_references);
        Self {
            tree,
            inbound,
            referrers_read,
        }
    }

    /// Whether the producer of these references read `path` completely.
    pub(crate) fn referrer_read(&self, path: &str) -> bool {
        self.referrers_read.contains(path)
    }

    pub(crate) fn file(&self, path: &str) -> Option<&FileFact> {
        let index = self
            .tree
            .files
            .binary_search_by(|file| file.path.as_str().cmp(path))
            .ok()?;
        self.tree.files.get(index)
    }

    pub(crate) fn coverage(&self) -> &Coverage {
        &self.tree.coverage
    }

    pub(crate) fn inbound(&self, path: &str) -> (Vec<DiagramRelated>, usize) {
        self.inbound.get(path)
    }
}

#[derive(Default)]
struct Related {
    rows: Vec<DiagramRelated>,
    omitted: usize,
}

struct Inbound(BTreeMap<String, Related>);

impl Inbound {
    fn new(references: &[Reference<String>]) -> Self {
        let mut by_target = BTreeMap::<&str, Related>::new();
        for row in references.iter().filter(|row| row.from != row.to) {
            let target = by_target.entry(&row.to).or_default();
            if target.rows.len() == RELATED_MAX || !valid_path(&row.from) {
                target.omitted += 1;
            } else {
                target.rows.push(DiagramRelated::FileReference {
                    path: row.from.clone(),
                    basis: row.basis,
                    reference_kind: row.kind,
                    count: row.count,
                });
            }
        }
        Self(
            by_target
                .into_iter()
                .map(|(path, related)| (path.to_owned(), related))
                .collect(),
        )
    }

    fn get(&self, path: &str) -> (Vec<DiagramRelated>, usize) {
        // One keyed lookup; neither crowded targets nor unrelated references
        // can add work beyond cloning the bounded reply rows.
        self.0
            .get(path)
            .map(|related| (related.rows.clone(), related.omitted))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use corral_facts::{ModuleId, ReferenceBasis, ReferenceKind};

    use super::*;

    fn tree(files: &[(&str, ReadStatus)]) -> TreeFacts {
        let module = ModuleId {
            container: String::new(),
            path: String::new(),
        };
        TreeFacts {
            files: files
                .iter()
                .map(|(path, read_status)| FileFact {
                    path: (*path).into(),
                    language: "Rust".into(),
                    lines: 1,
                    test: false,
                    read_status: *read_status,
                    container: String::new(),
                    module: module.clone(),
                    definitions: vec![],
                })
                .collect(),
            containers: vec![],
            modules: vec![module.clone()],
            dependencies: vec![],
            file_references: vec![],
            module_references: vec![],
            package_edges: vec![],
            spawns: vec![],
            runtime: Default::default(),
            entry_points: Default::default(),
            data: Default::default(),
            coverage: Coverage::default(),
        }
    }

    #[test]
    fn referrer_reads_come_from_the_producer_of_the_references() {
        use ReadStatus::{Complete, Unsupported};
        let builtin = DiagramFacts::new(tree(&[("a.rs", Complete), ("b.txt", Unsupported)]));
        assert!(builtin.referrer_read("a.rs") && !builtin.referrer_read("b.txt"));
        // The provider replaced the references but reported no row for a.rs: the merged
        // status still says Complete, and is no evidence the provider read it.
        let merged = tree(&[("a.rs", Complete), ("b.txt", Complete)]);
        let provider = builtin.enriched(merged.clone(), true, ["b.txt".to_owned()].into());
        assert!(!provider.referrer_read("a.rs") && provider.referrer_read("b.txt"));
        // A files-only provider's Complete does not cover the built-in references' unread file.
        let files_only = builtin.enriched(merged, false, ["b.txt".to_owned()].into());
        assert!(files_only.referrer_read("a.rs") && !files_only.referrer_read("b.txt"));
    }

    #[test]
    fn diagram_inbound_projection_keeps_only_bounded_rows_for_each_target() {
        let make = |from: String, to: &str| Reference {
            from,
            to: to.into(),
            basis: ReferenceBasis::Resolved,
            kind: ReferenceKind::Import,
            count: 2,
        };
        let mut refs = vec![
            make("gate.rs".into(), "gate.rs"),
            make("../bad.rs".into(), "gate.rs"),
        ];
        refs.push(make("other.rs".into(), "elsewhere.rs"));
        refs.extend((0..RELATED_MAX + 3).map(|i| make(format!("from-{i}.rs"), "gate.rs")));
        let inbound = Inbound::new(&refs);
        let (rows, omitted) = inbound.get("gate.rs");
        assert_eq!(rows.len(), RELATED_MAX);
        assert_eq!(omitted, 4); // Three over the cap plus the invalid path, not the self-reference.
        assert_eq!(
            rows[0],
            DiagramRelated::FileReference {
                path: "from-0.rs".into(),
                basis: ReferenceBasis::Resolved,
                reference_kind: ReferenceKind::Import,
                count: 2,
            }
        );
        assert_eq!(inbound.get("elsewhere.rs").0.len(), 1);
        assert_eq!(inbound.get("no-inbound.rs"), (Vec::new(), 0));
    }
}
