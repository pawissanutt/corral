import { expect, test } from "bun:test";
import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import type { RewindInput } from "../bridge/protocol.ts";
import { BridgeRefusal } from "../bridge/protocol.ts";
import { CHECKPOINT_ENTRY } from "../capabilities/checkpoints.ts";
import { pastRewinds, rewindChoices, runRewind, turnPrompts, type TranscriptBranch } from "./rewind-picker.ts";

const HEAD = "1a2b3c4d5e6f";
const point = (turn: number, phase: "start" | "end", extra: Record<string, unknown> = {}) =>
  ({ id: `cp-${turn}-${phase}`, phase, turn, commit: `c${turn}`, head: `${HEAD}${turn}`, ...extra });
const listed = {
  checkpoints: [point(1, "start"), point(1, "end"), point(2, "start"), point(2, "end"), point(3, "start", { commit: undefined, error: "git failed" })],
  rewinds: [{ turn: 2, checkpoint: "cp-2-start", rescue_ref: "refs/worktree/corral/rescue/20261118T140200Z", from_head: "f", head: "h", at: "2026-11-18T14:02:00Z" }],
};
/** Pi's branch entries: each start checkpoint, then the message that started its turn. */
const branch: TranscriptBranch = { getBranch: () => [
  { type: "custom", customType: CHECKPOINT_ENTRY, data: point(1, "start") },
  { type: "message", message: { role: "user", content: "Make greet say Hello\nwith a capital H" } },
  { type: "custom", customType: CHECKPOINT_ENTRY, data: point(2, "start") },
  { type: "message", message: { role: "user", content: [{ type: "text", text: "Add a test for empty names" }] } },
] as unknown as SessionEntry[] };

test("choices are the turns with a saved start, newest first, named by what started them", () => {
  const choices = rewindChoices(listed, turnPrompts(branch));
  expect(choices).toEqual([
    { turn: 2, label: `Turn 2 · “Add a test for empty names” · head ${HEAD.slice(0, 7)}` },
    { turn: 1, label: `Turn 1 · “Make greet say Hello” · head ${HEAD.slice(0, 7)}` },
  ]);
  expect(rewindChoices({ checkpoints: "nonsense" }, new Map())).toEqual([]);
  expect(pastRewinds(listed, Date.parse("2026-11-18T15:00:00Z"))[0]).toMatch(/· back to turn 2 · saved work rescue\/20261118T140200Z$/);
});

/** A recording select and confirm that answer as told. */
function dialogs(pick: (options: string[]) => string | undefined, sure: boolean) {
  const asked: string[] = [];
  return { asked, ui: {
    select: async (title: string, options: string[]) => { asked.push(title); return pick(options); },
    confirm: async (title: string, message: string) => { asked.push(`${title}\n${message}`); return sure; },
  } };
}

test("a pick asks once, says the work is saved under a rescue ref, then rewinds and names the ref", async () => {
  const calls: RewindInput[] = [], notices: string[] = [];
  const rewind = async (input: RewindInput) => {
    calls.push(input);
    return "list" in input ? listed : { turn: 1, rescue_ref: "refs/worktree/corral/rescue/20261118T150000Z" };
  };
  const { asked, ui } = dialogs(options => options[1], true);
  await runRewind(ui, rewind, branch, (text, tone) => notices.push(`${tone}: ${text}`));
  expect(calls).toEqual([{ list: true }, { turn: 1 }]);
  expect(asked[0]).toContain("Earlier rewinds:");
  expect(asked[1]).toContain("Rewind to the start of turn 1?");
  expect(asked[1]).toContain("saved first under a rescue ref");
  expect(notices).toEqual(["success: Rewound to the start of turn 1 · current work saved as refs/worktree/corral/rescue/20261118T150000Z"]);
});

test("Esc at the pick or the confirm rewinds nothing; a refusal is a notice in its own words", async () => {
  const calls: RewindInput[] = [], notices: string[] = [];
  const rewind = async (input: RewindInput) => { calls.push(input); return listed; };
  await runRewind(dialogs(() => undefined, true).ui, rewind, branch, text => notices.push(text));
  await runRewind(dialogs(options => options[0], false).ui, rewind, branch, text => notices.push(text));
  expect(calls).toEqual([{ list: true }, { list: true }]);
  expect(notices).toEqual([]);
  const refused = async () => { throw new BridgeRefusal("rewind_unavailable", "rewind is available in a running worker with turn checkpoints"); };
  await runRewind(dialogs(() => undefined, true).ui, refused, branch, (text, tone) => notices.push(`${tone}: ${text}`));
  expect(notices).toEqual(["warning: Rewind unavailable: rewind is available in a running worker with turn checkpoints"]);
});
