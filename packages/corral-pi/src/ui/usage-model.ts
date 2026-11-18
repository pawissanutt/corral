import type { ContextUsage } from "@earendil-works/pi-coding-agent";
import type { Role } from "../roles/types.ts";
import type { PanelSnapshot, SpendingSnapshot } from "./panels-view.ts";
import { childHue } from "./build/contract.ts";

export interface UsageIdentity { role: Role; project: string; task?: string; run?: string; assistant?: string; build?: string }
export interface SessionRow { key: string; name: string; hue: number | undefined; tokens: number | undefined; cost: number | undefined; contextPercent: number | undefined }
export interface SessionUsage { model: string | null; effort: string; context: ContextUsage | undefined;
  rows: readonly SessionRow[]; total: { cost: number; partial: boolean } | undefined }
export function sessionCost(snapshot: PanelSnapshot | null): { total: number; partial: boolean } | undefined {
  const children = snapshot?.build?.fleet.children ?? [];
  const priced = children.filter(child => child.usage.cost !== undefined);
  if (!snapshot || (snapshot.cost === undefined && !priced.length)) return undefined;
  return { total: (snapshot.cost ?? 0) + priced.reduce((sum, child) => sum + (child.usage.cost ?? 0), 0),
    partial: snapshot.cost === undefined || children.some(child => child.usage.cost === undefined && child.usage.turns > 0) };
}
export interface UsageModel { identity: UsageIdentity; session: SessionUsage | undefined; spending: SpendingSnapshot | undefined }
export function usageModel({ identity, snapshot }: { identity: UsageIdentity; snapshot: PanelSnapshot | null }): UsageModel {
  const children = snapshot?.build?.fleet.children ?? [];
  const total = snapshot?.build ? sessionCost(snapshot) : undefined;
  return { identity, spending: snapshot?.budget,
    session: snapshot ? { model: snapshot.model, effort: snapshot.effort, context: snapshot.context,
      rows: [{ key: "pane", name: "Pane", hue: undefined, tokens: snapshot.tokens, cost: snapshot.cost,
        contextPercent: snapshot.context?.percent ?? undefined },
        ...children.map(child => ({ key: `child:${child.id}`, name: child.id,
          hue: snapshot.build ? childHue(snapshot.build.fleet, child.id) : undefined,
          tokens: child.usage.input + child.usage.output, cost: child.usage.cost, contextPercent: child.usage.contextPercent }))],
      total: total ? { cost: total.total, partial: total.partial } : undefined } : undefined };
}
