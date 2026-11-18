import { expect, test } from "bun:test";
import { cpSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { EventLog } from "../bridge/events.ts";
import type { Role } from "../roles/types.ts";
import { CorralClient } from "../tools/corral.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "../../test/support/fixture-install.ts";
import { loadModules } from "./modules.ts";
import { guardedFetch } from "./restrict.ts";
import { authContext, HostSession } from "./session.ts";
import { ASK_USER_GUIDELINES } from "../roles/common/ask-user.ts";

test("configured host loadouts expose ask_user and its guidance across roles", async () => {
  const assets = buildFixtureInstall({ omit: ["role"] });
  const work = mkdtempSync(join(tmpdir(), "ask-user-loadouts-"));
  cpSync(join(PACKAGE_ROOT, "assets"), join(assets.dir, "assets"), { recursive: true });
  const sdk = await import("@earendil-works/pi-coding-agent");
  try {
    for (const role of ["assistant", "pm", "worker", "reviewer"] as Role[]) {
      const identity = { store: "st-0123456789abcdef", project: "demo", role,
        task: "T-007", run: "R1", review: "R1-review1", assistant: "A-1" };
      const modules = await loadModules(assets.dir, role);
      const transport = modules.auth.transport?.(authContext());
      if (!transport) throw new Error("fixture transport missing");
      const client = new CorralClient({ corral: "", root: work, cwd: work, identity });
      const host = await HostSession.create({ sdk, identity, cwd: work, runtimeDir: work, agentDir: join(work, `agent-${role}`),
        model: "gpt-6.1-sol", modules, client, events: new EventLog(), network: guardedFetch(transport),
        capabilities: { catalog: 8, digest: "d".repeat(64), preset: "standard", enabled: ["ask_user"] },
        sessionManager: sdk.SessionManager.create(work, join(work, `sessions-${role}`)) });
      expect(host.loadout).toContain("ask_user");
      // Structural contract inclusion, not a claim of semantic model compliance.
      for (const guideline of ASK_USER_GUIDELINES) {
        expect(host.session.systemPrompt).toContain(guideline);
      }
      await host.close();
    }
  } finally {
    assets.cleanup();
    rmSync(work, { recursive: true, force: true });
  }
}, 60_000);
