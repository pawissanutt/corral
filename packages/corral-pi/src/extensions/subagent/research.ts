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
  quick: "Depth: quick. Locate and cite the main answer; one or two rounds of parallel searches are enough.",
  standard: "Depth: standard. Trace the whole path the question names and verify it in the code; do not stop at the first plausible hit.",
  deep: "Depth: deep. Be exhaustive: cover every layer, alternative implementation, error path and edge case that could change the answer.",
};

/** Shared method for every read-only code research child, standalone or Build. */
export const CODE_METHOD = [
  "Method:",
  "1. Orient broadly first. Turns are counted per model response, not per tool call: batch 3-8 independent searches (grep for key identifiers and synonyms, find for likely file names, ls of relevant directories) in one response instead of one search per turn. With codemode, one cell can run many searches and filter the results.",
  "2. Read whole functions or sections around each hit, not isolated lines.",
  "3. Follow the path. For each key symbol check its definition, callers and callees (lsp_definition/lsp_references when available), configuration and tests. Cover every layer the question names.",
  "4. Look for what would change the answer: other implementations, duplicates, flags, error paths and edge cases.",
  "5. Before answering, check coverage: every claim has path:line evidence and every part of the question was examined. Spend remaining turns on gaps. Report something as absent only after listing the searches that failed to find it.",
].join("\n");
const DOCS_METHOD = "Prefer official, version-relevant documentation. Run independent searches in parallel, open the most authoritative pages, and cross-check important claims across sources. Cite exact URLs and separate documented behavior from inference. If web tools/search are unavailable, report that limitation; do not invent sources.";
/** The answer shape shared by research children. */
export const ANSWER_FORMAT = "Answer with: the direct answer first; findings with path:line (or URL) evidence; the relevant files, each with a one-line role; coverage (what you examined) and remaining gaps or uncertainties; a next step. Distinguish observed behavior from inference. Do not implement changes.";

export function researchPrompt(prompt: string, options: ResearchOptions, cwd: string, tools: readonly string[], maxTurns: number,
  orientation?: string): string {
  const profile = options.profile ?? "code";
  const effort = options.effort ?? "standard";
  return [
    `Research profile: ${profile}; effort: ${effort}; root: ${cwd}.`,
    `Mission (at most ${maxTurns} model turns, including final synthesis; each turn may batch several tool calls):\n${prompt}`,
    `Scope (advisory, not an additional sandbox): ${options.scope ?? "the supplied question within the current worktree"}`,
    `Starting points: ${options.starting_points ?? "none supplied; start with broad parallel searches"}`,
    `Relevant constraints supplied by parent: ${options.constraints ?? "none supplied; read relevant local guidance when needed"}`,
    `Available tools: ${tools.join(", ") || "none"}. No shell, writes, MCP, board authority or recursive delegation.`,
    ...(orientation && profile === "code" ? [orientation] : []),
    DEPTH_GUIDANCE[effort],
    profile === "docs" ? DOCS_METHOD : CODE_METHOD,
    `${ANSWER_FORMAT} Reserve your final turn for synthesis, not further searching.`,
  ].join("\n\n");
}

/** One follow-up for a run that answered after very little searching. */
export const GAP_CHECK_PROMPT = "Before you finish: list each claim in your answer that lacks path:line evidence, and each part of the question or lead you have not examined. "
  + "Investigate the most important gaps now, batching independent searches in one response. "
  + "Then give your complete final answer again in full (it replaces the previous one), including what you verified and what remains uncertain.";
/** Tool calls below which a non-quick answer earns one gap check. */
export const GAP_CHECK_MIN_CALLS = 8;
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
      for (const block of message.content) if (block.type === "text") resultBytes += block.text.length;
    }
  }
  return { turns, toolCalls: observed ?? toolCalls, resultBytes };
}

/** One line for the parent; a thin run is labelled so it can be retried deeper. */
export function depthLine(depth: ResearchDepth | undefined, effort?: ResearchEffort): string | undefined {
  if (!depth) return undefined;
  const kb = Math.round(depth.resultBytes / 1024);
  const thin = effort !== "quick" && depth.toolCalls < GAP_CHECK_MIN_CALLS;
  return `Depth: ${depth.turns} turns, ${depth.toolCalls} tool calls, ${kb} KB of tool output.`
    + (thin ? " Thin coverage: verify key claims or retry with effort deep or narrower questions." : "");
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
