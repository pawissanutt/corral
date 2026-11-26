//! Immutable facts prepared by the historical snapshot worker, not by element reads.
use std::collections::BTreeMap;

use corral_facts::{Coverage, FileFact, Reference, TreeFacts};
use corral_protocol::{detail::DiagramRelated, diagram::valid_path};

const RELATED_MAX: usize = 16;

pub(crate) struct DiagramFacts {
    // The worker uses this tree for the optional provider overlay. Views only
    // see the file lookup, coverage and bounded inbound projection below.
    pub(super) tree: TreeFacts,
    inbound: Inbound,
}

impl DiagramFacts {
    pub(super) fn new(tree: TreeFacts) -> Self {
        let inbound = Inbound::new(&tree.file_references);
        Self { tree, inbound }
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
    use corral_facts::{ReferenceBasis, ReferenceKind};

    use super::*;

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
