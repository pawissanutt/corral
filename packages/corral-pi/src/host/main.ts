// Runs the same Pi runtime for flag launches and the old direct source entry.
// A Bedrock host also adopts the AWS SDK's Node handlers into the guarded fetch, arms the socket
// tripwire and reads its named AWS profile before the SDK loads.
import { mkdirSync } from "node:fs";
import { ProcessTerminal } from "@earendil-works/pi-tui";
import { homedir } from "node:os";
import { dirname, join } from "node:path";

import { startBridge, type BridgeHandlers, type BridgeServer } from "../bridge/server.ts";
import { EventLog } from "../bridge/events.ts";
import { BRIDGE_VERSION } from "../bridge/protocol.ts";
import { CorralClient, sha256Hex } from "../tools/corral.ts";
import { readLaunchMessage, type HostConfig } from "./config.ts";
import type { BuildContext } from "../build/types.ts";
import { LocalControlPlane } from "../build/control-plane.ts";
import { createTodoLedger } from "../build/todo-ledger.ts";
import { saveSession } from "../build/session-store.ts";
import type { HostIdentity } from "../roles/types.ts";
import { openSession, withRecoveryContext } from "./lifecycle.ts";
import { loadModules } from "./modules.ts";
import { CredentialsUnavailable } from "../auth/paths.ts";
import { type DestinationRule, PROVIDERS, hostRules } from "../auth/providers.ts";
import { localInferenceRules } from "../auth/local.ts";
import { NetworkRefused, guardedFetch, restrictEnvironment } from "./restrict.ts";
import { compiledBundle } from "./bundle.ts";
import { attachHeadless, headlessEnabled } from "./headless.ts";
import { herdrLifecycle } from "./herdr.ts";
import { checkRuntime } from "./runtime.ts";
import { serverVisible } from "./mcp-settings.ts";
import { exportAssistantSession } from "../roles/assistant/env.ts";
import { type AwsRoute, HostSession, authContext } from "./session.ts";
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
import { SessionRecord } from "./session-record.ts";
import { summaryOptions } from "./summary.ts";
import { bareProviders, requireBareProvider } from "./bare.ts";
import { providerTransport } from "./capabilities.ts";
import { PROFILE_EXIT } from "./profiles.ts";

function launchFd(argv: readonly string[]): number {
  const index = argv.indexOf("--launch-fd");
  const value = index === -1 ? undefined : Number(argv[index + 1]);
  if (value === undefined || !Number.isSafeInteger(value) || value < 3) {
    throw new Error("launch_message_invalid: run by `corral agent launch` with --launch-fd <n>");
  }
  return value;
}

export async function main(): Promise<void> {
  if (!compiledBundle()) checkRuntime();
  await runHost(readLaunchMessage(launchFd(process.argv)));
}

interface HostOverrides {
  client?: (identity: HostIdentity, sessionId: () => string) => CorralClient;
  bridge?: false;
  build?: BuildContext;
}

export async function runHost(config: HostConfig, overrides: HostOverrides = {}): Promise<void> {
  const events = new EventLog(config.identity.generation);
  const id = config.local?.id ?? config.identity.assistant
    ?? [config.identity.task, config.identity.run].filter(Boolean).join("/");
  const bridge = overrides.bridge === false ? undefined : await startBridge({
    path: config.local?.socket ?? join(config.runtimeDir, "host.sock"), sessionId: id || config.identity.role, events,
  });
  try {
    const record = bridge ? new SessionRecord(config.local?.dir ?? config.runtimeDir, id, config.identity.role, config.cwd) : undefined;
    await runSession(config, overrides, events, bridge, record);
  } catch (error) {
    await bridge?.close();
    throw error;
  }
}

async function runSession(config: HostConfig, overrides: HostOverrides, events: EventLog,
  bridge: BridgeServer | undefined, record: SessionRecord | undefined): Promise<void> {
  const provider = config.provider ?? "openai-codex";
  if (overrides.build?.session.bare) requireBareProvider(provider);
  const connection = admitConnection(provider, config.binding, true);
  restrictEnvironment(process.env, provider);
  if (config.summary?.provider === "amazon-bedrock") restrictEnvironment(process.env, "amazon-bedrock");
  exportAssistantSession(process.env, config.identity);
  const compiled = compiledBundle();
  const assets = config.local && compiled?.writeAssets ? join(config.runtimeDir, "bundle") : config.assets;
  if (config.local) compiled?.writeAssets?.(assets);
  const agentDir = join(config.runtimeDir, "pi-agent");
  mkdirSync(agentDir, { recursive: true, mode: 0o700 });
  if (config.local) process.env.PI_CODING_AGENT_DIR ??= agentDir;
  const modules = withRecoveryContext(await loadModules(assets, config.identity.role, provider));
  const rawFetch = globalThis.fetch.bind(globalThis);
  const api = isApiConnection(connection);
  const bare = overrides.build?.session.bare ? await bareProviders(modules.auth, provider) : undefined;
  const providers = bare?.providers ?? (api ? [provider] : (await subscriptionRoutes(modules.auth, provider, config.identity.role)).providers);
  const send = bare ? providerTransport(modules.auth, bare.providers, rawFetch)
    : modules.auth.transport?.(authContext(provider, connection)) ?? rawFetch;
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
  const summary = await summaryOptions(config, modules, rawFetch);
  if (aws || summary?.awsRoute) {
    const { installSocketTripwire } = await import("./smithy.ts");
    installSocketTripwire((destination) => network.refuse?.(destination) ?? new NetworkRefused(destination));
  }
  globalThis.fetch = summary?.awsRoute ? guardedFetch(rawFetch, undefined, providers,
    [...(rules ?? providers.flatMap(id => hostRules(id))), ...summary.rules]).fetch as typeof globalThis.fetch
    : network.fetch as typeof globalThis.fetch;
  // An unusable profile or an expired SSO session stops the host here, before any session exists.
  if (aws) await aws.resolver.get();
  const sdk = await import("@earendil-works/pi-coding-agent");
  process.chdir(config.cwd);
  mkdirSync(config.sessionDir, { recursive: true, mode: 0o700 });
  let remote: RemoteOwner | undefined;
  let host: HostSession | undefined;
  let physicalTerminal: ProcessTerminal | undefined;
  const restart = (code: number) => {
    // Leave the session callback before disposing its runtime.
    setImmediate(() => { void restartAssistant(code).catch((error: unknown) => {
      process.stderr.write(`corral: assistant handoff stopped: ${error instanceof Error ? error.message : String(error)}\n`);
      process.exit(2);
    }); });
  };
  const sessionId = () => host?.sessionId() ?? "";
  const client = overrides.client?.(config.identity, sessionId) ?? (config.secrets
    ? new CorralClient({ corral: config.corral, root: config.root, identity: config.identity, hostCap: config.secrets.hostCap, sessionId })
    : (() => {
      // Reuse build's host-local adapter until the remaining tool calls move to engine argv.
      const dir = config.local?.dir ?? config.runtimeDir;
      const ledger = createTodoLedger(dir);
      return new LocalControlPlane({ dir, root: config.root, identity: config.identity, sessionId,
        handlers: { todo: ledger.handle }, todoSnapshot: () => ledger.snapshot() });
    })());
  host = await HostSession.create({
    sdk, identity: config.identity, cwd: config.cwd, agentDir, model: config.model, provider, ...(aws ? { aws } : {}),
    ...(config.binding ? { binding: config.binding } : {}),
    ...(config.daemonHosted ? { daemonHosted: true, ui: "plain" as const } : {}),
    ...(config.local ? { local: { paneInput: async (text, images) => await remote?.paneInput(text, images) ?? false } } : {}),
    ...(config.effort ? { effort: config.effort } : {}),
    quickSummary: config.quickSummary,
    ...(summary ? { summary } : {}),
    ...(config.promptFile ? { promptFile: config.promptFile } : {}),
    ...(config.memoryFile ? { memoryFile: config.memoryFile } : {}),
    ...(!config.daemonHosted && config.ui ? { ui: config.ui } : {}),
    todo: config.todo === true,
    ...(config.capabilities ? { capabilities: config.capabilities } : {}),
    ...(config.guidance ? { guidance: config.guidance } : {}),
    root: config.root, exportDir: join(config.sessionDir, "exports"), runtimeDir: config.runtimeDir,
    modules, client, events, network,
    ...(overrides.build ? { build: overrides.build } : {}),
    ...(!config.daemonHosted && (overrides.build || config.identity.role === "assistant" || config.identity.role === "pm")
      ? { switchProfile: () => restart(PROFILE_EXIT) } : {}),
    ...(config.identity.role === "assistant" && process.argv.includes("--assistant-new")
      ? { relinkAssistant: () => restart(RELINK_ASSISTANT_EXIT),
        continueAssistant: () => restart(CONTINUE_ASSISTANT_EXIT),
        ...(!headlessEnabled(config.capabilities) ? { newAssistant: () => restart(NEW_ASSISTANT_EXIT),
          resumeAssistant: () => restart(RESUME_ASSISTANT_EXIT) } : {}),
      } : {}),
    ...(config.pane !== null && !config.daemonHosted
      ? herdrLifecycle(process.env, config.pane, config.identity, events, headlessEnabled(config.capabilities)) : {}),
    sessionManager: openSession(sdk, config.cwd, config.sessionDir, config.identity.generation,
      config.resumeSession ?? (config.local ? record?.transcript : undefined), config.freshSession, config.resumeLeaf),
  });
  const session = host;
  let terminal: InstanceType<typeof sdk.InteractiveMode> | undefined;
  let closing: Promise<void> | undefined;
  const close = () => closing ??= (async () => {
    try { await session.close(); await session.drained(); }
    finally { await bridge?.close(); }
  })();
  if (config.secrets) await session.hello(config.secrets.bindNonce, config.pane, sha256Hex(config.secrets.ctlCap));
  else await session.startLocal();
  const sessionFile = session.session.sessionFile;
  if (!sessionFile) throw new Error("the Pi session has no transcript");
  if (overrides.build) {
    saveSession(dirname(dirname(overrides.build.dir)), { ...overrides.build.session, sessionFile, updated: new Date().toISOString() });
  }
  let handlers: BridgeHandlers | undefined;
  if (record) {
    record.save(sessionFile);
    const owner = new RemoteOwner(session, events, record, sessionFile, config.runtimeDir);
    remote = owner;
    handlers = {
      hello: () => ({
        protocol: BRIDGE_VERSION, role: config.identity.role, session_id: config.local?.id ?? session.sessionId(),
        session_file: sessionFile, pid: process.pid,
      }),
      state: () => {
        const { generation: _generation, identity: _identity, ...state } = session.state();
        return { ...state, paused: owner.paused, owner_questions: session.pendingOwnerQuestions() };
      },
      submit: input => owner.submit(input),
      answer: input => owner.answer(input),
      pause: () => owner.pause(),
      resume: () => owner.resume(),
      setModel: args => owner.setModel(args),
      cancel: turn => session.cancel(turn),
      queueState: limit => session.queue.snapshot(limit, "socket"),
      queueWithdraw: key => session.queue.withdrawInput(key),
      shutdown() {
        events.emit("shutdown");
        terminal?.stop();
        void close().then(() => process.exit(0), error => {
          process.stderr.write(`corral-pi shutdown failed: ${String(error)}\n`);
          process.exit(2);
        });
      },
    };
  }
  async function restartAssistant(code: number): Promise<void> {
    await physicalTerminal?.drainInput(1000);
    terminal?.stop();
    await session.close();
    await session.runtime.dispose();
    await session.drained();
    await bridge?.close();
    process.exit(code); // 75 /new, 76 relink, 77 /resume, 78 quota continuation, 79 profile selection.
  }
  if (config.local?.headless || config.daemonHosted || headlessEnabled(config.capabilities)) {
    await attachHeadless(session, events, line => process.stdout.write(line));
    if (handlers) {
      bridge?.attach(handlers, session.liveEvents);
      events.emit("ready");
    }
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
  if (handlers) bridge?.attach(handlers, session.liveEvents);
  try { await terminal.run(); }
  finally { await close(); }
}

if (import.meta.main) main().catch((error: unknown) => {
  const message = error instanceof Error ? error.message : String(error);
  process.stderr.write(`corral-pi host stopped: ${message}\n`);
  process.exit(2);
});
