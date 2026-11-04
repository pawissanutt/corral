// Offline provider only: the production HostSession talks to the built corral CLI and store.
// The ignored Rust private-herdr scenario runs this child through the real supervisor loop.
import { readFileSync } from "node:fs";
import { dirname } from "node:path";
import { z } from "zod";
import { EventLog } from "../../src/bridge/events.ts";
import { parseLaunchMessage } from "../../src/host/config.ts";
import { openSession } from "../../src/host/lifecycle.ts";
import { loadModules } from "../../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../../src/host/restrict.ts";
import { HostSession, authContext } from "../../src/host/session.ts";
import { RELINK_ASSISTANT_EXIT } from "../../src/host/assistant-relink.ts";
import { exportAssistantSession } from "../../src/roles/assistant/env.ts";
import { CorralClient, sha256Hex } from "../../src/tools/corral.ts";
import { nextEvent } from "./wait.ts";

const Readback = z.object({
  session: z.object({ id: z.string(), project: z.string(), linked_scratch: z.string().nullable() }),
  directory: z.object({ path: z.string(), branch: z.string().nullable() }),
});
const Transport = z.object({
  script: z.custom<(steps: readonly unknown[]) => void>(value => typeof value === "function"),
});

async function main(): Promise<void> {
  const config = parseLaunchMessage(readFileSync(0, "utf8"));
  restrictEnvironment(process.env);
  exportAssistantSession(process.env, config.identity);
  process.env.PATH = `${dirname(config.corral)}:${process.env.PATH ?? ""}`;
  process.chdir(config.cwd);
  const sdk = await import("@earendil-works/pi-coding-agent");
  const modules = await loadModules(config.assets, "assistant");
  const send = modules.auth.transport?.(authContext());
  if (!send) throw new Error("offline fixture transport missing");
  const network = guardedFetch(send, undefined, "openai-codex", undefined,
    config.capabilities?.web ? { settings: config.capabilities.web, send } : undefined,
    config.capabilities?.mcp ? { origins: [], send } : undefined);
  globalThis.fetch = network.fetch;
  let host: HostSession | undefined;
  let relink = false;
  const events = new EventLog(config.identity.generation);
  const client = new CorralClient({
    corral: config.corral, root: config.root, identity: config.identity,
    hostCap: config.secrets.hostCap, sessionId: () => host?.sessionId() ?? "",
  });
  try {
    host = await HostSession.create({
      sdk, identity: config.identity, cwd: config.cwd, root: config.root,
      agentDir: `${config.runtimeDir}/pi-agent`, runtimeDir: config.runtimeDir,
      model: config.model, modules, client, events, network, ui: "plain",
      capabilities: config.capabilities, guidance: config.guidance,
      sessionManager: openSession(sdk, config.cwd, config.sessionDir, config.identity.generation,
        config.resumeSession, config.freshSession),
      relinkAssistant: () => { relink = true; },
    });
    await host.hello(config.secrets.bindNonce, config.pane, sha256Hex(config.secrets.ctlCap));
    if (config.identity.generation === 1) {
      const holder: unknown = Reflect.get(globalThis, Symbol.for("corral-pi.fixture.transport"));
      Transport.parse(holder).script([
        { toolCalls: [{ callId: "scratch-create", name: "bash", arguments: {
          command: "corral assistant scratch new -p app --slug linked --json",
        } }] },
        { text: "Scratch created." },
      ]);
      await host.session.bindExtensions({});
      await host.session.prompt("Create the requested scratch and continue there.", { source: "interactive" });
    } else {
      const ended = nextEvent(events, event => event.kind === "turn_ended");
      await host.session.bindExtensions({});
      await ended;
    }
    await host.drained();
    const shown = Readback.parse(await client.todoSnapshot());
    console.log("ASSISTANT_PROJECT_HOST " + JSON.stringify({
      generation: config.identity.generation, identity: config.identity,
      cwd: config.cwd, session_id: host.sessionId(), session_file: host.session.sessionFile,
      relink, readback: shown,
    }));
    if (relink !== (config.identity.generation === 1)) throw new Error("wrong relink disposition");
  } finally {
    await host?.close();
  }
  process.exit(relink ? RELINK_ASSISTANT_EXIT : 0);
}

main().catch((error: unknown) => {
  console.error(error instanceof Error ? error.stack : String(error));
  process.exit(2);
});
