import type { AgentMessage } from "@earendil-works/pi-agent-core";

export type ResearchProfile = "code" | "docs";
export type ResearchEffort = "quick" | "standard" | "deep";
export interface ResearchOptions {
  readonly profile?: ResearchProfile;
  readonly effort?: ResearchEffort;
  readonly scope?: string;
  readonly starting_points?: string;
  readonly constraints?: string;
}
/** How much the helper actually looked; lets the parent spot a thin answer. */
export interface ResearchDepth {
  readonly turns: number;
  readonly toolCalls: number;
  readonly resultBytes: number;
}
export interface ResearchReport {
  readonly status: "completed" | "partial" | "failed";
  readonly stopReason: "completed" | "turn_limit" | "time_reserve" | "timeout" | "cancelled" | "provider_error" | "empty_response" | "output_limit";
  readonly depth?: ResearchDepth;
}

const THINKING_ORDER = ["off", "minimal", "low", "medium", "high", "xhigh", "max"] as const;
type Thinking = typeof THINKING_ORDER[number];
/** A research preset is a floor: a stronger configured profile effort is never lowered. */
export function strongerThinking<T extends string>(floor: Thinking | undefined, chosen: T): T | Thinking {
  if (floor === undefined) return chosen;
  const rank = (level: string) => THINKING_ORDER.indexOf(level as Thinking);
  return rank(chosen) >= rank(floor) ? chosen : floor;
}

/** Presets never expand the configured turn ceiling or choose another credential/model route. */
export function researchBudget(options: ResearchOptions, maxTurns: number) {
  switch (options.effort ?? "standard") {
    case "quick": return { maxTurns: Math.min(maxTurns, 4), thinkingLevel: "off" as const };
    case "standard": return { maxTurns, thinkingLevel: "low" as const };
    case "deep": return { maxTurns, thinkingLevel: "high" as const };
  }
}

export const DEPTH_GUIDANCE: Record<ResearchEffort, string> = {
  quick: "Depth: quick. Focus on locating and citing the main answer.",
  standard: "Depth: standard. Trace the whole path the question names and verify it in the code; do not stop at the first plausible hit.",
  deep: "Depth: deep. Check relevant alternative implementations, error paths and edge cases that could change the answer.",
};

/** Shared method for every read-only code research child, standalone or Build. */
export const CODE_METHOD = [
  "Method:",
  "1. Start with the most relevant sources. If you have independent leads, batch their searches in one response instead of spending a turn on each. With codemode, one cell can run several searches and filter the results.",
  "2. Read the functions or sections around relevant hits, not isolated lines.",
  "3. Follow the path the question names. Check definitions, callers and callees (lsp_definition/lsp_references when available), configuration and tests where they could change the answer.",
  "4. Consider what might overturn the evidence: other implementations, duplicates, flags, error paths and edge cases. If you conclude something is absent, say where you looked.",
].join("\n");
const DOCS_METHOD = "Prefer official, version-relevant documentation. Run independent searches in parallel, open the most authoritative pages, and cross-check claims when another source could change the answer. Cite exact URLs and separate documented behavior from inference. If web tools/search are unavailable, report that limitation; do not invent sources.";
const STOPPING_RULE = "If a missing check could materially change the requested answer and is feasible with your current tools, time and budget, do it now. "
  + "If it is not feasible, name the blocker and what it leaves unverified; do not imply the check ran. "
  + "Stop searching when the requested answer is supported or no further material check is feasible.";
/** The stopping rule and answer shape shared by research children. */
export const ANSWER_FORMAT = `${STOPPING_RULE}\n\nAnswer with: the direct answer first; findings with path:line (or URL) evidence; the relevant files, each with a one-line role; coverage (what you examined) and remaining gaps or uncertainties. Distinguish observed behavior from inference. Do not implement changes.`;

export function researchPrompt(prompt: string, options: ResearchOptions, cwd: string, tools: readonly string[], maxTurns: number,
  orientation?: string): string {
  const profile = options.profile ?? "code";
  const effort = options.effort ?? "standard";
  return [
    `Research profile: ${profile}; effort: ${effort}; root: ${cwd}.`,
    `Mission (at most ${maxTurns} model turns, including final synthesis; each turn may batch several tool calls):\n${prompt}`,
    `Scope (advisory, not an additional sandbox): ${options.scope ?? "the supplied question within the current worktree"}`,
    `Starting points: ${options.starting_points ?? "none supplied"}`,
    `Relevant constraints supplied by parent: ${options.constraints ?? "none supplied; read relevant local guidance when needed"}`,
    `Available tools: ${tools.join(", ") || "none"}. No shell, writes, MCP, board authority or recursive delegation.`,
    ...(orientation && profile === "code" ? [orientation] : []),
    DEPTH_GUIDANCE[effort],
    profile === "docs" ? DOCS_METHOD : CODE_METHOD,
    `${ANSWER_FORMAT} Reserve your final turn for synthesis, not further searching.`,
  ].join("\n\n");
}

/** One follow-up for a run that answered after very little searching. */
export const GAP_CHECK_PROMPT = "Before you finish: consider whether an unsupported claim or unexamined part of the question could change your answer. "
  + `${STOPPING_RULE} Give your complete answer with evidence and any remaining uncertainty (it replaces the previous answer). `
  + "If there is no material gap, return the supported answer without further searching.";
/** Tool calls below which a non-quick answer earns one gap check. */
export const GAP_CHECK_MIN_CALLS = 8;
/** The follow-up stops this long before the child deadline and falls back to the first answer. */
export const GAP_CHECK_RESERVE_MS = 15_000;
/** Longest the optional follow-up may run, including runs without a deadline. */
export const GAP_CHECK_MAX_MS = 180_000;
/** Whether a finished research run should get one gap-check follow-up. */
export function wantsGapCheck(input: { effort?: ResearchEffort; toolCalls: number; turns: number; maxTurns?: number;
  deadlineAt?: number; now: number; stopped: boolean }): boolean {
  if (input.stopped || input.effort === "quick" || input.toolCalls >= GAP_CHECK_MIN_CALLS) return false;
  if (input.maxTurns !== undefined && input.maxTurns - input.turns < 3) return false;
  return input.deadlineAt === undefined || input.deadlineAt - input.now >= 45_000;
}

/**
 * Count what the helper looked at. `observed` is the runner's count of admitted calls (including
 * calls inside Code Mode cells, excluding the cells themselves); without it, direct tool-call
 * blocks are counted, which also includes refused calls.
 */
export function researchDepth(messages: readonly AgentMessage[], observed?: number): ResearchDepth {
  let turns = 0;
  let toolCalls = 0;
  let resultBytes = 0;
  for (const message of messages) {
    if (message.role === "assistant") {
      turns++;
      toolCalls += message.content.filter(block => block.type === "toolCall" && block.name !== "codemode").length;
    } else if (message.role === "toolResult") {
      for (const block of message.content) if (block.type === "text") resultBytes += Buffer.byteLength(block.text, "utf8");
    }
  }
  return { turns, toolCalls: observed ?? toolCalls, resultBytes };
}

/** Report observed work, not a quality verdict inferred from its volume. */
export function depthLine(depth: ResearchDepth | undefined): string | undefined {
  if (!depth) return undefined;
  return `Depth: ${depth.turns} turns, ${depth.toolCalls} tool calls, ${Math.round(depth.resultBytes / 1024)} KB of tool output.`;
}

/** Never substitute a tool log for verified findings without labelling it. No thinking is exposed. */
export function researchResult(messages: readonly AgentMessage[], stopReason: ResearchReport["stopReason"], observedCalls?: number) {
  const assistant = messages.filter(message => message.role === "assistant");
  const last = assistant.at(-1);
  const text = last?.content.filter(block => block.type === "text").map(block => block.text).join("\n").trim() ?? "";
  const final = last?.stopReason === "stop"
    && !last.content.some(block => block.type === "toolCall") && text.length > 0;
  const previous = assistant.flatMap(message => message.content.filter(block => block.type === "text").map(block => block.text)).join("\n").trim();
  const calls = new Map(assistant.flatMap(message => message.content.flatMap(block => block.type === "toolCall"
    ? [[block.id, `${block.name} ${JSON.stringify(block.arguments).slice(0, 400)}`] as const] : [])));
  const excerpts = messages.flatMap(message => {
    if (message.role !== "toolResult" || message.isError) return [];
    const body = message.content.filter(block => block.type === "text").map(block => block.text).join("\n").trim();
    return body ? [`${calls.get(message.toolCallId) ?? message.toolName}:\n${body.slice(0, 1000)}`] : [];
  }).slice(-4);
  const partial = [previous ? `Partial assistant notes (not a final answer):\n${previous.slice(-8000)}` : "",
    excerpts.length ? `Evidence excerpts only (not synthesized findings):\n${excerpts.join("\n\n")}` : "",
  ].filter(Boolean).join("\n\n");
  const reason = stopReason === "completed" && !final
    ? last?.stopReason === "length" ? "output_limit" : "empty_response" : stopReason;
  const report: ResearchReport = { status: final && reason === "completed" ? "completed" : partial ? "partial" : "failed", stopReason: reason,
    depth: researchDepth(messages, observedCalls) };
  return { text: final ? text : partial || "No findings were produced. Narrow the question or inspect the child transcript before retrying.", report };
}
