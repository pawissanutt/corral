import { expect, test } from "bun:test";
import { cpSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { EventLog } from "../bridge/events.ts";
import type { Role } from "../roles/types.ts";
import { CorralClient } from "../tools/corral.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "../../test/support/fixture-install.ts";
import { writeFakeCorral } from "../../test/support/fake-corral.ts";
import { loadModules } from "./modules.ts";
import { guardedFetch } from "./restrict.ts";
import { authContext, HostSession } from "./session.ts";
import { ASK_USER_GUIDELINES } from "../roles/common/ask-user.ts";

test("real role host loadouts expose ask_user only to assistant and PM", async () => {
  const assets = buildFixtureInstall({ omit: ["role"] });
  const work = mkdtempSync(join(tmpdir(), "ask-user-loadouts-"));
  cpSync(join(PACKAGE_ROOT, "assets"), join(assets.dir, "assets"), { recursive: true });
  const sdk = await import("@earendil-works/pi-coding-agent");
  try {
    for (const role of ["assistant", "pm", "worker", "reviewer"] as Role[]) {
      const identity = { store: "st-0123456789abcdef", project: "demo", role, generation: 1,
        task: "T-007", run: "R1", review: "R1-review1", assistant: "A-1", callback: "g1:c1" };
      const modules = await loadModules(assets.dir, role);
      const transport = modules.auth.transport?.(authContext());
      if (!transport) throw new Error("fixture transport missing");
      let host: HostSession;
      const client = new CorralClient({ corral: writeFakeCorral(work).path, root: work, identity,
        hostCap: "h".repeat(64), sessionId: () => host.sessionId() });
      host = await HostSession.create({ sdk, identity, cwd: work, agentDir: join(work, `agent-${role}`),
        model: "gpt-6.1-sol", modules, client, events: new EventLog(1), network: guardedFetch(transport),
        capabilities: { catalog: 8, digest: "d".repeat(64), preset: "standard", enabled: ["ask_user"] },
        sessionManager: sdk.SessionManager.create(work, join(work, `sessions-${role}`)) });
      const interactive = role === "assistant" || role === "pm";
      expect(host.loadout.includes("ask_user")).toBe(interactive);
      // Structural contract inclusion, not a claim of semantic model compliance.
      for (const guideline of ASK_USER_GUIDELINES) {
        expect(host.session.systemPrompt.includes(guideline)).toBe(interactive);
      }
      await host.close();
    }
  } finally {
    assets.cleanup();
    rmSync(work, { recursive: true, force: true });
  }
}, 60_000);
