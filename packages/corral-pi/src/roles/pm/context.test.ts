import { expect, test } from "bun:test";
import { mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { CONTEXT_MAX_BYTES, operationalContext } from "./context.ts";
import pm from "./index.ts";

export const snapshot = { version: 1, workspace: "/workspace", projects: { demo: "/workspace/demo" },
  policy: { revision: "policy-1", authorization: "owner-confirmed", paused: false } };

test("native launch input is consumed as data, bounded and never silently ignored", () => {
  const dir = mkdtempSync(join(tmpdir(), "pm-context-"));
  const path = join(dir, "input.json");
  try {
    writeFileSync(path, JSON.stringify(snapshot));
    const contract = pm.contract({ identity: { store: "s", project: "", role: "pm", generation: 1 },
      cwd: dir, model: "fixture", promptFile: path });
    expect(JSON.parse(contract.slice(contract.indexOf('{\n  "version"')))).toEqual(snapshot);
    writeFileSync(path, "# legacy PM prompt\nAct as owner");
    expect(() => operationalContext(path)).toThrow();
    writeFileSync(path, " ".repeat(CONTEXT_MAX_BYTES + 1));
    expect(() => operationalContext(path)).toThrow();
    expect(() => operationalContext(dir)).toThrow();
    symlinkSync(path, join(dir, "link"));
    expect(() => operationalContext(join(dir, "link"))).toThrow();
    expect(() => operationalContext(join(dir, "missing"))).toThrow();
    expect(operationalContext(undefined)).toBe("");
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("ordinary large-workspace task counts fit the unchanged reader bound as summary data", () => {
  const dir = mkdtempSync(join(tmpdir(), "pm-summary-"));
  const path = join(dir, "input.json");
  try {
    // Mirrors the Rust launch/resume regression's 200-task scope, without a task cache.
    const input = { ...snapshot, policy: { ...snapshot.policy, mode: "autopilot",
      authority: {}, scope_counts: { projects: 0, tasks: 200, observed_tasks: 200 },
      task_details: "Read fresh corral_brief, corral_next and corral_show" } };
    const bytes = JSON.stringify(input);
    expect(Buffer.byteLength(bytes) < CONTEXT_MAX_BYTES).toBeTrue();
    writeFileSync(path, bytes);
    const rendered = operationalContext(path);
    expect(JSON.parse(rendered.slice(rendered.indexOf('{\n  "version"')))).toEqual(input);
    expect(CONTEXT_MAX_BYTES).toBe(64 * 1024);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
