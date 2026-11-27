---
name: programming
description: Implement a known behavior change or defect fix by identifying the observable contract, making a scoped change and testing the outcome.
allowed-tools: read grep find ls edit write bash lsp_diagnostics lsp_definition lsp_references
---

# Programming

Name the observable contract: what input or action should produce which result, including relevant failures. Read local guidance and inspect the implementation, affected callers and nearby tests. Find the smallest correct change at the boundary or shared source of the behavior. Prefer existing helpers and conventions over new dependencies or unrelated refactors. If edits aren't authorized, identify the change and the check that would verify it without applying either as a patch.

- Validate input when it crosses a trust or format boundary. Preserve useful error context without turning failures into apparent success. Where a set of variants really is closed, handle each explicitly so a new case cannot silently take a default path. Match the project's conventions without adding a type system just to express a local contract.
- Where the change owns resources or external work, bound I/O and retries as appropriate, propagate cancellation, and release resources on success, failure and cancellation. Don't block an async executor with synchronous work.

Use the cheapest check that can fail for the named defect. Where practical, observe it fail before the fix and rerun the same case afterward. Assert independently known outcomes, not values computed by the code under test. Keep fakes narrow; for concurrent checks synchronize on events, not sleeps, and isolate shared resources.

For example, if an untyped script treats an invalid count as zero, validate it at argument parsing and check that a malformed argument exits nonzero without producing output, while a valid count still produces the expected result. The script needs neither a new framework nor a language server for that check.

Read the final diff for contract changes outside the requested scope. Stop when the targeted behavior and affected cases are verified, or report which checks could not run and what remains unproven. Compilation alone does not verify the user scenario.
