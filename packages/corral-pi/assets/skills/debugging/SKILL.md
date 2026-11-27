---
name: debugging
description: Investigate an unexplained crash, wrong result, hang or intermittent failure by narrowing the cause with evidence.
allowed-tools: read grep find ls edit write bash
---

# Debugging

State what should happen and what happened instead. Capture the failing input and relevant runtime, configuration or version. Check existing findings if available before repeating expensive work. Choose the cheapest observation that distinguishes likely causes, then revise your explanation from the result. If an exception and its failing input already point to a specific boundary, trace that value to the failing operation; don't invent alternatives. When causes remain ambiguous, control one variable at a time and record the probe, observation and what it rules out.

- Reproduce in a bounded, isolated fixture with the narrowest useful input. Trace the actual call path and inspect values at boundaries. Instrument only authorized, task-owned processes; avoid collecting secrets or exposing sensitive inputs in logs or reports. Don't load-test a shared host. Use an authorized, capped environment or report that load reproduction is blocked.
- For a flake, record event order and timing, then check shared state such as leaked fixtures, reused ports or paths, and concurrent writes. For example, check whether the consumer runs before readiness is signaled. Synchronize on the responsible event rather than increasing sleeps or retry budgets.
- Confirm the cause when a targeted change to the suspected condition toggles the failure or equivalent direct evidence connects it to the symptom. If the next distinguishing probe needs unavailable access, data or authority, stop and report what was observed, what is inferred and what would settle it. In investigation-only mode, report the evidence without patching.

When a fix is in scope, keep the failing scenario as regression coverage where practical, make the smallest root-cause change and rerun that scenario and nearby affected checks. A typecheck or mocked happy path alone does not establish that the original failure is gone. Remove only your own temporary instrumentation, fixtures and processes; retain requested evidence and check the diff for debug logging or sensitive data.
