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
