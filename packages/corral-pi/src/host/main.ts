// The Bun host entry point, exec'd by `corral agent launch` with `--launch-fd <n>`. It reads
// the launch message from that inherited pipe, refuses an unpinned runtime or a missing module,
// starts the bridge, binds the generation with hello and hands the pane to InteractiveMode.
import { mkdirSync } from "node:fs";
import { join } from "node:path";

import { startBridge } from "../bridge/server.ts";
import { EventLog } from "../bridge/events.ts";
import { CorralClient, sha256Hex } from "../tools/corral.ts";
import { readLaunchMessage } from "./config.ts";
import { loadModules } from "./modules.ts";
import { loadPeerLookup } from "./peer.ts";
import { guardedFetch, restrictEnvironment } from "./restrict.ts";
import { checkRuntime, readCompat } from "./runtime.ts";
import { HostSession, authContext } from "./session.ts";

function launchFd(argv: readonly string[]): number {
  const index = argv.indexOf("--launch-fd");
  const value = index === -1 ? undefined : Number(argv[index + 1]);
  if (value === undefined || !Number.isSafeInteger(value) || value < 3) {
    throw new Error("launch_message_invalid: run by `corral agent launch` with --launch-fd <n>");
  }
  return value;
}

async function main(): Promise<void> {
  const config = readLaunchMessage(launchFd(process.argv));
  restrictEnvironment(process.env);
  checkRuntime(config.assets, readCompat(config.assets));
  const modules = await loadModules(config.assets, config.identity.role);
  const events = new EventLog(config.identity.generation);
  const send = modules.auth.transport?.(authContext()) ?? globalThis.fetch.bind(globalThis);
  const network = guardedFetch(send, (destination) => events.emit("network_refused", { destination }));
  globalThis.fetch = network.fetch as typeof globalThis.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  process.chdir(config.cwd);
  mkdirSync(config.sessionDir, { recursive: true, mode: 0o700 });
  const agentDir = join(config.runtimeDir, "pi-agent");
  mkdirSync(agentDir, { recursive: true, mode: 0o700 });
  let host: HostSession | undefined;
  const client = new CorralClient({
    corral: config.corral, root: config.root, identity: config.identity, hostCap: config.secrets.hostCap,
    sessionId: () => host?.sessionId() ?? "",
  });
  host = await HostSession.create({
    sdk, identity: config.identity, cwd: config.cwd, agentDir, model: config.model,
    ...(config.effort ? { effort: config.effort } : {}),
    ...(config.promptFile ? { promptFile: config.promptFile } : {}),
    modules, client, events, network, sessionManager: sdk.SessionManager.create(config.cwd, config.sessionDir),
  });
  const session = host;
  let terminal: InstanceType<typeof sdk.InteractiveMode> | undefined;
  const bridge = await startBridge({
    path: join(config.runtimeDir, "host.sock"),
    ctlCap: config.secrets.ctlCap,
    generation: config.identity.generation,
    uid: process.getuid?.() ?? -1,
    peer: await loadPeerLookup(),
    events,
    handlers: {
      hello: () => ({
        protocol: 1, generation: config.identity.generation, role: config.identity.role,
        bound: session.bound, session_id: session.sessionId(), last_seq: events.last,
        commands: ["hello", "get_state", "submit_input", "cancel", "events_since", "shutdown"],
      }),
      state: () => session.state(),
      submit: (input, requestId) => session.submit(input, requestId),
      cancel: (turn) => session.cancel(turn),
      shutdown: () => {
        events.emit("shutdown");
        terminal?.stop();
        void session.drained().then(() => bridge.close()).then(() => process.exit(0));
      },
    },
  });
  await session.hello(config.secrets.bindNonce, config.pane, sha256Hex(config.secrets.ctlCap));
  terminal = new sdk.InteractiveMode(session.runtime, { verbose: false });
  await terminal.init();
  session.markInteractive();
  await terminal.run();
}

main().catch((error: unknown) => {
  const message = error instanceof Error ? error.message : String(error);
  process.stderr.write(`corral-pi host stopped: ${message}\n`);
  process.exit(2);
});
