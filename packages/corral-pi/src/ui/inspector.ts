// What the host reports about itself for read-only inspection: its session, turn, effective configuration and its
// bounded event log, which the Task › Events timeline (lifecycle-view.ts) lists. Nothing here is task settlement.
import type { EventPage } from "../bridge/events.ts";
import type { CapabilitySet } from "../roles/types.ts";

export interface InspectorSnapshot {
  readonly session: string;
  readonly boundSession: string | null;
  readonly turn: number;
  readonly turnOpen: boolean;
  readonly capabilities: Pick<CapabilitySet, "catalog" | "digest" | "preset" | "enabled"> | null;
  readonly guidance: {
    readonly projectText: boolean | null;
    readonly skillPaths: readonly string[];
    readonly promptPaths: readonly string[];
    readonly prompts: readonly string[];
    readonly skills: readonly string[];
  };
  readonly settings: readonly string[];
  readonly events: EventPage;
}
