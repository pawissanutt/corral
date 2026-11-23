// The shared contract of `corral-pi build`. Types and constants only: nothing here
// imports the host session, the SDK runtime or the UI, so every build module can depend on it.
import type { AwsBinding } from "../auth/aws.ts";
import type { HelperMessage } from "../extensions/subagent/inspection.ts";
import type { BuildRecord } from "./record-types.ts";
import type { ProviderId } from "../auth/providers.ts";
import type { ModelChoice } from "../host/config.ts";

export interface BuildRoute {
  readonly provider: ProviderId;
  readonly model: string;
  readonly effort?: string;
  readonly fallback?: readonly ModelChoice[];
  readonly aws?: AwsBinding;
}

/** What landing from the session requires for a project: an approving in-session review and the full gate, the full gate alone, or no landing. */
export type ExitPolicy = "review" | "gate" | "none";
export const EXIT_POLICIES: readonly ExitPolicy[] = ["review", "gate", "none"];

/** A named model choice for a child agent. `model` is `<provider>/<id>`; the provider may differ from the parent's. */
export interface BuildProfile {
  readonly name: string;
  readonly model: string;
  readonly effort?: string;
  readonly description?: string;
}

export interface BuildDelegationRoute {
  readonly match: string;
  readonly kind: "explore" | "slice";
  readonly profile: string;
  readonly strategy: "when_delegating" | "prefer_child";
}

export interface BuildDelegation {
  readonly mode: "standard" | "proactive";
  readonly routes: readonly BuildDelegationRoute[];
}

export interface BuildSettings {
  /** Unset in config means `review`; outside a corral workspace it is `none`. */
  readonly exitPolicy: ExitPolicy;
  /** Children running at once, 1 to `MAX_PARALLEL_LIMIT`. Default 32. */
  readonly maxParallel: number;
  /** Configured profiles. The runtime adds `default` (the parent's model) when it is absent. */
  readonly profiles: readonly BuildProfile[];
  readonly delegation?: BuildDelegation;
}
export const DEFAULT_MAX_PARALLEL = 32;
export const MAX_PARALLEL_LIMIT = 32;
/** Children one `task` call may start. */
export const MAX_CHILDREN_PER_CALL = 32;

/** Options derived from the local build flags, before choosing the startup model. */
export interface BuildLaunchOptions extends Partial<BuildRoute> {
  readonly version: 1;
  readonly assets: string;
  /** `$XDG_DATA_HOME/corral/corral-pi`; build sessions live under `<dataRoot>/build/`. */
  readonly dataRoot: string;
  /** Bare sessions never have a corral executable or workspace binding. */
  readonly bare?: boolean;
  readonly corral?: string;
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
  readonly settings: BuildSettings;
}

/** A new build with its route resolved, ready to make a session record. */
export interface BuildLaunch extends BuildLaunchOptions {
  readonly provider: ProviderId;
  readonly model: string;
}

/** `<dataRoot>/build/<id>/session.json`, rewritten atomically. The launcher lists these to resume. */
export interface BuildSessionRecord {
  readonly version: 1;
  /** `B-` plus lowercase base36. */
  readonly id: string;
  readonly name: string;
  /** Explicit display title supplied at creation, independent of the worktree slug. */
  readonly title?: string;
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
  /** Absent on older records; false only for bare sessions started outside Git. */
  readonly repository?: boolean;
  readonly bare?: boolean;
  /** The branch checked out when a bare session started. */
  readonly baseBranch?: string | null;
  readonly project?: string;
  readonly workspaceRoot?: string;
  /** Frozen route identity; resume does not consult mutable profile defaults. */
  readonly route?: BuildRoute;
  /** Actual bridge address, including any explicit --socket. */
  readonly socket?: string;
  /** Written before binding, so cleanup also sees a host in startup. */
  readonly pid?: number;
  /** A retained record no longer owns its old worktree or branch name. */
  readonly cleaned?: true;
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

/**
 * What a child may do. `slice` writes inside the shared worktree; `explore` is read-only research;
 * `review` is read-only and ends by recording a verdict.
 */
export type ChildKind = "slice" | "explore" | "review";

/** One slice the parent hands to a child. */
export interface ChildSpec {
  /** Absent means `slice`. */
  readonly kind?: ChildKind;
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

/** One message between agents of a session, or to or from another session. Kept in the fleet journal. */
export interface AgentMessage {
  /** 1-based, in send order, unique within the session. */
  readonly seq: number;
  /** Epoch ms. */
  readonly at: number;
  /** A child id, `parent`, `owner`, or a cross-session recipient key. */
  readonly from: string;
  /** A child id, `parent`, `all`, or a cross-session recipient key. */
  readonly to: string;
  /** At most `AGENT_MESSAGE_MAX` characters. */
  readonly text: string;
  /**
   * `delivered`: injected at the recipient's next step. `queued`: the recipient has not started; it
   * reads the message in its opening prompt. `undeliverable`: no such live recipient.
   * `answered_from_report`: the recipient had finished, so the sender received its final report.
   */
  readonly delivery: "delivered" | "queued" | "undeliverable" | "answered_from_report";
  /** The `seq` this message answers, when the sender replied to a `waitReply` message. */
  readonly replyTo?: number;
}
export const AGENT_MESSAGE_MAX = 8192;
/** Messages one child may send in its lifetime. */
export const AGENT_SEND_BUDGET = 24;
/** Messages the fleet journal retains; older ones are dropped from the front. */
export const AGENT_JOURNAL_MAX = 500;

/** What `agent_message` returns to its caller. */
export interface MessageOutcome {
  readonly message: AgentMessage;
  /** The recipient's answer when the call waited for one. */
  readonly reply?: string;
  /** The recipient's final report, when `delivery` is `answered_from_report`. */
  readonly report?: string;
  /** Why a wait ended without an answer: the timeout elapsed, or waiting would have closed a cycle of agents waiting on each other. */
  readonly unanswered?: "timeout" | "would_deadlock" | "recipient_finished";
}

/** One line of the roster every agent can read. */
export interface RosterEntry {
  /** A child id or `parent`. */
  readonly id: string;
  readonly kind: ChildKind | "parent";
  readonly title: string;
  readonly owns: readonly string[];
  readonly state: ChildState | "live";
}

export interface ReviewFinding {
  readonly severity: "blocker" | "major" | "minor";
  /** Repository-relative. */
  readonly path?: string;
  readonly line?: number;
  readonly text: string;
}

/** What a `review` child records with its typed verdict tool. The host stores it as given; the parent cannot edit it. */
export interface ReviewVerdict {
  readonly verdict: "approve" | "changes";
  readonly summary: string;
  readonly findings: readonly ReviewFinding[];
  /** The commit the reviewer read, when it named one. */
  readonly head?: string;
  readonly at: number;
}

export interface ChildSnapshot {
  readonly id: string;
  readonly kind: ChildKind;
  /** Present once a `review` child has recorded its verdict. */
  readonly verdict?: ReviewVerdict;
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
  /** When each steer was sent (epoch ms), index for index with `steers`; absent in snapshots that predate it. */
  readonly steeredAt?: readonly number[];
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
  /** Messages dropped from the front of `messages` by the retention limit; absent when none were. */
  readonly dropped?: number;
}

export interface FleetSnapshot {
  readonly children: readonly ChildSnapshot[];
  /** The session's agent messages, oldest first, at most `AGENT_JOURNAL_MAX`. */
  readonly mail: readonly AgentMessage[];
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
  /**
   * Sends `text` from one agent to another. `to` is a child id, `parent` or `all`. With `waitReply`
   * the promise resolves when the recipient answers, the timeout elapses or the wait is refused.
   */
  message(from: string, to: string, text: string, options?: { readonly waitReply?: boolean; readonly timeoutMs?: number; readonly replyTo?: number; readonly signal?: AbortSignal }): Promise<MessageOutcome>;
  roster(): readonly RosterEntry[];
  /** Resolves with the named children's snapshots once each has settled; an unknown id rejects. */
  wait(ids: readonly string[], signal?: AbortSignal): Promise<readonly ChildSnapshot[]>;
  subscribe(listener: () => void): () => void;
}

// ---------- exit ----------

/** `land` fast-forwards an explicitly offered, already prepared candidate. */
export type ExitChoice = "keep" | "land" | "iterate";

export interface ExitOption {
  readonly choice: ExitChoice;
  readonly label: string;
  readonly detail: string;
  /** Why the option cannot be chosen; absent when it can. */
  readonly unavailable?: string;
  /** Set on `land` when review findings are open or no approval covers the head: the owner must give a reason to land anyway. */
  readonly needsReason?: string;
  /** Opaque ticket for this exact offered candidate; must be echoed when choosing Land. */
  readonly authorization?: string;
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
  /** The base branch commit after `land`. */
  readonly landed?: string;
  readonly baseBranch?: string;
  /** The full-tier gate attempt that vouched for the landed head: `G3`. */
  readonly gateAttempt?: string;
  /** Why the owner landed over open review findings. */
  readonly overrideReason?: string;
}

/** `rebasing` is explicit preparation, before `pending` offers the owner's choice. `verifying` and `repairing` are retained for compatibility. */
export type ExitPhase = "none" | "pending" | "rebasing" | "verifying" | "repairing" | "landing" | "kept" | "landed" | "failed";

/** One step of `land` as the owner follows it. Times are milliseconds since the epoch. */
export interface LandStep {
  readonly id: "rebase" | "gate" | "land";
  readonly state: "waiting" | "running" | "done" | "failed";
  /** A short fact: the rebased head, the gate attempt, what stopped it. */
  readonly note?: string;
  readonly startedAt?: number;
  readonly endedAt?: number;
}

/** The gate `land` is running or last ran. */
export interface LandGate {
  /** `G3`, once the attempt is recorded. */
  readonly attempt?: string;
  /** Absolute path of the combined command log. */
  readonly log?: string;
  /** The command running, or the one that failed, and its 1-based place among the gate's commands. */
  readonly command?: string;
  readonly index?: number;
  readonly of?: number;
  /** The log's last lines without terminal sequences; empty once the gate passed. */
  readonly tail: readonly string[];
}

/** Preparation, existing gate evidence, and fast-forward progress. */
export interface LandProgress {
  readonly base: string;
  readonly steps: readonly LandStep[];
  readonly gate?: LandGate;
}

/** Immutable Git identity of a prepared landing candidate. `base` is the tip SHA, never a ref name. */
export interface PreparedLanding {
  readonly head: string;
  readonly base: string;
  readonly branch: string;
  /** Identity of the source branch, integration target, and live review/check policy. */
  readonly reviewScope?: string;
}

export type LandCandidateState = "preparing" | "ready" | "conflicts" | "needs_review" | "needs_gate" | "gate_failed" | "base_moved" | "failed";
export interface LandCandidate {
  readonly state: LandCandidateState;
  readonly base?: string;
  readonly baseTip?: string;
  readonly head?: string;
  readonly source?: string;
  readonly conflicts?: readonly string[];
  readonly gateAttempt?: string;
  readonly movedTo?: string;
  readonly detail?: string;
}

export interface ExitState {
  readonly phase: ExitPhase;
  readonly record?: ExitRecord;
  /** Progress or failure text for the conversation. */
  readonly detail?: string;
  readonly repair?: { readonly round: number; readonly of: number };
  /** Present from the first step of `land` until the next exit choice opens. */
  readonly land?: LandProgress;
  readonly candidate?: LandCandidate;
}

export interface ExitControl {
  /** Explicit owner Finish, sharing preparation and evidence with build_finish. */
  requestFinish?(): Promise<void>;
  options(): readonly ExitOption[];
  summary(): Promise<BuildSummary>;
  state(): ExitState;
  /** Runs the owner's choice. Land needs the ticket from its displayed option and, when indicated, an override reason. */
  choose(choice: ExitChoice, options?: { readonly authorization?: string; readonly overrideReason?: string }): Promise<ExitState>;
  subscribe(listener: () => void): () => void;
}

// ---------- board fan-out ----------

export const TASK_SIZES = ["xs", "s", "m", "l", "xl"] as const;
export const TASK_DIFFICULTIES = ["routine", "standard", "hard"] as const;
export type TaskDifficulty = typeof TASK_DIFFICULTIES[number];

export interface BoardTaskMetadata {
  readonly size?: string;
  readonly difficulty?: TaskDifficulty;
  readonly tags?: readonly string[];
}

export interface BoardTaskPlan extends BoardTaskMetadata {
  /** Local key, unique in the plan; `depends` refers to keys or existing `T-<n>` ids. */
  readonly key: string;
  readonly title: string;
  readonly kind?: string;
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

export interface BoardTaskStatus extends BoardTaskMetadata {
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
  readonly tasks: readonly (BoardTaskMetadata & { readonly key: string; readonly id: string; readonly title: string; readonly depends: readonly string[] })[];
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
  /** The session's tracked record as it stands now: verification attempts, review rounds, messages, decisions. */
  readonly record: BuildRecord;
  readonly board: readonly BoardTaskStatus[];
}

/** Handed to the native panels of a build session; absent in every other role. */
export interface BuildUi {
  readonly fleet: FleetControl;
  readonly exit: ExitControl;
  /** The same record `BuildPanelState.record` carries, read on demand. */
  record(): BuildRecord;
  /** Absent in a bare session, which has no board connection. */
  readonly board?: BoardControl;
  target(): MessageTarget;
  setTarget(target: MessageTarget): void;
}
