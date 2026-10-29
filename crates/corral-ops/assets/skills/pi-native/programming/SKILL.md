---
name: programming
description: Write type-safe, minimal code with typed errors, exhaustive variants and tests that can fail. Use for Rust, TypeScript, JavaScript, Python and Go implementation work.
allowed-tools: read grep find ls edit write bash lsp_diagnostics lsp_definition lsp_references
---
{{marker}}

# Programming

Read repository guidance, manifests, the affected implementation, callers and nearby tests before
editing. Reuse existing helpers or the standard library before adding code or dependencies.
Fix the shared root cause, not each symptom. Respect repo-local size ceilings, language versions,
error conventions and verification mode; do not impose a new toolchain or a blanket refactor.

## Types and boundaries

- Parse untrusted input once at its boundary into a typed value; distinguish IDs and units where
  mixing them would be a bug. Prefer immutable data and make illegal states unrepresentable.
- Match enums and discriminated unions exhaustively. In Rust use exhaustive match; in TypeScript
  narrow unknown inputs and use a never-based exhaustiveness check; in Python use typed variants
  and assert_never; in Go model closed variants deliberately and use the project's checking tools.
  Avoid catch-all defaults that silently accept a newly added variant.
- Use typed errors with useful context: domain error variants in libraries, contextual application
  errors at external calls, narrowed exceptions or error results in dynamic languages. Preserve
  causes; do not swallow failures, fabricate success or replace errors with ambiguous null values.
- Do not paper over contracts with unchecked casts, any, type-check suppression, non-null assertions,
  or production unwrap/expect. Follow the repository's explicit exceptions, not convenience.
- Bound external I/O and retries; release files, locks, child processes and network resources on
  every exit. Propagate cancellation; do not block an async executor with synchronous work.

## Tests that can fail

Read the existing tests before changing behavior. For a bug, reproduce the failure before fixing
it and retain the same scenario as regression coverage where the repository keeps such tests.
Use the cheapest test layer that observes the behavior, with Given/When/Then conditions, independent
expected values and assertions on outcomes (including meaningful error/boundary cases).

Prefer real objects and narrow fakes over mocking every collaborator. Never derive expected values
from the result under test, use fixtures whose overrides equal their defaults, or pin prose to prove
runtime behavior. A test should fail if the behavior is removed or regressed. Synchronize concurrent
tests on events/barriers, not sleeps; isolate temp paths, ports and subprocess environments and clean
up owned resources. Never hide a failing test by weakening assertions or regenerating expectations.

## Change and check

Use `read`, `grep`, `find` and `ls` to inspect; `edit` for precise changes and `write` for new files.
If available, use `lsp_definition` and `lsp_references` to trace contracts and `lsp_diagnostics` for
changed-file feedback. Use `bash` for the relevant compiler/linter/test command under repo policy.
Before finishing inspect the diff for scope, ownership, exhaustive variants, resource cleanup and
type escape hatches. Report exact checks and limitations; compilation alone does not prove the
user scenario. Do not add broad cleanup or a full gate the task did not authorize.
