// The rewind picker (F2 › Rewind to an earlier turn…): the host's `rewind --list` (the transcript's turn checkpoints
// and the rewinds already made) as Pi's own select, drawn as the docked owner card; then one confirm that says the
// current work is saved under a rescue ref; then the host's `rewind {turn}`. The host owns every step: this module
// only words its list and its answer, and never touches Git or the transcript.
import type { ExtensionContext, ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import type { RewindInput } from "../bridge/protocol.ts";
import { CHECKPOINT_ENTRY } from "../capabilities/checkpoints.ts";
import type { Notice } from "./header.ts";
import { plainLabel } from "./panels-view.ts";

/** The transcript reader Pi hands an extension: only its current branch is read here. */
export type TranscriptBranch = Pick<ExtensionContext["sessionManager"], "getBranch">;

/** The host's rewind: `{list: true}` answers `{checkpoints, rewinds}`, `{turn}` the rewind it made. */
export type RewindCall = (input: RewindInput) => Promise<Record<string, unknown>>;

export interface RewindChoice { readonly turn: number; readonly label: string }

const PROMPT_CELLS = 60;
const RESCUE_PREFIX = "refs/worktree/corral/rescue/";

const record = (value: unknown): Record<string, unknown> | undefined =>
  value !== null && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : undefined;
const text = (value: unknown): string | undefined => typeof value === "string" && value.trim() ? value : undefined;
const short = (sha: unknown): string | undefined => text(sha)?.slice(0, 7);

/** `14:02`, or `Nov 16 14:02` on another day; the recorded text when it does not parse. */
function when(at: unknown, now: number): string {
  const date = new Date(String(at));
  if (Number.isNaN(date.getTime())) return plainLabel(String(at ?? ""));
  const time = date.toTimeString().slice(0, 5);
  return date.toDateString() === new Date(now).toDateString() ? time
    : `${date.toLocaleDateString("en-US", { month: "short", day: "numeric" })} ${time}`;
}

/**
 * What started each turn, by its start checkpoint's id: the first owner message after that checkpoint on the
 * current branch, first line only. A turn the agent started itself (a prompt file, automated input) has the text it
 * was given.
 */
export function turnPrompts(manager: TranscriptBranch | undefined): Map<string, string> {
  const prompts = new Map<string, string>();
  let waiting: string | undefined;
  for (const entry of manager?.getBranch() ?? []) {
    if (entry.type === "custom" && entry.customType === CHECKPOINT_ENTRY) {
      const data = record(entry.data);
      if (data?.["phase"] === "start" && text(data["id"])) waiting = String(data["id"]);
      continue;
    }
    if (waiting === undefined || entry.type !== "message" || entry.message.role !== "user") continue;
    const content = entry.message.content;
    const words = typeof content === "string" ? content
      : content.map(part => part.type === "text" ? part.text : "").join(" ");
    const line = plainLabel(words.split("\n").find(row => row.trim()) ?? "").trim();
    if (line) prompts.set(waiting, line.length > PROMPT_CELLS ? `${line.slice(0, PROMPT_CELLS - 1)}…` : line);
    waiting = undefined;
  }
  return prompts;
}

/**
 * Turns a rewind can return to, newest first: one per turn whose start checkpoint saved a commit and a head (the
 * host refuses any other), `Turn 3 · “what started it” · head 1a2b3c4`.
 */
export function rewindChoices(listed: unknown, prompts: ReadonlyMap<string, string>): RewindChoice[] {
  const points = Array.isArray(record(listed)?.["checkpoints"]) ? record(listed)!["checkpoints"] as unknown[] : [];
  const latest = new Map<number, Record<string, unknown>>();
  for (const value of points) {
    const point = record(value), turn = point?.["turn"];
    if (!point || point["phase"] !== "start" || typeof turn !== "number" || !text(point["commit"]) || !text(point["head"])) continue;
    latest.set(turn, point);
  }
  return [...latest].sort(([a], [b]) => b - a).map(([turn, point]) => {
    const prompt = prompts.get(String(point["id"]));
    return { turn, label: `Turn ${turn}${prompt ? ` · “${prompt}”` : ""} · head ${short(point["head"])}` };
  });
}

/** The rewinds already made in this session, oldest first: `14:02 · back to turn 2 · saved work rescue/…`. */
export function pastRewinds(listed: unknown, now: number): string[] {
  const rewinds = Array.isArray(record(listed)?.["rewinds"]) ? record(listed)!["rewinds"] as unknown[] : [];
  return rewinds.flatMap(value => {
    const past = record(value);
    if (!past) return [];
    const ref = text(past["rescue_ref"]);
    return [`${when(past["at"], now)} · back to turn ${String(past["turn"] ?? "?")}${ref ? ` · saved work ${plainLabel(ref.replace(RESCUE_PREFIX, "rescue/"))}` : ""}`];
  });
}

function reasonOf(error: unknown): string {
  const reason = record(error)?.["reason"];
  return plainLabel(typeof reason === "string" ? reason : error instanceof Error ? error.message : String(error));
}

/**
 * The whole flow: list, pick, confirm once, rewind. A refusal at any step is a notice in its words; Esc at the pick
 * or the confirm changes nothing.
 */
export async function runRewind(ui: Pick<ExtensionUIContext, "select" | "confirm">, rewind: RewindCall,
  manager: TranscriptBranch | undefined,
  notice: (text: string, tone: Notice["tone"], detail?: string) => void, now = Date.now()): Promise<void> {
  let listed: Record<string, unknown>;
  try { listed = await rewind({ list: true }); }
  catch (error) { notice(`Rewind unavailable: ${reasonOf(error)}`, "warning"); return; }
  const choices = rewindChoices(listed, turnPrompts(manager));
  if (!choices.length) { notice("No turn has a saved checkpoint to rewind to yet", "warning"); return; }
  const past = pastRewinds(listed, now);
  const title = ["Rewind to the start of which turn?", "The worktree and this conversation go back to where that turn began.",
    ...(past.length ? ["", "Earlier rewinds:", ...past] : [])].join("\n");
  const picked = await ui.select(title, choices.map(choice => choice.label));
  const choice = choices.find(item => item.label === picked);
  if (!choice) return;
  const sure = await ui.confirm(`Rewind to the start of turn ${choice.turn}?`,
    `Files and conversation return to the start of turn ${choice.turn}; the turns after it leave this conversation. `
    + `Your current work is saved first under a rescue ref (${RESCUE_PREFIX}…), named when the rewind finishes.`);
  if (!sure) return;
  try {
    const done = await rewind({ turn: choice.turn });
    const ref = text(done["rescue_ref"]);
    notice(`Rewound to the start of turn ${choice.turn}${ref ? ` · current work saved as ${plainLabel(ref)}` : ""}`, "success", JSON.stringify(done, null, 2));
  } catch (error) { notice(`Rewind refused: ${reasonOf(error)}`, "error"); }
}
