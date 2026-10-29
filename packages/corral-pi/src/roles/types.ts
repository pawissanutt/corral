// The role module interface (F6). A role is loaded by fixed path from
// src/roles/<role>/index.ts under the asset root; its default export is a RoleModule.
// Roles differ only in tool loadout, contract prompt and allowed corral operations (OD1).
// The Rust operation table stays the authority: a loadout is a convenience.
import type { ExtensionAPI, ToolDefinition } from "@earendil-works/pi-coding-agent";
import type { TSchema } from "typebox";

export type Role = "worker" | "reviewer" | "pm";

export const ROLES: readonly Role[] = ["worker", "reviewer", "pm"];

/** Upstream Pi tools a role may activate (OD3). Nothing else upstream is ever active. */
export type UpstreamTool = "read" | "edit" | "write" | "bash" | "grep" | "find" | "ls";

export const UPSTREAM_TOOLS: readonly UpstreamTool[] = [
  "read", "edit", "write", "bash", "grep", "find", "ls",
];

/** The enumerated v1 operations of `corral agent call --op`. */
export type CorralOp =
  | "hello" | "event" | "receipt_lookup" | "ask" | "withdraw" | "wait" | "progress"
  | "checkpoint" | "evidence" | "artifact" | "gate" | "done" | "verdict" | "todo"
  | "question_answer" | "question_escalate" | "question_cancel" | "reply" | "comment"
  | "capture" | "read_brief" | "read_next" | "read_show" | "read_list" | "read_search"
  | "read_route" | "read_artifacts" | "read_lessons";

export const CORRAL_OPS: readonly CorralOp[] = [
  "hello", "event", "receipt_lookup", "ask", "withdraw", "wait", "progress", "checkpoint",
  "evidence", "artifact", "gate", "done", "verdict", "question_answer", "question_escalate",
  "question_cancel", "reply", "comment", "capture", "read_brief", "read_next", "read_show",
  "read_list", "read_search", "read_route", "read_artifacts", "read_lessons", "todo",
];

/** The launcher-bound identity; never chosen by the model. */
export interface HostIdentity {
  readonly store: string;
  readonly project: string;
  readonly role: Role;
  readonly task?: string;
  readonly run?: string;
  readonly review?: string;
  readonly callback?: string;
  readonly generation: number;
}

/** What a role module can see when it builds its loadout and contract. */
export interface RoleContext {
  readonly identity: HostIdentity;
  /** Enabled only by the Rust launch snapshot; absent means disabled. */
  readonly todo?: boolean;
  /** The worktree (worker, reviewer) or workspace root (pm) Pi works in. */
  readonly cwd: string;
  /** The run's prompt file, when dispatch wrote one. */
  readonly promptFile?: string;
  readonly model: string;
  readonly effort?: string;
}

/** A model-callable corral tool: one corral operation with a typed payload. */
export interface CorralToolSpec<P = Record<string, unknown>> {
  readonly name: string;
  readonly label: string;
  readonly description: string;
  readonly op: CorralOp;
  readonly parameters: TSchema;
  /** The canonical payload sent for validated tool parameters. */
  payload(params: P): Record<string, unknown>;
}

export interface RoleModule {
  readonly role: Role;
  /** Upstream tools the role activates; the host denies every other tool call. */
  readonly upstreamTools: readonly UpstreamTool[];
  /** The corral operations the role's tools and lifecycle may call. */
  readonly corralOps: readonly CorralOp[];
  /** Model-callable corral tools; every op must be listed in corralOps. */
  tools(context: RoleContext): readonly CorralToolSpec[];
  /**
   * Names of first-party tools that run in the host process (the LSP extension's). The loader
   * refuses a name that is an upstream tool, a corral_ tool or a duplicate.
   */
  readonly localToolNames?: readonly string[];
  /** Definitions of exactly the localToolNames tools, in that order; they join the loadout. */
  localTools?(context: RoleContext): readonly ToolDefinition[];
  /** The role's contract, appended to the system prompt. */
  contract(context: RoleContext): string;
  /** Optional role commands (slash commands). Tools registered here are never activated. */
  extend?(pi: ExtensionAPI, context: RoleContext): void;
}
