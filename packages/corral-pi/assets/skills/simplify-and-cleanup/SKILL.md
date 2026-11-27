---
name: simplify-and-cleanup
description: Reduce concrete maintenance costs in scoped code, comments or docs without changing behavior or meaning. Use for cleanup and simplification, not feature work, redesign or correctness-only review.
allowed-tools: read grep find ls edit write bash lsp_diagnostics lsp_definition lsp_references
---

# Simplify and cleanup

1. **Bound the diff.** Identify the files or base and which staged, unstaged or untracked changes are in scope. Read the surrounding code or prose and applicable guidance. Preserve others' edits; leave generated, vendored and historical material alone unless included explicitly.
2. **Name the cost.** Look for duplicated maintenance, misleading commentary, or indirection that adds work without serving a contract. Do not change something because of its presumed authorship or to meet a deletion quota.
3. **Establish equivalence.** Use the relevant contracts, callers and checks to explain what stays the same. Consider outputs, errors, side effects, ordering, cancellation and public interfaces where relevant. For prose, compare the claims, qualifications and links.
4. **Edit or skip.** Make the smallest coherent change if authorized and supported by evidence. Otherwise record the missing evidence or report that nothing justified changing. Audit-only requests end with findings and proposed edits, not modifications. The listed tools grant no permissions and are not prerequisites.

## Code: check what the extra layer does

A private forwarding function may only duplicate a call, or it may be an extension hook, compatibility path or test seam. Inspect callers and registration or dynamic references before inlining it. If its role is uncertain, keep it. Likewise, a green suite does not show that a fallback never runs. Look for the actual reachability contract and callers before removing guards, error paths or fallbacks; preserve trust-boundary validation and resource cleanup. Surface bug fixes or architecture changes separately instead of folding them into cleanup.

Use the smallest relevant existing checks; compare with a baseline when needed to distinguish pre-existing failures. Add a test only for a concrete behavioral risk not covered by existing checks, and make sure it can fail for that risk. Skip a change if establishing safety would require investigation outside scope.

## Prose: remove repetition without removing a contract

If two paragraphs repeat a procedure under a publicly linked heading, consolidate the wording while keeping that anchor and any warning, prerequisite or exception. Preserve runnable examples, citations and machine-consumed text unless their correction is in scope and verified. Trim comments that narrate syntax; keep rationale, invariants, security warnings, licenses and tool directives. Verify stale statements before correcting them; otherwise flag them.

For editorial changes, compare the before and after text and use existing docs or link checks where relevant; no new test is required just because wording changed. Stop once the scoped improvement is supported or no candidate meets that bar. Report the concrete improvement or reason for leaving things alone, and distinguish checks actually run from remaining uncertainty.
