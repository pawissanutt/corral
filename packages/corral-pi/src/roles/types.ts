// The role module interface (F6). A role is loaded by fixed path from
// src/roles/<role>/index.ts under the asset root; its default export is a RoleModule.
// Roles supply a tool loadout and contract prompt. The host only executes tools in the loadout.
import type { ExtensionAPI, ToolDefinition } from "@earendil-works/pi-coding-agent";
import type { TSchema } from "typebox";
import type { WebSettings } from "../extensions/web/settings.ts";
import type { McpSettings } from "../host/mcp-settings.ts";
import type { BuildContext, BuildProfile } from "../build/types.ts";

export const ROLES = ["worker", "reviewer", "assistant", "build"] as const;
export type Role = typeof ROLES[number];

/** Upstream Pi tools a role may activate (OD3). Nothing else upstream is ever active. */
export type UpstreamTool = "read" | "edit" | "write" | "bash" | "grep" | "find" | "ls";

export const UPSTREAM_TOOLS: readonly UpstreamTool[] = [
  "read", "edit", "write", "bash", "grep", "find", "ls",
];

/** The launcher-bound identity; never chosen by the model. */
export interface HostIdentity {
  readonly store: string;
  readonly project: string;
  readonly role: Role;
  readonly task?: string;
  readonly run?: string;
  /** Assistant only: the session `A-<n>`; `project` is its recorded project, or empty. */
  readonly assistant?: string;
}

/** Host abilities and settings; old Rust launches also carry snapshot metadata. */
export interface CapabilitySet {
  readonly catalog: number;
  readonly digest?: string;
  readonly preset?: string;
  readonly enabled: readonly string[];
  /** Resolved only when web.enabled is effective for this session. */
  readonly web?: WebSettings;
  /** Workspace-configured servers; each explicitly names its enabled projects. */
  readonly mcp?: McpSettings;
}

/** What a role module can see when it builds its loadout and contract. */
export interface RoleContext {
  readonly identity: HostIdentity;
  /** Present when the host enables the local todo ledger. */
  readonly todo?: boolean;
  /** Absent when an older corral launched this host without a snapshot. */
  readonly capabilities?: CapabilitySet;
  /** LSP server settings resolved for this session. */
  readonly lspSettings?: Readonly<Record<string, unknown>>;
  readonly registerCapability?: (pid: number) => () => void;
  /** The store root, independent of a project's or assistant scratch's tool cwd. */
  readonly workspaceRoot?: string;
  /** The directory Pi's tools work in. */
  readonly cwd: string;
  /** The run's prompt file, when dispatch wrote one. */
  readonly promptFile?: string;
  /** Build sessions only: the worktree the session writes in, its settings and its data directory. */
  readonly build?: BuildContext;
  /** Worker sessions only: named profiles its `task` children may select. */
  readonly childProfiles?: readonly BuildProfile[];
  readonly model: string;
  readonly effort?: string;
}

/** A model-callable tool backed by an engine command or host-local state. */
export interface CorralToolSpec {
  readonly name: string;
  readonly label: string;
  readonly description: string;
  readonly op: "done" | "verdict" | "artifact" | "todo" | "checkpoint";
  readonly parameters: TSchema;
  payload(params: Record<string, unknown>): Record<string, unknown>;
}

export interface RoleModule {
  readonly role: Role;
  /** Upstream tools the role activates; the host denies every other tool call. */
  readonly upstreamTools: readonly UpstreamTool[];
  /** Engine and host-local tools included in the role's loadout. */
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
   * The model-family working-style overlay for the session's current model, sent as its own
   * prompt section each turn; undefined leaves the section out. It never carries authority.
   */
  calibration?(provider: string, model: string): string | undefined;
  /**
   * The role's fixed shell refusals: checked for every bash call, top-level or nested, before any
   * configured bash rule, and never disabled by capability settings. A reason refuses the call.
   */
  readonly shellRefusal?: (command: string) => string | undefined;
  /** Optional role commands (slash commands). Tools registered here are never activated. */
  extend?(pi: ExtensionAPI, context: RoleContext): void;
}
