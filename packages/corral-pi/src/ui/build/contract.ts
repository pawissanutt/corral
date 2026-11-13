import type { BoardPlan, BuildPanelState, BuildSummary, FleetSnapshot } from "../../build/types.ts";

export type BuildTone = "text" | "muted" | "accent" | "success" | "warning" | "error";

/** Who Enter reaches, as the UI keeps it. Absent: the parent. `answer`: Enter answers the child's open question. */
export interface ComposerTarget { readonly id: string; readonly answer: boolean }

export const CHILD_PREFIX = "child:";
export function childKey(id: string): string { return CHILD_PREFIX + id; }
export function childIdOf(key: string): string | undefined {
  return key.startsWith(CHILD_PREFIX) ? key.slice(CHILD_PREFIX.length) : undefined;
}
/** Fleet creation order is a child's hue, so one child has one colour on every surface. */
export function childHue(fleet: FleetSnapshot | undefined, id: string): number {
  const index = fleet?.children.findIndex(child => child.id === id) ?? -1;
  return index < 0 ? 0 : index;
}

/** What the conversation cards read. The integration implements it with BuildController. */
export interface BuildCardSource {
  state(): BuildPanelState | undefined;
  /** The branch summary read for the current exit episode; undefined while it is read. */
  summary(): BuildSummary | undefined;
  /** Tool call id or entry id of the exit card that shows live progress. */
  latestFinish(): string | undefined;
  color(hue: number): string;
  now(): number;
  /** Redraws a live card on fleet, exit or board changes until the returned function is called. */
  watch(id: string, invalidate: () => void): () => void;
  question(id: string): void;
  exit(): void;
  plan(): void;
  openChild(id: string): void;
  /** The board plan waiting for the owner's confirmation, if any. */
  proposed?(): BoardPlan | undefined;
}
