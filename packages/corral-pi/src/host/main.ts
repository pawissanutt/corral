// The Bun host entry point, exec'd by `corral agent launch` with `--launch-fd <n>`. It reads
// the launch message from that inherited pipe, refuses an unpinned runtime or a missing module,
// starts the bridge, binds the generation with hello and hands the pane to InteractiveMode, or,
// when the generation enables headless, attaches without a terminal UI (`./headless.ts`).
// A Bedrock host also adopts the AWS SDK's Node handlers into the guarded fetch, arms the socket
// tripwire and reads its named AWS profile before the SDK loads.
import { mkdirSync } from "node:fs";
import { ProcessTerminal } from "@earendil-works/pi-tui";
import { homedir } from "node:os";
import { dirname, join } from "node:path";

import { startBridge } from "../bridge/server.ts";
import { attestSourceDrain } from "../bridge/source-drain.ts";
import { EventLog } from "../bridge/events.ts";
import { COMMANDS } from "../bridge/protocol.ts";
import { CorralClient, sha256Hex } from "../tools/corral.ts";
import { readLaunchMessage, type HostConfig } from "./config.ts";
import type { BuildContext } from "../build/types.ts";
import { saveSession } from "../build/session-store.ts";
import type { HostIdentity } from "../roles/types.ts";
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
import { type AwsRoute, HostSession, authContext, processStartTime } from "./session.ts";
import { subscriptionRoutes } from "./subscriptions.ts";
import { guardTerminalStop } from "./terminal-stop.ts";
import { NEW_ASSISTANT_EXIT, RELINK_ASSISTANT_EXIT } from "./assistant-relink.ts";
import { RESUME_ASSISTANT_EXIT } from "./assistant-resume.ts";
import { CONTINUE_ASSISTANT_EXIT } from "./continuation.ts";
import { admitConnection } from "./connections.ts";
import { connectionRules, isApiConnection } from "../auth/connections.ts";
import { presentHostNotice } from "../ui/host-notices.ts";
import { errorPresenter } from "../ui/error-copy.ts";
import { RemoteOwner } from "./remote-owner.ts";

function launchFd(argv: readonly string[]): number {
  const index = argv.indexOf("--launch-fd");
  const value = index === -1 ? undefined : Number(argv[index + 1]);
  if (value === undefined || !Number.isSafeInteger(value) || value < 3) {
    throw new Error("launch_message_invalid: run by `corral agent launch` with --launch-fd <n>");
  }
  return value;
}

export async function main(): Promise<void> {
  await runHost(readLaunchMessage(launchFd(process.argv)));
}

export async function runHost(config: HostConfig, overrides: {
  client?: (identity: HostIdentity, sessionId: () => string) => CorralClient;
  bridge?: false;
  build?: BuildContext;
} = {}): Promise<void> {
  const provider = config.provider ?? "openai-codex";
  const connection = admitConnection(provider, config.binding, true);
  restrictEnvironment(process.env, provider);
  exportAssistantSession(process.env, config.identity);
  const compat = readCompat(config.assets);
  if (compiledBundle()) checkCompiledRuntime(compat);
  else checkRuntime(config.assets, compat);
  const modules = withRecoveryContext(await loadModules(config.assets, config.identity.role, provider));
  const events = new EventLog(config.identity.generation);
  const rawFetch = globalThis.fetch.bind(globalThis);
  const api = isApiConnection(connection);
  const providers = api ? [provider] : (await subscriptionRoutes(modules.auth, provider, config.identity.role)).providers;
  const send = modules.auth.transport?.(authContext(provider, connection)) ?? rawFetch;
  let aws: AwsRoute | undefined;
  let rules: readonly DestinationRule[] | undefined = api ? connectionRules(connection) : undefined;
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
  const network = guardedFetch(send, (destination) => events.emit("network_refused", { destination }), providers, rules,
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
  let physicalTerminal: ProcessTerminal | undefined;
  const restart = (code: number) => {
    // Leave the session callback before disposing its runtime.
    setImmediate(() => { void restartAssistant(code).catch((error: unknown) => {
      process.stderr.write(`corral: assistant handoff stopped: ${error instanceof Error ? error.message : String(error)}\n`);
      process.exit(2);
    }); });
  };
  const client = overrides.client?.(config.identity, () => host?.sessionId() ?? "") ?? new CorralClient({
    corral: config.corral, root: config.root, identity: config.identity, hostCap: config.secrets.hostCap,
    sessionId: () => host?.sessionId() ?? "",
  });
  host = await HostSession.create({
    sdk, identity: config.identity, cwd: config.cwd, agentDir, model: config.model, provider, ...(aws ? { aws } : {}),
    ...(config.binding ? { binding: config.binding } : {}),
    ...(config.daemonHosted ? { daemonHosted: true, ui: "plain" as const } : {}),
    ...(config.effort ? { effort: config.effort } : {}),
    ...(config.promptFile ? { promptFile: config.promptFile } : {}),
    ...(config.memoryFile ? { memoryFile: config.memoryFile } : {}),
    ...(!config.daemonHosted && config.ui ? { ui: config.ui } : {}),
    todo: config.todo === true,
    ...(config.capabilities ? { capabilities: config.capabilities } : {}),
    ...(config.guidance ? { guidance: config.guidance } : {}),
    root: config.root, exportDir: join(config.sessionDir, "exports"), runtimeDir: config.runtimeDir,
    modules, client, events, network,
    ...(overrides.build ? { build: overrides.build } : {}),
    ...(config.identity.role === "assistant" && process.argv.includes("--assistant-new")
      ? { relinkAssistant: () => restart(RELINK_ASSISTANT_EXIT),
        continueAssistant: () => restart(CONTINUE_ASSISTANT_EXIT),
        ...(!headlessEnabled(config.capabilities) ? { newAssistant: () => restart(NEW_ASSISTANT_EXIT),
          resumeAssistant: () => restart(RESUME_ASSISTANT_EXIT) } : {}),
      } : {}),
    ...(config.pane !== null && !config.daemonHosted
      ? herdrLifecycle(process.env, config.pane, config.identity, events, headlessEnabled(config.capabilities)) : {}),
    sessionManager: openSession(sdk, config.cwd, config.sessionDir, config.identity.generation, config.resumeSession, config.freshSession,
      config.resumeLeaf),
  });
  const session = host;
  const remote = config.daemonHosted ? new RemoteOwner(session, events, config.identity.assistant ?? "", config.runtimeDir) : undefined;
  let terminal: InstanceType<typeof sdk.InteractiveMode> | undefined;
  const bridge = overrides.bridge === false ? undefined : await startBridge({
    path: join(config.runtimeDir, "host.sock"),
    ctlCap: config.secrets.ctlCap,
    generation: config.identity.generation,
    uid: process.getuid?.() ?? -1,
    peer: await loadPeerLookup(),
    events,
    liveEvents: session.liveEvents.events,
    handlers: {
      hello: () => ({
        protocol: 1, generation: config.identity.generation, role: config.identity.role,
        bound: session.bound, session_id: session.sessionId(), last_seq: events.last,
        commands: COMMANDS.filter(command => (command !== "handoff_drain" || config.identity.role === "assistant")
          && (!["owner_turn", "set_model"].includes(command) || remote !== undefined)),
        daemon_hosted: remote !== undefined, remote_ready: remote?.isReady ?? false, turn: session.state().turn,
      }),
      state: () => session.state(),
      queueState: limit => session.queue.snapshot(limit, false),
      queueWithdraw: (key, op_key) => ({ ...session.queue.bridgeWithdraw(key, op_key) }),
      handoffDrain: args => attestSourceDrain(session, config.identity,
        { pid: process.pid, start_time: processStartTime() }, args),
      submit: (input, requestId) => remote && input.origin === "daemon-owner"
        ? Promise.resolve({ disposition: "rejected" as const, reason: "owner_turn_required" }) : session.submit(input, requestId),
      ...(remote ? {
        ownerTurn: async (args: Record<string, unknown>) => ({ ...await remote.submit(args) }),
        setModel: (args: Record<string, unknown>) => remote.setModel(args),
      } : {}),
      cancel: (turn) => {
        if (remote && turn === undefined) throw new Error("remote_stop_requires_turn");
        return session.cancel(turn);
      },
      questions: (updates) => session.questionsChanged(updates),
      shutdown: () => {
        events.emit("shutdown");
        terminal?.stop();
        void session.drained().then(() => session.close()).then(() => bridge?.close()).then(() => process.exit(0));
      },
    },
  });
  async function restartAssistant(code: number): Promise<void> {
    await physicalTerminal?.drainInput(1000);
    terminal?.stop();
    await session.close();
    await session.runtime.dispose();
    await session.drained();
    await bridge?.close();
    process.exit(code); // 75 = /new, 76 = relink, 77 = /resume, 78 = quota continuation request.
  }
  await session.hello(config.secrets.bindNonce, config.pane, sha256Hex(config.secrets.ctlCap));
  if (overrides.build) {
    const record = overrides.build.session;
    const state = session.state().session;
    if (!state || typeof state !== "object" || !("file" in state) || typeof state.file !== "string") {
      throw new Error("Build session has no transcript");
    }
    saveSession(dirname(dirname(overrides.build.dir)), { ...record, sessionFile: state.file, updated: new Date().toISOString() });
  }
  if (remote) {
    try { await remote.attach(); }
    catch (error) { await session.close(); await bridge?.close(); throw error; }
    return;
  }
  if (headlessEnabled(config.capabilities)) {
    await attachHeadless(session, events, (line) => process.stdout.write(line));
    return;
  }
  const panels = session.panels;
  physicalTerminal = new ProcessTerminal();
  terminal = new sdk.InteractiveMode(session.runtime, {
    // corral resumes sessions with its own commands; Pi's `pi --session …` line would name a command that does not work here.
    verbose: false, resumeHint: false, terminal: panels ? panels.wrap(physicalTerminal) : physicalTerminal,
    ...(panels ? { tuiMode: "fullscreen" as const, readOnlyQueue: true, mainView: (transcript, presentation) => panels.mainView(transcript, presentation),
      userPresentation: (text, bubble) => presentHostNotice(text, bubble, delivered => session.hostDelivered(delivered)),
      errorPresentation: errorPresenter(() => session.latestStopCause()), ...panels.presentation() } : {}),
  });
  guardTerminalStop(terminal, process.stdin);
  await terminal.init();
  session.markAttached("interactive");
  try { await terminal.run(); }
  finally { await session.close(); await bridge?.close(); }
}

if (import.meta.main) main().catch((error: unknown) => {
  const message = error instanceof Error ? error.message : String(error);
  process.stderr.write(`corral-pi host stopped: ${message}\n`);
  process.exit(2);
});
