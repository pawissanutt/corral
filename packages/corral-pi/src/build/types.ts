// The shared contract of build mode (`corral pi --build`). Types and constants only: nothing here
// imports the host session, the SDK runtime or the UI, so every build module can depend on it.
import type { HelperMessage } from "../extensions/subagent/inspection.ts";

/** What landing through corral requires for a project: independent review, the gate alone, or no corral exit. */
export type ExitPolicy = "review" | "gate" | "none";
export const EXIT_POLICIES: readonly ExitPolicy[] = ["review", "gate", "none"];

/** A named model choice for a child agent. `model` is `<provider>/<id>`; the provider may differ from the parent's. */
export interface BuildProfile {
  readonly name: string;
  readonly model: string;
  readonly effort?: string;
  readonly description?: string;
}

export interface BuildSettings {
  /** Unset in config means `review`; outside a corral workspace it is `none`. */
  readonly exitPolicy: ExitPolicy;
  /** Children running at once, 1 to 8. Default 3. */
  readonly maxParallel: number;
  /** Configured profiles. The runtime adds `default` (the parent's model) when it is absent. */
  readonly profiles: readonly BuildProfile[];
}
export const DEFAULT_MAX_PARALLEL = 3;

/**
 * The JSON `corral pi --build` writes to the host's inherited `--launch-fd`. Snake case on the wire:
 * `data_root`, `workspace_root`, `repo_root`. `capabilities` and `guidance` carry exactly the JSON
 * the rich launch message carries under the same names, resolved for the assistant role.
 */
export interface BuildLaunch {
  readonly version: 1;
  readonly assets: string;
  /** `$XDG_DATA_HOME/corral/corral-pi`; build sessions live under `<dataRoot>/build/`. */
  readonly dataRoot: string;
  /** The corral executable for the exit and board commands. */
  readonly corral: string;
  /** The corral workspace root, when launched inside one. */
  readonly workspaceRoot?: string;
  /** The configured project that owns `repoRoot`, when there is one. */
  readonly project?: string;
  /** The Git top-level the session builds in. */
  readonly repoRoot: string;
  /** Write in the current checkout and branch instead of a new worktree. */
  readonly here: boolean;
  readonly name?: string;
  /** A build session id to resume instead of starting one. */
  readonly resume?: string;
  readonly provider: string;
  readonly model: string;
  readonly effort?: string;
  readonly capabilities?: unknown;
  readonly guidance?: unknown;
  readonly settings: BuildSettings;
}

/** `<dataRoot>/build/<id>/session.json`, rewritten atomically. The launcher lists these to resume. */
export interface BuildSessionRecord {
  readonly version: 1;
  /** `B-` plus lowercase base36. */
  readonly id: string;
  readonly name: string;
  readonly created: string;
  readonly updated: string;
  readonly repoRoot: string;
  /** Where tools write: `<repoRoot>/.worktrees/build-<name>`, or `repoRoot` when `here`. */
  readonly worktree: string;
  /** `build/<name>` for a created worktree; the checked-out branch, or null when detached, for `here`. */
  readonly branch: string | null;
  /** The commit the session started from. */
  readonly base: string | null;
  readonly here: boolean;
  readonly project?: string;
  readonly workspaceRoot?: string;
  /** The Pi transcript of the parent conversation. */
  readonly sessionFile?: string;
  readonly exit?: ExitRecord;
}

/** What the host and role see of the running build session. */
export interface BuildContext {
  readonly session: BuildSessionRecord;
  readonly settings: BuildSettings;
  /** `<dataRoot>/build/<id>`: transcripts, journal and child session files. */
  readonly dir: string;
  readonly corral: string;
}

// ---------- children ----------

export type ChildState = "queued" | "running" | "needs" | "done" | "failed" | "cancelled";
export const LIVE_CHILD_STATES: readonly ChildState[] = ["queued", "running", "needs"];

/** One slice the parent hands to a child. */
export interface ChildSpec {
  /** Lowercase slug, unique within the session: `host-slots`. */
  readonly id: string;
  /** What the slice delivers, one line. */
  readonly title: string;
  /** The whole instruction; a child inherits nothing else from the conversation. */
  readonly brief: string;
  /** Repository-relative paths (files or directories) the child owns. An edit outside is flagged, not blocked. */
  readonly owns: readonly string[];
  /** A profile name; absent means `default`. */
  readonly profile?: string;
  /** Child ids that must finish first. */
  readonly after?: readonly string[];
}

export interface ChildEdit {
  /** Repository-relative. */
  readonly path: string;
  /** True when the path is outside the child's `owns`. */
  readonly outside: boolean;
  readonly at: number;
}

export interface ChildUsage {
  readonly input: number;
  readonly output: number;
  /** API-equivalent USD; absent when the model has no price. */
  readonly cost?: number;
  readonly contextPercent?: number;
  readonly turns: number;
}

export interface ChildQuestion {
  readonly question: string;
  readonly options: readonly { readonly label: string; readonly detail?: string }[];
  readonly askedAt: number;
}

export interface ChildSnapshot {
  readonly id: string;
  /** The parent tool call that started it. */
  readonly call: string;
  readonly title: string;
  readonly brief: string;
  readonly owns: readonly string[];
  readonly after: readonly string[];
  readonly profile: string;
  /** `<provider>/<id>` in use now. */
  readonly model: string;
  /** A model chosen while a turn was streaming; it applies from the next request. */
  readonly pendingModel?: string;
  readonly effort?: string;
  readonly state: ChildState;
  readonly createdAt: number;
  readonly startedAt?: number;
  readonly endedAt?: number;
  /** The tool call in flight, as `edit crates/host/src/slots.rs`. */
  readonly activity?: string;
  readonly edits: readonly ChildEdit[];
  readonly usage: ChildUsage;
  /** Messages the owner or parent sent it while it ran, oldest first. */
  readonly steers: readonly string[];
  readonly question?: ChildQuestion;
  /** Its final answer, capped at 32,768 characters. */
  readonly report?: string;
  readonly error?: string;
  readonly sessionFile?: string;
  /** Tool calls started per 5-second bucket since `startedAt`, capped at 7, last 120 buckets. */
  readonly pulse: readonly number[];
  /** One character per 5-second bucket since the fleet started: `q` queued, `r` running, `n` needs, `d` done, `f` failed, `c` cancelled. Last 240. */
  readonly history: string;
  /** Retained conversation, with the same caps as helper history. */
  readonly messages: readonly HelperMessage[];
}

export interface FleetSnapshot {
  readonly children: readonly ChildSnapshot[];
  readonly maxParallel: number;
  /** When the first child of this session was created. */
  readonly startedAt?: number;
}

/** What the host and the UI may do to children. Every method is safe to call for an unknown or finished id. */
export interface FleetControl {
  snapshot(): FleetSnapshot;
  /** Delivers `text` to a running child at its next step; false when the child is not live. */
  steer(id: string, text: string): Promise<boolean>;
  cancel(id: string): boolean;
  /** `model` is `<provider>/<id>` or a profile name; throws on an unknown or unapproved model. */
  setModel(id: string, model: string): void;
  /** Answers the child's open question; false when it has none. */
  answer(id: string, text: string): boolean;
  /** `<provider>/<id>` values a child may be switched to. */
  models(): readonly string[];
  profiles(): readonly BuildProfile[];
  subscribe(listener: () => void): () => void;
}

// ---------- exit ----------

export type ExitChoice = "keep" | "land" | "ship" | "iterate";

export interface ExitOption {
  readonly choice: ExitChoice;
  readonly label: string;
  readonly detail: string;
  /** Why the option cannot be chosen; absent when it can. */
  readonly unavailable?: string;
}

export interface BuildSummary {
  readonly branch: string | null;
  readonly base: string | null;
  readonly head: string | null;
  readonly commits: number;
  readonly files: number;
  readonly added: number;
  readonly removed: number;
  /** True when the worktree has uncommitted changes. */
  readonly dirty: boolean;
  readonly slices: readonly { readonly id: string; readonly state: ChildState; readonly seconds: number; readonly cost?: number }[];
  /** Files edited by a child outside its `owns`. */
  readonly outside: readonly { readonly path: string; readonly by: readonly string[] }[];
}

export interface ExitRecord {
  readonly choice: ExitChoice;
  readonly at: string;
  /** The corral task created by `ship` or `land`. */
  readonly task?: string;
  /** The assistant session corral created to carry the branch. */
  readonly assistant?: string;
  readonly reviewWaived?: boolean;
}

export type ExitPhase = "none" | "pending" | "running" | "kept" | "shipped" | "landing" | "landed" | "failed";

export interface ExitState {
  readonly phase: ExitPhase;
  readonly record?: ExitRecord;
  /** Progress or failure text for the conversation. */
  readonly detail?: string;
}

export interface ExitControl {
  options(): readonly ExitOption[];
  summary(): Promise<BuildSummary>;
  state(): ExitState;
  /** Runs the chosen exit. Only the owner's dialog calls this; no model-callable tool does. */
  choose(choice: ExitChoice): Promise<ExitState>;
  subscribe(listener: () => void): () => void;
}

// ---------- board fan-out ----------

export interface BoardTaskPlan {
  /** Local key, unique in the plan; `depends` refers to keys or existing `T-<n>` ids. */
  readonly key: string;
  readonly title: string;
  readonly kind?: string;
  readonly size?: string;
  readonly profile?: string;
  readonly depends: readonly string[];
  readonly body: string;
}

export interface BoardPlan {
  readonly title: string;
  readonly project?: string;
  readonly why: string;
  readonly tasks: readonly BoardTaskPlan[];
}

export type BoardPhase = "backlog" | "waiting" | "queued" | "running" | "gating" | "reviewing" | "landed" | "blocked" | "failed";

export interface BoardTaskStatus {
  readonly id: string;
  readonly title: string;
  /** corral's task status, verbatim. */
  readonly status: string;
  readonly phase: BoardPhase;
  readonly depends: readonly string[];
  /** What it waits for, or why it is blocked. */
  readonly detail?: string;
  readonly updatedAt: number;
}

export interface BoardPreview {
  readonly tracker: string;
  readonly tasks: readonly { readonly key: string; readonly id: string; readonly title: string; readonly depends: readonly string[] }[];
}

export interface BoardControl {
  /** Tasks this session filed, as last read. */
  tasks(): readonly BoardTaskStatus[];
  /** A plan the model proposed and the owner has not answered. */
  proposed(): BoardPlan | undefined;
  preview(plan: BoardPlan): Promise<BoardPreview>;
  /** Files the plan; with `start`, queues every filed task. Only the owner's dialog calls this. */
  file(plan: BoardPlan, start: boolean): Promise<readonly BoardTaskStatus[]>;
  dismiss(): void;
  refresh(): Promise<void>;
  subscribe(listener: () => void): () => void;
}

// ---------- what the workbench reads ----------

/** `"parent"` or a child id: who the composer's Enter reaches. */
export type MessageTarget = string;

export interface BuildPanelState {
  readonly session: BuildSessionRecord;
  readonly settings: BuildSettings;
  readonly fleet: FleetSnapshot;
  readonly target: MessageTarget;
  readonly exit: ExitState;
  readonly board: readonly BoardTaskStatus[];
}

/** Handed to the native panels of a build session; absent in every other role. */
export interface BuildUi {
  readonly fleet: FleetControl;
  readonly exit: ExitControl;
  readonly board: BoardControl;
  target(): MessageTarget;
  setTarget(target: MessageTarget): void;
}
