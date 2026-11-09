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
export interface ResearchReport {
  readonly status: "completed" | "partial" | "failed";
  /** Review coverage is independent of why inference stopped. */
  readonly snapshotCoverage?: "complete" | "incomplete";
  readonly stopReason: "completed" | "turn_limit" | "time_reserve" | "timeout" | "cancelled" | "provider_error" | "empty_response" | "output_limit";
}

/** Presets never expand the configured turn ceiling or choose another credential/model route. */
export function researchBudget(options: ResearchOptions, maxTurns: number) {
  switch (options.effort ?? "standard") {
    case "quick": return { maxTurns: Math.min(maxTurns, 4), thinkingLevel: "off" as const };
    case "standard": return { maxTurns, thinkingLevel: "low" as const };
    case "deep": return { maxTurns, thinkingLevel: "high" as const };
  }
}

export function researchPrompt(prompt: string, options: ResearchOptions, cwd: string, tools: readonly string[], maxTurns: number): string {
  const profile = options.profile ?? "code";
  return [
    `Research profile: ${profile}; effort: ${options.effort ?? "standard"}; root: ${cwd}.`,
    `Mission (at most ${maxTurns} model turns, including final synthesis):\n${prompt}`,
    `Scope (advisory, not an additional sandbox): ${options.scope ?? "the supplied question within the current worktree"}`,
    `Starting points: ${options.starting_points ?? "use targeted search; avoid broad file dumps"}`,
    `Relevant constraints supplied by parent: ${options.constraints ?? "none supplied; read relevant local guidance when needed"}`,
    `Available tools: ${tools.join(", ") || "none"}. No shell, writes, MCP, board authority or recursive delegation.`,
    profile === "docs"
      ? "Prefer official, version-relevant documentation. Cite exact URLs and separate documented behavior from inference. If web tools/search are unavailable, report that limitation; do not invent sources."
      : "Trace the smallest relevant execution path. Cite file:line evidence, distinguish observed behavior from inference, and avoid unrelated files.",
    "Stop once the question is answered. Return concise findings, evidence, coverage/uncertainties and a next step. Do not implement changes. Reserve your final turn for synthesis, not further searching.",
  ].join("\n\n");
}

/** Never substitute a tool log for verified findings without labelling it. No thinking is exposed. */
export function researchResult(messages: readonly AgentMessage[], stopReason: ResearchReport["stopReason"]) {
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
  const report: ResearchReport = { status: final && reason === "completed" ? "completed" : partial ? "partial" : "failed", stopReason: reason };
  return { text: final ? text : partial || "No findings were produced. Narrow the question or inspect the child transcript before retrying.", report };
}
