// The role module interface (F6). A role is loaded by fixed path from
// src/roles/<role>/index.ts under the asset root; its default export is a RoleModule.
// Roles differ only in tool loadout, contract prompt and allowed corral operations (OD1).
// The Rust operation table stays the authority: a loadout is a convenience.
import type { ExtensionAPI, ToolDefinition } from "@earendil-works/pi-coding-agent";
import type { TSchema } from "typebox";
import type { WebSettings } from "../extensions/web/settings.ts";
import type { McpSettings } from "../host/mcp-settings.ts";

export type Role = "worker" | "reviewer" | "pm" | "planner" | "plan_reviewer" | "assistant";

export const ROLES: readonly Role[] = ["worker", "reviewer", "pm", "planner", "plan_reviewer", "assistant"];

/** The native planning roles: each runs only while its `role_enabled` capability is on. */
export const PLANNING_ROLES: readonly Role[] = ["planner", "plan_reviewer"];

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
  | "read_content" | "read_route" | "read_artifacts" | "read_lessons" | "read_annotations" | "inbox_read" | "inbox_resolve"
  | "memory_list" | "memory_read" | "memory_put" | "memory_remove"
  | "job" | "job_monitor"
  | "plan_write" | "plan_submit" | "plan_verdict" | "owner_action" | "write_permission"
  | "assistant_plan" | "assistant_plan_review" | "assistant_handoff";

export const CORRAL_OPS: readonly CorralOp[] = [
  "hello", "event", "receipt_lookup", "ask", "withdraw", "wait", "progress", "checkpoint",
  "evidence", "artifact", "gate", "done", "verdict", "question_answer", "question_escalate",
  "question_cancel", "reply", "comment", "capture", "read_brief", "read_next", "read_show",
  "read_list", "read_search", "read_content", "read_route", "read_artifacts", "read_lessons", "read_annotations", "inbox_read", "inbox_resolve",
  "todo", "job", "job_monitor", "memory_list", "memory_read", "memory_put", "memory_remove",
  "plan_write", "plan_submit", "plan_verdict", "owner_action", "write_permission",
  "assistant_plan", "assistant_plan_review", "assistant_handoff",
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
  /** Assistant only: the session `A-<n>`; `project` is its recorded project, or empty. */
  readonly assistant?: string;
  readonly generation: number;
}

/**
 * The generation's capability snapshot as corral resolved and recorded it (T-563). The Rust
 * catalog owns keys, defaults and role ceilings; a host only reads which keys are enabled.
 */
export interface CapabilitySet {
  readonly catalog: number;
  readonly digest: string;
  readonly preset: string;
  readonly enabled: readonly string[];
  /** Resolved only when web.enabled is effective for this generation. */
  readonly web?: WebSettings;
  /** Workspace-configured servers; each explicitly names its enabled projects. */
  readonly mcp?: McpSettings;
}

/** What a role module can see when it builds its loadout and contract. */
export interface RoleContext {
  readonly identity: HostIdentity;
  /** Enabled only by the Rust launch snapshot; absent means disabled. */
  readonly todo?: boolean;
  /** Absent when an older corral launched this host without a snapshot. */
  readonly capabilities?: CapabilitySet;
  /** LSP server settings resolved for this generation. */
  readonly lspSettings?: Readonly<Record<string, unknown>>;
  /** Fresh canonical assistant scratch roots; never sourced from legacy files. */
  readonly scratchRoots?: () => Promise<readonly string[]>;
  readonly writePermission?: {
    readonly settings: import("./assistant/paths.ts").WriteSettings;
    onBound(notice: () => Promise<void>): Promise<void>;
    sessionId(): string;
    turn(): { id: number | undefined; signal: AbortSignal };
    call(requestId: string, payload: Record<string, unknown>): Promise<import("../tools/corral.ts").CorralReceipt>;
    emit(kind: string, data: Record<string, unknown>): void;
  };
  readonly registerCapability?: (pid: number) => () => void;
  /** The store root, independent of a project's or assistant scratch's tool cwd. */
  readonly workspaceRoot?: string;
  /** The directory Pi's tools work in. */
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
  /** Machine shape of operation results; the canonical receipt envelope is supplied centrally. */
  readonly resultSchema?: TSchema;
  /** The canonical payload sent for validated tool parameters. */
  payload(params: P): Record<string, unknown>;
  /** A deadline longer than the client's default, for an op that runs a whole corral command. */
  readonly timeoutMs?: number;
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
  /**
   * The role's fixed shell refusals: checked for every bash call, top-level or nested, before any
   * configured bash rule, and never disabled by capability settings. A reason refuses the call.
   */
  readonly shellRefusal?: (command: string) => string | undefined;
  /** Optional role commands (slash commands). Tools registered here are never activated. */
  extend?(pi: ExtensionAPI, context: RoleContext): void;
}
