import { expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as sdk from "@earendil-works/pi-coding-agent";
import auth from "./fixtures/auth/index.ts";
import { fakeCodexTransport } from "./fixtures/auth/fake-transport.ts";
import { createModelCatalog } from "../src/host/catalog.ts";
import { authContext, restrictProvider } from "../src/host/session.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { runAuxiliarySession } from "../src/extensions/subagent/session.ts";
import { sha256Hex } from "../src/tools/corral.ts";

test("real SDK review preserves terminal causes independently of snapshot coverage", async () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-review-sdk-stops-")), originalFetch = globalThis.fetch;
  try {
    let transport = fakeCodexTransport(), boundary: { mode: string; after: number } | undefined;
    const network = guardedFetch((input, init) => {
      if (boundary && transport.requests === boundary.after) {
        if (boundary.mode === "timeout" || boundary.mode === "cancelled") transport.holdNext();
        if (boundary.mode === "provider_error") transport.failNext();
      }
      return transport.send(input, init);
    });
    globalThis.fetch = network.fetch;
    const models = await createModelCatalog(auth.credentials(authContext()), { providers: ["openai-codex"] });
    restrictProvider(models, "openai-codex", network.fetch as typeof globalThis.fetch);
    const snapshot = (name: string, text: string) => {
      const path = join(dir, name); writeFileSync(path, text);
      return { path, bytes: Buffer.byteLength(text), digest: sha256Hex(text) };
    };
    const plan = snapshot("plan.md", "# Exact draft\n"), context = snapshot("context.json", "{}");
    const failures = [
      { mode: "timeout", reason: "timeout", requests: 1 },
      { mode: "cancelled", reason: "cancelled", requests: 1 },
      { mode: "provider_error", reason: "provider_error", requests: 1 },
      { mode: "fenced", reason: "cancelled", requests: 0 },
    ] as const;
    const cases = [
      ...failures.flatMap(item => [false, true].map(fullCoverage => ({ ...item, fullCoverage,
        status: fullCoverage ? "partial" : "failed", outcome: "uncertain", requests: item.requests + Number(fullCoverage) }))),
      { mode: "coverage", fullCoverage: false, status: "partial", reason: "completed", outcome: "completed", requests: 2 },
      { mode: "turn_limit", fullCoverage: false, status: "partial", reason: "turn_limit", outcome: "completed", requests: 2 },
      { mode: "time_reserve", fullCoverage: false, status: "partial", reason: "time_reserve", outcome: "completed", requests: 1 },
      { mode: "complete", fullCoverage: true, status: "completed", reason: "completed", outcome: "completed", requests: 2 },
    ];
    for (const item of cases) {
      transport = fakeCodexTransport(); boundary = { mode: item.mode, after: Number(item.fullCoverage) };
      const control = new AbortController(), held = transport.whenHeld();
      const reads = item.fullCoverage || ["coverage", "turn_limit"].includes(item.mode);
      transport.script([
        ...(reads ? [{ toolCalls: [
          { callId: "plan", name: "read_review_snapshot", arguments: { document: "plan", offset: 0 } },
          ...(item.fullCoverage ? [{ callId: "context", name: "read_review_snapshot", arguments: { document: "context", offset: 0 } }] : []),
        ] }] : []),
        { text: "Advisory findings." },
      ]);
      const id = `${item.mode}-${item.fullCoverage ? "complete" : "incomplete"}`;
      let checks = 0;
      const pending = runAuxiliarySession({ sdk, models, provider: "openai-codex", cwd: dir, agentDir: join(dir, "agent"),
        parentLoadout: ["read"], localTools: [] }, {
        request: id, kind: "explore", provider: "openai-codex", model: "gpt-6.1-sol", prompt: "Review both exact snapshots",
        sessionFile: join(dir, `${id}.jsonl`), sessionId: id, maxTurns: item.mode === "turn_limit" ? 2 : 12,
        ...(item.mode === "time_reserve" ? { deadlineAt: 0 } : {}),
        ...(item.mode === "fenced" ? { checkReview: async () => {
          if (++checks > Number(item.fullCoverage)) throw new Error("generation fenced");
        } } : {}),
        review: { association: { workflow: "W1", review: "LR1", digest: plan.digest }, plan, context },
      }, control.signal);
      if (item.mode === "timeout" || item.mode === "cancelled") {
        await Promise.race([held, pending.then(() => { throw new Error("SDK stopped before the held stream"); })]);
        control.abort(new DOMException("injected stop", item.mode === "timeout" ? "TimeoutError" : "AbortError"));
      }
      const result = await pending;
      console.log("REVIEW_SDK_STOP", JSON.stringify({ id, report: result.report, outcome: result.outcome, requests: transport.requests }));
      expect(result.report).toMatchObject({ status: item.status, stopReason: item.reason,
        snapshotCoverage: item.fullCoverage ? "complete" : "incomplete" });
      expect(result.outcome).toBe(item.outcome); expect(transport.requests).toBe(item.requests);
    }
  } finally { globalThis.fetch = originalFetch; rmSync(dir, { recursive: true, force: true }); }
});
