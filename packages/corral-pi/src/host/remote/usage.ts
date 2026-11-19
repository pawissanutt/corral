import { SESSION_PART_MAX } from "../../bridge/protocol.ts";
import type { ChildSnapshot } from "../../build/types.ts";
import type { HelperInspection } from "../../extensions/subagent/inspection.ts";
import type { PanelSnapshot } from "../../ui/panels-view.ts";
import { sessionCost } from "../../ui/usage-model.ts";
import { clipText, fitRows, jsonBytes, type UsageBrief } from "./state.ts";

interface UsageRow {
  key: string; name: string; kind: string; tokens: number; input: number; output: number;
  cost_usd?: number; context_percent?: number; turns: number;
}
export interface ParentUsage extends UsageBrief { input: number; output: number; turns: number }

export function usageTotal(parent: UsageBrief, children: readonly ChildSnapshot[], hasUnpricedHelpers = false) {
  let tokens = parent.tokens, cost_usd = parent.cost_usd, partial = parent.cost_partial || cost_usd === undefined || hasUnpricedHelpers;
  for (const { usage } of children) {
    tokens += usage.input + usage.output;
    if (usage.cost === undefined) partial = true;
    else cost_usd = (cost_usd ?? 0) + usage.cost;
  }
  return { tokens, cost_usd: partial && cost_usd === 0 ? undefined : cost_usd, partial };
}

export function usagePart(parent: ParentUsage, children: readonly ChildSnapshot[], model?: string, effort?: string,
  snapshot?: PanelSnapshot, helpers: readonly HelperInspection[] = []) {
  const total = usageTotal(parent, children, helpers.length > 0);
  if (snapshot?.build) {
    // Keep the pane's API-equivalent pricing, while also marking any missing child price.
    const priced = sessionCost({ ...snapshot, cost: parent.cost_usd, build: {
      ...snapshot.build, fleet: { ...snapshot.build.fleet, children },
    } });
    total.partial ||= priced?.partial ?? false;
    total.cost_usd = total.partial && priced?.total === 0 ? undefined : priced?.total;
  }
  const context = parent.context_window ? { tokens: parent.context_percent === undefined ? undefined : parent.context_tokens,
    context_window: parent.context_window, percent: parent.context_percent } : undefined;
  const rows: UsageRow[] = [{ key: "pane", name: "Pane", kind: "session", tokens: parent.tokens, input: parent.input,
    output: parent.output, cost_usd: parent.cost_usd, context_percent: parent.context_percent, turns: parent.turns }];
  for (const child of [...children].reverse()) {
    const { input, output, cost, contextPercent, turns } = child.usage;
    rows.push({ key: `child:${child.id}`, name: clipText(child.title, 256), kind: child.kind, tokens: input + output,
      input, output, cost_usd: cost, context_percent: contextPercent, turns });
  }
  for (const helper of helpers) rows.push({ key: `child:${helper.request}`, name: helper.kind, kind: `helper:${helper.kind}`,
    tokens: 0, input: 0, output: 0, turns: 0 });
  const head = { model, effort, context, total, rows: [] as UsageRow[] };
  return { ...head, rows: fitRows(rows, 0, rows.length, SESSION_PART_MAX - jsonBytes(head)) };
}
