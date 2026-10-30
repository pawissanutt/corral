// The Bun host entry point, exec'd by `corral agent launch` with `--launch-fd <n>`. It reads
// the launch message from that inherited pipe, refuses an unpinned runtime or a missing module,
// starts the bridge, binds the generation with hello and hands the pane to InteractiveMode, or,
// when the generation enables headless, attaches without a terminal UI (`./headless.ts`).
// A Bedrock host also adopts the AWS SDK's Node handlers into the guarded fetch, arms the socket
// tripwire and reads its named AWS profile before the SDK loads.
import { mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

import { startBridge } from "../bridge/server.ts";
import { EventLog } from "../bridge/events.ts";
import { CorralClient, sha256Hex } from "../tools/corral.ts";
import { readLaunchMessage } from "./config.ts";
import { openSession, withRecoveryContext } from "./lifecycle.ts";
import { loadModules } from "./modules.ts";
import { loadPeerLookup } from "./peer.ts";
import { CredentialsUnavailable } from "../auth/paths.ts";
import { type DestinationRule, PROVIDERS, hostRules } from "../auth/providers.ts";
import { localInferenceRules } from "../auth/local.ts";
import { NetworkRefused, guardedFetch, restrictEnvironment } from "./restrict.ts";
import { compiledBundle } from "./bundle.ts";
import { attachHeadless, headlessEnabled } from "./headless.ts";
import { herdrLifecycle } from "./herdr.ts";
import { checkCompiledRuntime, checkRuntime, readCompat } from "./runtime.ts";
import { serverVisible } from "./mcp-settings.ts";
import { exportAssistantSession } from "../roles/assistant/env.ts";
import { type AwsRoute, HostSession, authContext } from "./session.ts";
import { subscriptionRoutes } from "./subscriptions.ts";
import { guardTerminalStop } from "./terminal-stop.ts";

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
  const provider = config.provider ?? "openai-codex";
  restrictEnvironment(process.env, provider);
  exportAssistantSession(process.env, config.identity);
  const compat = readCompat(config.assets);
  if (compiledBundle()) checkCompiledRuntime(compat);
  else checkRuntime(config.assets, compat);
  const modules = withRecoveryContext(await loadModules(config.assets, config.identity.role, provider));
  const events = new EventLog(config.identity.generation);
  const rawFetch = globalThis.fetch.bind(globalThis);
  const subscriptions = await subscriptionRoutes(modules.auth, provider, config.identity.role);
  const send = modules.auth.transport?.(authContext(provider)) ?? rawFetch;
  let aws: AwsRoute | undefined;
  let rules: readonly DestinationRule[] | undefined;
  if (provider === "llama.cpp" && config.capabilities?.llama_cpp) {
    rules = localInferenceRules(config.capabilities.llama_cpp);
  }
  if (PROVIDERS[provider].auth === "aws-profile") {
    if (!config.aws) {
      throw new CredentialsUnavailable(`no Amazon Bedrock binding was passed for provider ${provider}; ` +
        `run \`corral agent login --provider ${provider}\``);
    }
    const [{ bedrockRoute, createAwsCredentialResolver }, { adoptSmithyHandlers }] =
      await Promise.all([import("../auth/aws.ts"), import("./smithy.ts")]);
    adoptSmithyHandlers();
    const resolver = createAwsCredentialResolver(config.aws, homedir());
    const model = config.model.startsWith(`${provider}/`) ? config.model.slice(provider.length + 1) : config.model;
    rules = hostRules(provider, bedrockRoute(config.aws, model, await resolver.profile()));
    aws = { binding: config.aws, resolver };
  }
  const network = guardedFetch(send, (destination) => events.emit("network_refused", { destination }), subscriptions.providers, rules,
    config.capabilities?.web ? { settings: config.capabilities.web, send: rawFetch } : undefined,
    config.capabilities?.mcp ? {
      origins: Object.values(config.capabilities.mcp.servers)
        .flatMap((server) => serverVisible(server, config.identity.role, config.identity.project) && server.url
          ? [new URL(server.url).origin] : []),
      send: rawFetch,
    } : undefined);
  if (aws) {
    const { installSocketTripwire } = await import("./smithy.ts");
    installSocketTripwire((destination) => network.refuse?.(destination) ?? new NetworkRefused(destination));
  }
  globalThis.fetch = network.fetch as typeof globalThis.fetch;
  // An unusable profile or an expired SSO session stops the host here, before any session exists.
  if (aws) await aws.resolver.get();
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
    sdk, identity: config.identity, cwd: config.cwd, agentDir, model: config.model, provider, ...(aws ? { aws } : {}),
    ...(config.effort ? { effort: config.effort } : {}),
    ...(config.promptFile ? { promptFile: config.promptFile } : {}),
    ...(config.ui ? { ui: config.ui } : {}),
    todo: config.todo === true,
    ...(config.capabilities ? { capabilities: config.capabilities } : {}),
    ...(config.guidance ? { guidance: config.guidance } : {}),
    root: config.root, exportDir: join(config.sessionDir, "exports"), runtimeDir: config.runtimeDir,
    modules, client, events, network,
    ...herdrLifecycle(process.env, config.pane, config.identity, events, headlessEnabled(config.capabilities)),
    sessionManager: openSession(sdk, config.cwd, config.sessionDir, config.identity.generation, config.resumeSession, config.freshSession,
      config.resumeLeaf),
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
        commands: ["hello", "get_state", "submit_input", "cancel", "events_since", "shutdown", "questions"],
      }),
      state: () => session.state(),
      submit: (input, requestId) => session.submit(input, requestId),
      cancel: (turn) => session.cancel(turn),
      questions: (updates) => session.questionsChanged(updates),
      shutdown: () => {
        events.emit("shutdown");
        terminal?.stop();
        void session.drained().then(() => session.close()).then(() => bridge.close()).then(() => process.exit(0));
      },
    },
  });
  await session.hello(config.secrets.bindNonce, config.pane, sha256Hex(config.secrets.ctlCap));
  if (headlessEnabled(config.capabilities)) {
    await attachHeadless(session, events, (line) => process.stdout.write(line));
    return;
  }
  terminal = new sdk.InteractiveMode(session.runtime, { verbose: false });
  guardTerminalStop(terminal, process.stdin);
  await terminal.init();
  session.markAttached("interactive");
  try { await terminal.run(); }
  finally { await session.close(); await bridge.close(); }
}

main().catch((error: unknown) => {
  const message = error instanceof Error ? error.message : String(error);
  process.stderr.write(`corral-pi host stopped: ${message}\n`);
  process.exit(2);
});
