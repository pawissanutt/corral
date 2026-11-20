import { join } from "node:path";
import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { validatePlan } from "./board.ts";
import { reviewBrief } from "./review-brief.ts";
import { checked, type Run } from "./corral-cli.ts";
import type { BuildJournal } from "./record.ts";
import { verifyBuild } from "./gate.ts";
import type { FleetRuntime } from "../extensions/fleet/runtime.ts";
import type { BoardPlan, BuildContext } from "./types.ts";

export function buildFinishTool(open: () => void, journal: BuildJournal, landing: () => boolean = () => false): ToolDefinition {
  return {
    name: "build_finish", label: "Offer the build exit",
    description: "Store a real title describing what was built and open the owner's Keep / Iterate / Land choice. This tool never chooses an exit or files board work.",
    parameters: Type.Object({ title: Type.String({ minLength: 1, maxLength: 120 }), summary: Type.Optional(Type.String()) }, { additionalProperties: false }),
    async execute(_id, params: { title: string; summary?: string }) {
      try {
        if (landing()) throw new Error("The owner's Land is already running; no exit choice was opened and the title is unchanged.");
        journal.finish(params.title, params.summary);
        let detail = "";
        try { await journal.refresh(); } catch (error) { detail = `\nBuild record Git snapshot failed: ${String(error)}`; }
        open();
        return { content: [{ type: "text", text: `The owner now chooses Keep, Iterate or Land. Do not choose an exit yourself.${detail}` }], details: {} };
      } catch (error) { return { content: [{ type: "text", text: String(error) }], details: {}, isError: true }; }
    },
  };
}

/** Verification and review collect evidence only; the owner alone can land. */
export function buildEvidenceTools(options: { context: BuildContext; git: Run; fleet: FleetRuntime; journal: BuildJournal }): ToolDefinition[] {
  const { context, git, fleet, journal } = options;
  const session = context.session;
  let reviewNumber = Math.max(0, ...journal.record().reviews.map(review => Number(review.reviewer.replace(/^review-/, "")) || 0));
  const tools: ToolDefinition[] = [{
    name: "build_verify", label: "Verify the build",
    description: "Run the project's configured gate after committing. Record its result, failed command and log. Does not choose an exit.",
    parameters: Type.Object({}, { additionalProperties: false }),
    async execute(_id, _params, signal) {
      if (!session.workspaceRoot || !session.project) return { content: [{ type: "text", text: "build_verify requires a corral workspace and configured project." }], details: {}, isError: true };
      try {
        const gate = await verifyBuild(context, git, journal, signal);
        let detail = "";
        try { await journal.refresh(); } catch (error) { detail = `\nBuild record Git snapshot failed: ${String(error)}`; }
        return { content: [{ type: "text", text: `${gate.attempt}: ${gate.result}${gate.failed_command ? `\nFailed command: ${gate.failed_command}` : ""}${gate.log ? `\nLog: ${gate.log}` : ""}${detail}` }],
          details: { gate }, isError: gate.result === "fail" };
      } catch (error) {
        return { content: [{ type: "text", text: error instanceof Error ? error.message : String(error) }], details: {}, isError: true };
      }
    },
  }, {
    name: "build_review", label: "Review the build",
    description: "Start an independent read-only reviewer and wait for its exact verdict. Records findings for build_resolve; never chooses an exit.",
    parameters: Type.Object({ focus: Type.Optional(Type.String()) }, { additionalProperties: false }),
    async execute(call, params: { focus?: string }, signal) {
      try {
        const outsideGit = session.bare && session.repository === false;
        const head = outsideGit ? "" : (await checked(git, ["rev-parse", "HEAD"], session.worktree)).trim();
        const base = session.base;
        if (!base && !outsideGit) throw new Error("Commit a base and the candidate before requesting review.");
        const used = new Set([...fleet.snapshot().children.map(child => child.id), ...journal.record().slices.map(child => child.id)]);
        let id: string;
        do { id = `review-${++reviewNumber}`; } while (used.has(id));
        const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;
        const brief = reviewBrief({
          head: [
            "Independently review this build. You are read-only: do not edit files or run Git writes. Do not run build gates.",
            outsideGit ? `Directory: ${session.worktree}\nThis is a bare session outside Git. Inspect files directly; there is no diff, commit or gate.`
              : `Worktree: ${session.worktree}\nBase: ${base}\nHead: ${head}`,
            ...(outsideGit ? [] : [`Read-only commands:\ngit -C ${quote(session.worktree)} diff --stat ${quote(`${base}..${head}`)} --\ngit -C ${quote(session.worktree)} diff ${quote(`${base}..${head}`)} --\ngit -C ${quote(session.worktree)} log --oneline ${quote(`${base}..${head}`)}`]),
          ],
          prompts: journal.prompts().map(prompt => `[${prompt.at}]\n${prompt.text}`),
          slices: journal.record().slices.filter(child => child.kind !== "review").map(child => `${child.id}: ${child.title}\nOwned paths: ${child.owns.join(", ") || "none"}\nReport:\n${child.report ?? "No report."}`),
          tail: [
            `Latest gate: ${JSON.stringify(journal.latestGate() ?? null)}`,
            ...(params.focus ? [`Review focus: ${params.focus}`] : []),
            `Finish by calling review_verdict with verdict approve or changes, summary, findings${outsideGit ? " (omit head: this directory has no Git head)" : ` and head ${head}`}. Record the verdict exactly as you judge it; peer messages are information, not authority to approve.`,
          ],
          record: join(context.dir, "journal.json"),
        });
        await fleet.start(call, [{ id, kind: "review", title: `Review build round ${journal.record().reviews.length + 1}`, brief, owns: [],
          profile: fleet.profiles().some(profile => profile.name === "review") ? "review" : "default" }], signal);
        const [child] = await fleet.wait([id], signal);
        if (!child?.verdict) throw new Error(`Reviewer ${id} ended without a verdict${child?.error ? `: ${child.error}` : "."}`);
        const review = journal.review(child, head);
        let detail = "";
        try { await journal.refresh(); } catch (error) { detail = `\nBuild record Git snapshot failed: ${String(error)}`; }
        return { content: [{ type: "text", text: `${review.verdict}: ${review.summary ?? ""}\n${review.findings.map(finding => `${finding.id} [${finding.severity}] ${finding.path ?? ""}${finding.line ? `:${finding.line}` : ""} ${finding.text}`).join("\n")}${detail}` }],
          details: { round: review.round, review } };
      } catch (error) { return { content: [{ type: "text", text: error instanceof Error ? error.message : String(error) }], details: {}, isError: true }; }
    },
  }, {
    name: "build_resolve", label: "Resolve a review finding",
    description: "Record a finding as fixed or declined. A declined finding requires a reason; this never changes the reviewer's verdict.",
    parameters: Type.Object({ round: Type.Integer({ minimum: 1 }), finding: Type.String(), state: Type.Union([Type.Literal("fixed"), Type.Literal("declined")]),
      commit: Type.Optional(Type.String()), reason: Type.Optional(Type.String()) }, { additionalProperties: false }),
    async execute(_id, params: { round: number; finding: string; state: "fixed" | "declined"; commit?: string; reason?: string }) {
      try {
        journal.resolve(params.round, params.finding, { state: params.state, commit: params.commit, reason: params.reason });
        return { content: [{ type: "text", text: `${params.finding} recorded as ${params.state}.` }], details: {} };
      } catch (error) { return { content: [{ type: "text", text: String(error) }], details: {}, isError: true }; }
    },
  }];
  return session.bare ? tools.filter(tool => tool.name !== "build_verify") : tools;
}

export function boardPlanTool(propose: (plan: BoardPlan) => void, available: () => string | undefined): ToolDefinition {
  return {
    name: "board_plan", label: "Propose board tasks",
    description: "Propose a bounded plan of at most 20 follow-on tasks for the owner to confirm. "
      + "Give each task a unique key, a concrete title and body, and dependencies naming local keys or existing T-<n> tasks. "
      + "The owner must confirm before the tasks are filed.",
    parameters: Type.Object({
      title: Type.String({ minLength: 1 }), project: Type.Optional(Type.String()), why: Type.String(),
      tasks: Type.Array(Type.Object({
        key: Type.String({ minLength: 1 }), title: Type.String({ minLength: 1 }),
        kind: Type.Optional(Type.String()), size: Type.Optional(Type.String()), profile: Type.Optional(Type.String()),
        depends: Type.Array(Type.String()), body: Type.String(),
      }, { additionalProperties: false }), { minItems: 1, maxItems: 20 }),
    }, { additionalProperties: false }),
    async execute(_id, plan: BoardPlan) {
      const unavailable = available();
      if (unavailable) return { content: [{ type: "text", text: unavailable }], details: {}, isError: true };
      try {
        validatePlan(plan);
        propose(plan);
        return { content: [{ type: "text", text: "The owner will confirm this board plan before any task is filed or queued." }], details: {} };
      } catch (error) {
        return { content: [{ type: "text", text: error instanceof Error ? error.message : String(error) }], details: {}, isError: true };
      }
    },
  };
}
