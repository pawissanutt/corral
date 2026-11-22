# corral-facts

Built-in inputs to the [system map](../../docs/plans/system-map.md). This library
has no daemon, extension, or model dependency. Call it off the scheduling path.

```rust,no_run
use std::path::Path;
use corral_facts::{FactsCache, resolve_revision};

# fn main() -> anyhow::Result<()> {
let workspace = Path::new("/path/to/workspace");
let repo = workspace.join("project");
let cache = FactsCache::new(workspace);
let head = resolve_revision(&repo, "HEAD")?;
let facts = cache.project_facts(&repo, &head)?;
let base = resolve_revision(&repo, "HEAD^1")?;
let delta = cache.delta(&repo, &base, &head)?;
# Ok(())
# }
```

Resolve moving refs again when they move or the clone's shallow boundary
changes. Reuse the resolved revision for subsequent requests. `tree_facts`
caches by tree ID under `.corral-next/cache/system-map/`; activity is cached by
**commit ID** and shallow boundary, because an empty commit can move the
activity windows even when its tree is unchanged.
Repeat requests with the same resolved revision make no Git calls. Deltas cache
by the ordered pair of trees. The cache may be deleted and rebuilt.

Activity counts non-merge commits reachable from the requested commit, in
inclusive windows ending at its committer timestamp. This counts a branch's
changes once when it merges, but not edits made in the merge commit itself.
Activity includes per-commit changes so callers can count distinct commits
across several files. Shallow clones report partial activity and omit changes
at the cut-off commits whose parents are unavailable. Diffs treat renames as
removal and addition, and binary changes as touches without line churn.
Activity and delta require Git 2.41 or newer. They read committed attributes in
an isolated, temporary bare Git directory with read-only access to the source
object database. Neither the checkout nor local diff drivers affect the facts.

The scanner is heuristic, not a language resolver. It recognizes common Rust
`use` paths, JS/TS imports and some literal process constructors. Local package
references honor Cargo’s `[lib].path` and npm’s `main` when they point to tracked
files in the target package. The scanner does not execute manifests or build
scripts, load editor/compiler configuration, or resolve arbitrary aliases,
generated code or dynamic process names. Unresolved
syntax matches and unreadable files are counted separately. In particular an
external dependency can have a declaration without any local map endpoint.
Git paths that cannot be represented as UTF-8 are counted as omissions.
