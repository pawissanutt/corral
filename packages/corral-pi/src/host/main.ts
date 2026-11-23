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
import { inheritTranscript, openSession, restoreModelChoice, SessionResumeRefused, siblingTranscript } from "./lifecycle.ts";
import { loadModules } from "./modules.ts";
import { CredentialsUnavailable } from "../auth/paths.ts";
import { type DestinationRule, PROVIDERS, type ProviderId, hostRules, isApiProvider, isProviderId } from "../auth/providers.ts";
import { fallbackProvidersFor, restorableFallbacks } from "./fallback-settings.ts";
import { localInferenceRules } from "../auth/local.ts";
import { NetworkRefused, guardedFetch, restrictEnvironment } from "./restrict.ts";
import { compiledBundle } from "./bundle.ts";
import { attachHeadless, headlessEnabled } from "./headless.ts";
import { herdrLifecycle } from "./herdr.ts";
import { serverVisible } from "./mcp-settings.ts";
import { type AwsRoute, HostSession, authContext } from "./session.ts";
import { guardTerminalStop } from "./terminal-stop.ts";
import { errorPresenter } from "../ui/error-copy.ts";
import { repaintTranscript } from "../ui/transcript-repaint.ts";
import { presentHostNotice } from "../ui/host-notices.ts";
import { RemoteOwner } from "./remote-owner.ts";
import { installRemote } from "./remote/index.ts";
import { SessionRecord } from "./session-record.ts";
import { summaryOptions } from "./summary.ts";
import { childProfileProviders, interactiveModels, providerTransport, sessionRoutes } from "./capabilities.ts";
import { readBuildProfiles } from "../build/profiles.ts";
import { resolveStartupModel } from "./startup-model.ts";

import { writeCodeSeed, type CodeBootstrap, type CodeHandoff, type CodeHandoffRequest, type CodeLaunch } from "./code-handoff.ts";

interface HostOverrides {
  codeBootstrap?: CodeBootstrap;
  prepareHandoff?: (request: CodeHandoffRequest, model: string, effort: string) => Promise<CodeLaunch>;
  commitHandoff?: (next: CodeLaunch) => Promise<void>;
  build?: BuildContext;
}

export async function runHost(config: HostConfig, overrides: HostOverrides = {}): Promise<void> {
  const events = new EventLog();
  const id = config.local?.id ?? config.identity.assistant
    ?? [config.identity.task, config.identity.run].filter(Boolean).join("/");
  const openRecord = (socket?: string) => new SessionRecord(config.local.dir, id, config.identity.role, config.cwd,
    config.terminalChat && config.freshSession, socket);
  const existing = config.terminalChat && openRecord().transcript;
  if (config.terminalChat && !existing) {
    const choice = await resolveStartupModel({ ...config, role: "assistant",
      headless: config.local.headless || !process.stdout.isTTY || headlessEnabled(config.capabilities),
      workspace: { root: config.root, corral: config.corral, model: config.startupModel ?? null },
      local: config.capabilities?.llama_cpp });
    if (!choice) return;
    config = { ...config, ...choice };
  }
  preparePrivateDirectory(config.local.dir);
  const bridge = await startBridge({ path: config.local.socket, sessionId: id || config.identity.role, events });
  try {
    const record = openRecord(bridge.path);
    if (existing && !record.transcript) throw new SessionResumeRefused(existing, "the recorded Chat changed during launch; try again");
    await runSession(config, overrides, events, bridge, record);
  } catch (error) {
    await bridge.close();
    throw error;
  }
}

async function runSession(config: HostConfig, overrides: HostOverrides, events: EventLog,
  bridge: BridgeServer, record: SessionRecord): Promise<void> {
  const launchProvider = config.provider;
  restrictEnvironment(process.env, config.aws || config.summary?.provider === "amazon-bedrock" ? "amazon-bedrock" : launchProvider);
  const rawFetch = globalThis.fetch.bind(globalThis);
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
  mkdirSync(config.sessionDir, { recursive: true, mode: 0o700 });
  // Keep SDK startup offline; the selected provider's guard is installed after the transcript is opened.
  globalThis.fetch = guardedFetch(rawFetch, undefined, []).fetch as typeof globalThis.fetch;
  const smithy = config.aws || config.summary?.provider === "amazon-bedrock" ? await import("./smithy.ts") : undefined;
  smithy?.adoptSmithyHandlers();
  const sdk = await import("@earendil-works/pi-coding-agent");
  let transcript = record.transcript;
  if (!transcript && config.identity.role === "worker" && !config.freshSession) {
    const sibling = siblingTranscript(config.local.dir);
    if (sibling) transcript = inheritTranscript(sibling, config.sessionDir);
  }
  if (!transcript && overrides.codeBootstrap?.seed !== undefined) {
    transcript = writeCodeSeed(config.sessionDir, config.cwd, sdk.CURRENT_SESSION_VERSION, overrides.codeBootstrap.seed);
  }
  const sessionManager = openSession(sdk, config.cwd, config.sessionDir, transcript);
  const launch = config.model === undefined || launchProvider === undefined ? undefined : {
    provider: launchProvider, model: config.model.startsWith(`${launchProvider}/`)
      ? config.model.slice(launchProvider.length + 1) : config.model,
  };
  const branch = record.transcript ? sessionManager.getBranch() : undefined;
  const resumed = branch ? restoreModelChoice(sessionManager, interactiveModels(config.identity.role),
    restorableFallbacks(launch, config.fallback, config.fallbackPolicy, config.noFallback, branch),
    record.legacyAnthropicApi) : undefined;
  const choice = resumed ?? launch;
  if (record.transcript && (config.terminalChat || overrides.build) && !resumed &&
      (config.terminalChat || !launch || branch?.some(entry => entry.type === "model_change"))) {
    throw new SessionResumeRefused(record.transcript, "no supported saved model selection; start a new conversation instead");
  }
  if (!choice) throw new Error("missing launch model");
  const { provider, model } = choice;
  if (provider === "llama.cpp" && overrides.build) throw new Error("Local inference (llama.cpp) is unavailable in a build session");
  const effort = resumed ? branch?.findLast(entry => entry.type === "thinking_level_change")?.thinkingLevel : config.effort;
  const workerLaunch = config.identity.role === "worker" && launch ? { ...launch, effort: config.effort } : undefined;
  const modules = await loadModules(assets, config.identity.role, provider);
  // A worker's child profiles join the session's routes like fallbacks; its own model stays bound.
  const childProfiles = config.identity.role === "worker" && config.capabilities?.enabled.includes("fleet")
    ? readBuildProfiles(config.root) : undefined;
  const childProviders = childProfileProviders(config.identity.role, workerLaunch?.provider ?? provider, childProfiles);
  const profileChoices = (childProfiles ?? []).flatMap(profile => {
    const slash = profile.model.indexOf("/");
    const id = profile.model.slice(0, slash);
    return isProviderId(id) && childProviders.includes(id) ? [{ provider: id, model: profile.model.slice(slash + 1) }] : [];
  });
  const activeFallbackPolicy = config.noFallback ? undefined : config.fallbackPolicy;
  const providerFallbackSeeds: ProviderId[] = [
    ...(!config.fallback?.length ? [provider] : []),
    ...(activeFallbackPolicy?.children ? [workerLaunch?.provider ?? provider, ...childProviders] : []),
  ];
  const extraFallbackProviders = fallbackProvidersFor(providerFallbackSeeds, activeFallbackPolicy);
  const requested = [
    { provider, model },
    ...(workerLaunch ? [workerLaunch] : []),
    ...(!config.noFallback ? config.fallback ?? [] : []),
    ...profileChoices,
    ...extraFallbackProviders.map(id => ({ provider: id, model })),
  ];
  const bedrock = requested.filter(choice => choice.provider === "amazon-bedrock");
  const local = config.capabilities?.llama_cpp;
  let aws: AwsRoute | undefined;
  const rules: DestinationRule[] = [];
  if (local && requested.some(choice => choice.provider === "llama.cpp")) rules.push(...localInferenceRules(local));
  if (bedrock.length) {
    if (!config.aws && provider === "amazon-bedrock") {
      throw new CredentialsUnavailable(`no Amazon Bedrock binding was passed for provider ${provider}; ` +
        `run \`corral agent login --provider ${provider}\``);
    }
    if (config.aws) {
      try {
        const { bedrockRoute, createAwsCredentialResolver } = await import("../auth/aws.ts");
        const resolver = createAwsCredentialResolver(config.aws, homedir());
        const profile = await resolver.profile();
        const modelIds = new Set(bedrock.map(choice => choice.model));
        for (const id of modelIds) rules.push(...hostRules("amazon-bedrock", bedrockRoute(config.aws, id, profile)));
        aws = { binding: config.aws, resolver, modelIds };
      } catch (error) {
        if (provider === "amazon-bedrock") throw error;
      }
    }
  }
  const routes = await sessionRoutes(modules.auth, provider, config.identity.role, requested.map(choice => choice.provider)
    .filter(id => (id !== "amazon-bedrock" || aws !== undefined) && (id !== "llama.cpp" || local !== undefined)));
  // Key-authenticated API requests never get the generic transport: the API stream owns their guard.
  const providers = routes.providers.filter(id => !isApiProvider(id));
  for (const id of providers) if (PROVIDERS[id].auth === "oauth") rules.push(...hostRules(id));
  const fallbackSend = PROVIDERS[provider].auth === "local" || PROVIDERS[provider].auth === "aws-profile"
    ? modules.auth.transport?.(authContext(provider)) ?? rawFetch : rawFetch;
  const send = providerTransport(modules.auth, providers.filter(id => PROVIDERS[id].auth === "oauth"), fallbackSend);
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
    smithy!.installSocketTripwire((destination) => network.refuse?.(destination) ?? new NetworkRefused(destination));
  }
  globalThis.fetch = summary?.awsRoute ? guardedFetch(rawFetch, undefined, providers,
    [...rules, ...summary.rules]).fetch as typeof globalThis.fetch
    : network.fetch as typeof globalThis.fetch;
  if (provider === "amazon-bedrock") await aws!.resolver.get();
  if (summary?.awsRoute) await summary.awsRoute.resolver.get();
  process.chdir(config.cwd);
  let remote: RemoteOwner | undefined;
  let host: HostSession | undefined;
  let terminal: InstanceType<typeof sdk.InteractiveMode> | undefined;
  const client = new CorralClient({ corral: config.corral, root: config.root, cwd: config.cwd, identity: config.identity,
    sessionDir: config.local.dir });
  const pane = config.pane || process.env.HERDR_PANE_ID;
  let handoffPending = false;
  let terminalDrain = async () => {};
  const codeHandoff: CodeHandoff | undefined = overrides.prepareHandoff && overrides.commitHandoff
    ? async request => {
      if (handoffPending || !terminal || !host) throw new Error("Code handoff is not available right now.");
      handoffPending = true;
      const active = host.session.model;
      let next: CodeLaunch;
      try {
        next = await overrides.prepareHandoff!(request, active ? `${active.provider}/${active.id}` : `${provider}/${model}`,
          host.session.thinkingLevel);
      } catch (error) {
        handoffPending = false;
        throw error;
      }
      try {
        await terminalDrain();
        terminal.stop();
        await bridge.close();
        await host.session.abort();
        try { await host.session.extensionRunner.emit({ type: "session_shutdown", reason: "quit" }); }
        finally { await close(); }
        await host.runtime.services.modelRuntime.settleCredentialWrites();
        await new Promise<void>(resolve => process.stdout.write("", () => resolve()));
        await overrides.commitHandoff!(next);
        process.exit(0);
      } catch (error) {
        process.stderr.write(`corral-pi Code handoff failed: ${String(error)}\n`);
        process.exit(2);
      }
    } : undefined;
  host = await HostSession.create({
    sdk, identity: config.identity, cwd: config.cwd, agentDir, model, provider, ...(aws ? { aws } : {}),
    ...(workerLaunch ? { workerLaunch } : {}),
    routes, fallback: config.fallback,
    ...(config.noFallback ? { noFallback: true } : {}),
    ...(config.fallbackPolicy ? { fallbackPolicy: config.fallbackPolicy } : {}),
    apiFetch: rawFetch, sessionDir: config.local.dir,
    paneInput: async (text, images) => await remote?.paneInput(text, images) ?? false,
    transcriptRewound: () => { if (terminal) repaintTranscript(terminal); },
    automatedInput: input => remote?.submit(input),
    startInput: (input, answers) => remote?.start(input, answers),
    ...(effort ? { effort } : {}),
    quickSummary: config.quickSummary,
    ...(summary ? { summary } : {}),
    ...(config.promptFile ? { promptFile: config.promptFile } : {}),
    ...(config.ui ? { ui: config.ui } : {}),
    todo: config.todo === true,
    ...(config.planMode ? { planMode: true } : {}),
    paused: () => record.paused,
    ...(config.capabilities ? { capabilities: config.capabilities } : {}),
    ...(config.guidance ? { guidance: config.guidance } : {}),
    root: config.root, exportDir: join(config.sessionDir, "exports"), runtimeDir: config.runtimeDir,
    ...(childProfiles ? { childProfiles } : {}),
    modules, client, events, network,
    ...(overrides.build ? { build: overrides.build } : {}),
    ...(pane ? herdrLifecycle(process.env, pane, config.identity, events, config.local?.headless || headlessEnabled(config.capabilities)) : {}),
    sessionManager,
    ...(codeHandoff ? { codeHandoff } : {}),
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
  // Pi normally does not persist a transcript until the first prompt. Code can resume even before that.
  if (overrides.build?.session.trustedCode && !record.transcript) {
    // A copied transcript may carry an earlier or foreign selection. Persist the
    // destination selection so the next explicit resume uses the destination route.
    if (overrides.codeBootstrap?.seed !== undefined) {
      const branch = sessionManager.getBranch();
      const saved = branch.findLast(entry => entry.type === "model_change");
      const active = session.session.model;
      if (active && (saved?.provider !== active.provider || saved.modelId !== active.id)) {
        sessionManager.appendModelChange(active.provider, active.id);
      }
      if (branch.findLast(entry => entry.type === "thinking_level_change")?.thinkingLevel !== session.session.thinkingLevel) {
        sessionManager.appendThinkingLevelChange(session.session.thinkingLevel);
      }
    }
    sessionManager.appendCustomEntry("corral-code-start", undefined, { immediate: true });
  }
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
      state: () => session.controlState(owner.paused),
      submit: (input, requestId) => owner.submit(input, undefined, requestId),
      answer: input => owner.answer(input),
      pause: () => owner.pause(),
      resume: () => owner.resume(),
      setModel: args => owner.setModel(args),
      cancel: turn => session.cancel(turn),
      rewind: input => owner.rewind(input),
      queueState: limit => owner.queueState(limit),
      queueWithdraw: key => owner.withdraw(key),
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
    await attachHeadless(session, events, line => process.stdout.write(line), () => {
      if (handlers) {
        installRemote(session, owner, config.local.dir);
        bridge.attach(handlers, session.liveEvents, session.remoteRegistry);
        events.emit("ready");
      }
    });
    return;
  }
  const panels = session.panels;
  const { ProcessTerminal } = await import("@earendil-works/pi-tui");
  const physicalTerminal = new ProcessTerminal();
  terminalDrain = () => physicalTerminal.drainInput(1000);
  terminal = new sdk.InteractiveMode(session.runtime, {
    // corral resumes sessions with its own commands; Pi's `pi --session …` line would name a command that does not work here.
    ...(codeHandoff ? { onTerminalCommand: (text: string) => session.handleCodeCommand(text) } : {}),
    verbose: false, resumeHint: false, terminal: panels ? panels.wrap(physicalTerminal) : physicalTerminal,
    ...(panels ? { tuiMode: "fullscreen" as const, readOnlyQueue: true, mainView: (transcript, presentation) => panels.mainView(transcript, presentation),
      errorPresentation: errorPresenter(() => session.latestStopCause()), userPresentation: presentHostNotice, ...panels.presentation() } : {}),
  });
  guardTerminalStop(terminal, process.stdin);
  await terminal.init();
  if (overrides.codeBootstrap?.draft) {
    session.session.extensionRunner.getUIContext().setEditorText(overrides.codeBootstrap.draft);
  }
  session.markAttached("interactive");
  if (handlers) {
    installRemote(session, owner, config.local.dir);
    bridge.attach(handlers, session.liveEvents, session.remoteRegistry);
  }
  try { await terminal.run(); }
  finally { await close(); }
}

