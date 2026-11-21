import { readFileSync } from "node:fs";
import { BridgeRefusal } from "../../bridge/protocol.ts";
import { LIVE_CHILD_STATES, type ChildSnapshot, type FleetControl, type FleetSnapshot } from "../../build/types.ts";
import type { HelperInspection } from "../../extensions/subagent/inspection.ts";
import { helperState } from "./fleet.ts";
import { parseChildId } from "./input.ts";
import type { RemoteRegistry } from "./registry.ts";
import { clipText, fitRows, isoTime, optionalText, optionalTime } from "./state.ts";

type Choices = Pick<FleetControl, "models" | "profiles">;
const KiB = 1024;

/** A previous host cannot own a live child. Keep only its settled snapshots. */
export function archivedChildren(file: string): readonly ChildSnapshot[] {
  try {
    const journal = JSON.parse(readFileSync(file, "utf8")) as { version?: number; children?: ChildSnapshot[] };
    return journal.version === 1 && Array.isArray(journal.children)
      ? journal.children.filter(child => !LIVE_CHILD_STATES.includes(child.state)) : [];
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return [];
    throw error;
  }
}

export function childDetail(child: ChildSnapshot, transcript: boolean, choices?: Choices) {
  const brief = clipText(child.brief, 8 * KiB), report = optionalText(child.report, 16 * KiB);
  const firstEdit = Math.max(0, child.edits.length - 200), firstSteer = Math.max(0, child.steers.length - 32);
  const verdict = child.verdict, question = child.question;
  return { id: child.id, kind: child.kind, title: clipText(child.title, 512), brief, brief_more: brief !== child.brief,
    owns: fitRows(child.owns.map(path => clipText(path, 512)), 0, 64, 2 * KiB),
    after: fitRows(child.after, 0, 64, KiB), profile: clipText(child.profile, 512), model: clipText(child.model, 512),
    pending_model: optionalText(child.pendingModel, 512), effort: optionalText(child.effort, 64), state: child.state,
    created_at: child.createdAt, started_at: child.startedAt, ended_at: child.endedAt,
    activity: optionalText(child.activity, 512),
    edits: fitRows(child.edits.slice(firstEdit).reverse().map(({ path, outside, at }) => ({ path: clipText(path, 4096), outside, at: isoTime(at) })), 0, 200, 4 * KiB).reverse(),
    usage: { input: child.usage.input, output: child.usage.output, cost_usd: child.usage.cost,
      context_percent: child.usage.contextPercent, turns: child.usage.turns },
    steers: fitRows(child.steers.slice(firstSteer).map((text, i) => ({ text: clipText(text, 2 * KiB),
      at: optionalTime(child.steeredAt?.[firstSteer + i]) })).reverse(), 0, 32, 3 * KiB).reverse(),
    question: question ? { question: clipText(question.question, KiB), asked_at: isoTime(question.askedAt),
      options: fitRows(question.options.map(({ label, detail }) => ({ label: clipText(label, 300), detail: optionalText(detail, 256) })), 0, 64, KiB) } : undefined,
    report, report_more: report !== child.report, error: optionalText(child.error, 512),
    verdict: verdict ? { verdict: verdict.verdict, summary: clipText(verdict.summary, KiB), head: optionalText(verdict.head, 128), at: isoTime(verdict.at),
      findings: fitRows(verdict.findings.slice(0, 50).map(({ severity, path, line, text }) => ({ severity,
        path: optionalText(path, 512), line, text: clipText(text, 768) })), 0, 50, 3 * KiB) } : undefined,
    pulse: child.pulse.slice(-120), history: clipText(child.history, 240), dropped: child.dropped, transcript,
    models: fitRows(choices?.models() ?? [], 0, Infinity, 3 * KiB),
    profiles: fitRows(choices?.profiles().map(({ name, model, effort, description }) => ({ name, model, effort,
      description: optionalText(description, 256) })) ?? [], 0, Infinity, 2 * KiB),
  };
}

export function helperDetail(helper: HelperInspection, transcript: boolean, result = helper.result) {
  const brief = clipText(helper.prompt, 8 * KiB), report = optionalText(result, 16 * KiB);
  return { id: helper.request, kind: `helper:${helper.kind}`, title: helper.kind, brief,
    brief_more: brief !== helper.prompt || helper.truncated && helper.prompt.length >= 4096,
    profile: "", model: clipText(helper.model, 512), state: helperState(helper), created_at: helper.createdAt,
    started_at: helper.startedAt, ended_at: helper.endedAt, activity: optionalText(helper.latestActivity, 512),
    usage: { input: 0, output: 0, turns: 0 }, report,
    report_more: report !== result || result === helper.result && helper.truncated && (result?.length ?? 0) >= 4096,
    error: optionalText(helper.error ?? helper.inspectionError, 512), history: "", dropped: helper.dropped, transcript };
}

export function childDetailsChanged(children: readonly ChildSnapshot[], helpers: readonly HelperInspection[]) {
  // Fleet rows do not carry questions, text, edit identities or timeline changes that leave their counts unchanged.
  return [children.map(({ id, activity, edits, usage, steers, steeredAt, question, report, verdict, pulse, history, dropped, sessionFile }) =>
    [id, activity, edits, usage, steers, steeredAt, question, report, verdict, pulse, history, dropped, !!sessionFile]),
  helpers.map(({ request, result, latestActivity, dropped, inspectionError, truncated }) => [request, result, latestActivity, dropped, inspectionError, truncated])];
}

export function registerChildren(registry: RemoteRegistry, source: {
  snapshot(): FleetSnapshot | undefined; helpers(): readonly HelperInspection[];
  fleet(): FleetControl | undefined; ready(): boolean;
}): void {
  registry.register({ name: "child", parse: ({ child }) => parseChildId(child), run: child => {
    const row = source.snapshot()?.children.find(row => row.id === child);
    const transcript = () => {
      try { return !!registry.get("child_file")?.({ child }); } catch { return false; }
    };
    if (row) return childDetail(row, transcript(), LIVE_CHILD_STATES.includes(row.state) ? source.fleet() : undefined);
    const helper = source.helpers().find(row => row.request === child);
    if (helper) return helperDetail(helper, transcript());
    throw new BridgeRefusal("child_missing", "no such child in this session");
  } });
  registry.register({ name: "child_act", parse: ({ child, action, model }) => {
    const id = parseChildId(child);
    if (action === "cancel") return { id, action } as const;
    if (action !== "set_model" || typeof model !== "string" || !model.trim() || Buffer.byteLength(model) > 512) {
      throw new BridgeRefusal("bad_input", "invalid child action");
    }
    return { id, action, model } as const;
  }, run: args => {
    if (!source.ready()) throw new BridgeRefusal("host_not_ready", "session is not accepting child actions");
    const fleet = source.fleet();
    const child = source.snapshot()?.children.find(row => row.id === args.id);
    if (!fleet || !child || !LIVE_CHILD_STATES.includes(child.state)) return { ok: false, reason: "Child is not live" };
    if (args.action === "cancel") return fleet.cancel(args.id) ? { ok: true } : { ok: false, reason: "Child is not live" };
    const model = fleet.profiles().find(profile => profile.name === args.model)?.model ?? args.model;
    if (!fleet.models().includes(model)) return { ok: false, reason: "Model is not approved for this fleet" };
    try { fleet.setModel(args.id, args.model); }
    catch (error) { return { ok: false, reason: clipText(error instanceof Error ? error.message : String(error), 512) }; }
    return { ok: true };
  } });
}
