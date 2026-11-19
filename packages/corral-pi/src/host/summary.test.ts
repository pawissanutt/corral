import { expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as sdk from "@earendil-works/pi-coding-agent";
import fixtureAuth from "../../test/fixtures/auth/index.ts";
import { fakeCodexTransport, requestBody } from "../../test/fixtures/auth/fake-transport.ts";
import { guardedFetch } from "./restrict.ts";
import { createSummaryRequest, summarizeQuick, type SummaryRequest } from "./summary.ts";
import type { ModelUsage } from "../tools/corral.ts";

test("quick rollover uses the configured summary profile for exactly one observed request, including split turns", async () => {
  const transport = fakeCodexTransport();
  const calls: Record<string, unknown>[] = [];
  const usageRows: ModelUsage[] = [];
  const network = guardedFetch(async (input, init) => {
    calls.push(requestBody(init) as Record<string, unknown>);
    return transport.send(input, init);
  });
  const base = { modules: { auth: fixtureAuth }, agentDir: "/unused", cwd: "/unused" };
  expect(await createSummaryRequest(base)).toBeUndefined();
  const dir = mkdtempSync(join(tmpdir(), "corral-summary-"));
  const previousData = process.env.XDG_DATA_HOME;
  let request: SummaryRequest | undefined;
  try {
    process.env.XDG_DATA_HOME = dir;
    request = await createSummaryRequest({ ...base, summary: {
      profile: "cheap-summary", model: "gpt-6.1-terra", provider: "openai-codex", effort: "low",
      network, rules: [],
    } }, undefined, usage => usageRows.push(usage));
  } finally {
    if (previousData === undefined) delete process.env.XDG_DATA_HOME;
    else process.env.XDG_DATA_HOME = previousData;
    rmSync(dir, { recursive: true, force: true });
  }
  if (!request) throw new Error("summary profile not resolved");
  const result = await summarizeQuick(sdk, {
    firstKeptEntryId: "kept", tokensBefore: 20, isSplitTurn: true,
    previousSummary: "Earlier decisions", fileOps: { read: new Set(["read.rs"]), written: new Set(), edited: new Set(["edit.rs"]) },
    settings: { enabled: true, reserveTokens: 8192, keepRecentTokens: 1 },
    messagesToSummarize: [{ role: "user", content: "Prior request", timestamp: 1 }],
    turnPrefixMessages: [{ role: "user", content: "Current turn prefix", timestamp: 2 }],
  }, request, new AbortController().signal, "quick-session");
  expect(calls).toHaveLength(1);
  expect(calls[0]?.model).toBe("gpt-6.1-terra");
  expect(transport.prompts[0]).toContain("Prior request");
  expect(transport.prompts[0]).toContain("Current turn prefix");
  expect(transport.prompts[0]).toContain("Earlier decisions");
  expect(result.firstKeptEntryId).toBe("kept");
  expect(result.details).toMatchObject({ summaryProfile: "cheap-summary", summaryModel: "gpt-6.1-terra" });
  expect(result.usage?.input).toBeGreaterThan(0);
  expect(usageRows).toMatchObject([{ model: "openai-codex/gpt-6.1-terra", billing: "subscription", input: 32, output: 3 }]);
});
