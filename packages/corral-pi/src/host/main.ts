// Runs the local Pi runtime and its session socket.
// A Bedrock host also adopts the AWS SDK's Node handlers into the guarded fetch, arms the socket
// tripwire and reads its named AWS profile before the SDK loads.
import { mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";

import { preparePrivateDirectory, startBridge, type BridgeHandlers, type BridgeServer } from "../bridge/server.ts";
import { EventLog } from "../bridge/events.ts";
import { BRIDGE_VERSION } from "../bridge/protocol.ts";
import { CorralClient } from "../tools/corral.ts";
import type { HostConfig } from "./config.ts";
import type { BuildContext } from "../build/types.ts";
import { saveSession } from "../build/session-store.ts";
import { inheritTranscript, openSession, siblingTranscript } from "./lifecycle.ts";
import { loadModules } from "./modules.ts";
import { CredentialsUnavailable } from "../auth/paths.ts";
import { type DestinationRule, PROVIDERS, hostRules } from "../auth/providers.ts";
import { localInferenceRules } from "../auth/local.ts";
import { NetworkRefused, guardedFetch, restrictEnvironment } from "./restrict.ts";
import { compiledBundle } from "./bundle.ts";
import { attachHeadless, headlessEnabled } from "./headless.ts";
import { herdrLifecycle } from "./herdr.ts";
import { serverVisible } from "./mcp-settings.ts";
import { type AwsRoute, HostSession, authContext } from "./session.ts";
import { subscriptionRoutes } from "./subscriptions.ts";
import { guardTerminalStop } from "./terminal-stop.ts";
import { admitConnection } from "./connections.ts";
import { connectionRules, isApiConnection } from "../auth/connections.ts";
import { errorPresenter } from "../ui/error-copy.ts";
import { repaintTranscript } from "../ui/transcript-repaint.ts";
import { presentHostNotice } from "../ui/host-notices.ts";
import { RemoteOwner } from "./remote-owner.ts";
import { SessionRecord } from "./session-record.ts";
import { summaryOptions } from "./summary.ts";
import { bareProviders, requireBareProvider } from "./bare.ts";
import { providerTransport } from "./capabilities.ts";

interface HostOverrides {
  build?: BuildContext;
}

export async function runHost(config: HostConfig, overrides: HostOverrides = {}): Promise<void> {
  const events = new EventLog();
  const id = config.local?.id ?? config.identity.assistant
    ?? [config.identity.task, config.identity.run].filter(Boolean).join("/");
  preparePrivateDirectory(config.local.dir);
  const bridge = await startBridge({ path: config.local.socket, sessionId: id || config.identity.role, events });
  try {
    const record = new SessionRecord(config.local.dir, id, config.identity.role, config.cwd);
    await runSession(config, overrides, events, bridge, record);
  } catch (error) {
    await bridge.close();
    throw error;
  }
}

async function runSession(config: HostConfig, overrides: HostOverrides, events: EventLog,
  bridge: BridgeServer, record: SessionRecord): Promise<void> {
  const provider = config.provider ?? "openai-codex";
  if (overrides.build?.session.bare) requireBareProvider(provider);
  const connection = admitConnection(provider, config.binding, true);
  restrictEnvironment(process.env, provider);
  if (config.summary?.provider === "amazon-bedrock") restrictEnvironment(process.env, "amazon-bedrock");
  process.env.CORRAL_NEXT_ROOT = config.root;
  if (config.corral) process.env.CORRAL = config.corral;
  else delete process.env.CORRAL;
  process.env.CORRAL_ACTOR = config.identity.role === "worker" ? "agent" : config.identity.role;
  const compiled = compiledBundle();
  const assets = config.local && compiled?.writeAssets ? join(config.runtimeDir, "bundle") : config.assets;
  if (config.local) compiled?.writeAssets?.(assets);
  const agentDir = join(config.runtimeDir, "pi-agent");
  mkdirSync(agentDir, { recursive: true, mode: 0o700 });
  if (config.local) process.env.PI_CODING_AGENT_DIR ??= agentDir;
  const modules = await loadModules(assets, config.identity.role, provider);
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
  let terminal: InstanceType<typeof sdk.InteractiveMode> | undefined;
  const client = new CorralClient({ corral: config.corral, root: config.root, cwd: config.cwd, identity: config.identity });
  const pane = config.pane || process.env.HERDR_PANE_ID;
  let transcript = record.transcript;
  if (!transcript && config.identity.role === "worker" && !config.freshSession) {
    const sibling = siblingTranscript(config.local.dir);
    if (sibling) transcript = inheritTranscript(sibling, config.sessionDir);
  }
  host = await HostSession.create({
    sdk, identity: config.identity, cwd: config.cwd, agentDir, model: config.model, provider, ...(aws ? { aws } : {}),
    ...(config.binding ? { binding: config.binding } : {}),
    paneInput: async (text, images) => await remote?.paneInput(text, images) ?? false,
    transcriptRewound: () => { if (terminal) repaintTranscript(terminal); },
    automatedInput: input => remote?.submit(input),
    ...(config.effort ? { effort: config.effort } : {}),
    quickSummary: config.quickSummary,
    ...(summary ? { summary } : {}),
    ...(config.promptFile ? { promptFile: config.promptFile } : {}),
    ...(config.ui ? { ui: config.ui } : {}),
    todo: config.todo === true,
    ...(config.capabilities ? { capabilities: config.capabilities } : {}),
    ...(config.guidance ? { guidance: config.guidance } : {}),
    root: config.root, exportDir: join(config.sessionDir, "exports"), runtimeDir: config.runtimeDir,
    modules, client, events, network,
    ...(overrides.build ? { build: overrides.build } : {}),
    ...(pane ? herdrLifecycle(process.env, pane, config.identity, events, config.local?.headless || headlessEnabled(config.capabilities)) : {}),
    sessionManager: openSession(sdk, config.cwd, config.sessionDir, transcript),
  });
  const session = host;
  let closing: Promise<void> | undefined;
  const close = () => closing ??= (async () => {
    try { await session.close(); await session.drained(); }
    finally { await bridge?.close(); }
  })();
  const sessionFile = session.session.sessionFile;
  if (!sessionFile) throw new Error("the Pi session has no transcript");
  const owner = new RemoteOwner(session, events, record, sessionFile, config.runtimeDir);
  remote = owner;
  await session.startLocal();
  if (overrides.build) {
    saveSession(dirname(dirname(overrides.build.dir)), { ...overrides.build.session, sessionFile, updated: new Date().toISOString() });
  }
  let handlers: BridgeHandlers | undefined;
  {
    record.save(sessionFile);
    handlers = {
      hello: () => ({
        protocol: BRIDGE_VERSION, role: config.identity.role, session_id: config.local.id,
        session_file: sessionFile, pid: process.pid,
      }),
      state: () => {
        const { identity: _identity, ...state } = session.state();
        return { ...state, paused: owner.paused, owner_questions: session.pendingOwnerQuestions() };
      },
      submit: input => owner.submit(input),
      answer: input => owner.answer(input),
      pause: () => owner.pause(),
      resume: () => owner.resume(),
      setModel: args => owner.setModel(args),
      cancel: turn => session.cancel(turn),
      rewind: input => owner.rewind(input),
      queueState: limit => session.queue.snapshot(limit),
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
  if (config.local?.headless || headlessEnabled(config.capabilities)) {
    await attachHeadless(session, events, line => process.stdout.write(line));
    if (handlers) {
      bridge.attach(handlers, session.liveEvents);
      events.emit("ready");
    }
    return;
  }
  const panels = session.panels;
  const { ProcessTerminal } = await import("@earendil-works/pi-tui");
  const physicalTerminal = new ProcessTerminal();
  terminal = new sdk.InteractiveMode(session.runtime, {
    // corral resumes sessions with its own commands; Pi's `pi --session …` line would name a command that does not work here.
    verbose: false, resumeHint: false, terminal: panels ? panels.wrap(physicalTerminal) : physicalTerminal,
    ...(panels ? { tuiMode: "fullscreen" as const, readOnlyQueue: true, mainView: (transcript, presentation) => panels.mainView(transcript, presentation),
      errorPresentation: errorPresenter(() => session.latestStopCause()), userPresentation: presentHostNotice, ...panels.presentation() } : {}),
  });
  guardTerminalStop(terminal, process.stdin);
  await terminal.init();
  session.markAttached("interactive");
  if (handlers) bridge.attach(handlers, session.liveEvents);
  try { await terminal.run(); }
  finally { await close(); }
}

