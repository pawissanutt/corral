// Runs of one kind: Pi's transcript folds consecutive settled successful tool rows of one kind into one group row
// (`TranscriptContainer` in the SDK's tool-execution.js asks each row for its member; tool-row.ts offers it). A kind
// is one tool and the row's verb (`Read`, `Resolved inbox item`), except plan updates, which fold across their
// operations. A kind with a summary of its own (the table below) folds from two rows, any other from three, as
// `✓ <verb> ×n · <newest outcome>` with the total time on the right. A click on the group row shows its rows.
// Presentation only: the rows' models are read, never changed.
import { visibleWidth } from "@earendil-works/pi-tui";

import { clipMiddle, clipPlain, stringArg, timeOf } from "./tool-row.ts";
import type { GroupEntry, GroupKind, RowGrouping, RowModel } from "./tool-row.ts";

/** Rows that fold under the generic summary. */
const GENERIC_MIN = 3;
/** Rows that fold under a summary of their own. */
const SUMMARY_MIN = 2;

/** The group row's words; the kit adds the `✓` and the total time. */
type Words = Pick<RowModel, "verb" | "target" | "targetClip" | "outcome" | "view">;
/** Words for `entries` (oldest first) on a row `room` cells wide. */
type Summarize = (entries: readonly GroupEntry[], room: number) => Words;
interface Summary {
  /** `tool`: every row of the tool is one kind whatever its verb (plan updates). */
  readonly key?: "tool";
  readonly summarize: Summarize;
}
/** How a listed name clips: file names in the middle, so the extension stays. */
type Clip = "middle" | "end";

function plural(count: number, one: string, many = `${one}s`): string {
  return `${count.toLocaleString("en-US")} ${count === 1 ? one : many}`;
}
function newest(entries: readonly GroupEntry[]): RowModel {
  return entries[entries.length - 1]!.model;
}
/** The newest view label of the run, so the group row still opens the view its rows open. */
function viewOf(entries: readonly GroupEntry[]): Pick<RowModel, "view"> {
  const view = [...entries].reverse().find(entry => entry.model.view)?.model.view;
  return view ? { view } : {};
}
/** The total time of the run's calls, when it reads as more than nothing. */
function durationOf(entries: readonly GroupEntry[]): Pick<RowModel, "duration"> {
  const total = entries.reduce((sum, entry) => sum + (typeof entry.model.duration === "number" ? entry.model.duration : entry.call.durationMs ?? 0), 0);
  return total >= 100 ? { duration: total } : {};
}
/** A file target without its `:from-to` line range. */
function wholeFile(target: string): string {
  return target.replace(/:\d+(-\d+)?$/, "");
}
/** A target's last path segment, without a line range. */
function fileName(target: string): string {
  const path = wholeFile(target);
  return path.split("/").filter(Boolean).at(-1) ?? path;
}
/** A paged read's target without its page (`T-935 report.md · page 2 of 4` → `T-935 report.md`). */
function unpaged(target: string): string {
  return target.replace(/ · page \d+( of \d+)?$/, "").replace(/ from byte \d+$/, "");
}
/** A search's pattern: its quoted part when it has one (`"foo" in src` → `"foo"`). */
function pattern(target: string): string {
  return /^"[^"]*"/.exec(target)?.[0] ?? target;
}

/** Cells left for the outcome on a row `room` wide after the glyph, verb, target, time and view, as the kit lays it out. */
function outcomeRoom(room: number, words: Words, entries: readonly GroupEntry[]): number {
  const time = timeOf({ state: "ok", verb: words.verb, ...durationOf(entries) }, 0);
  const right = [time, words.view?.label ?? ""].filter(Boolean).reduce((sum, part, index) => sum + (index ? 2 : 0) + visibleWidth(part), 0);
  return room - 2 - visibleWidth(words.verb) - (words.target ? 1 + visibleWidth(words.target) : 0) - 3 - (right ? 2 + right : 0);
}
/**
 * `a.ts, b.ts, … 3 more`: as many whole names as fit in `budget` cells; when not even two fit, the first one alone,
 * clipped to what is left.
 */
function listing(names: readonly string[], budget: number, clip: Clip): string {
  for (let count = names.length; count > 1; count--) {
    const text = names.slice(0, count).join(", ") + (count < names.length ? `, … ${names.length - count} more` : "");
    if (visibleWidth(text) <= budget) return text;
  }
  const first = names[0] ?? "", more = names.length > 1 ? `, … ${names.length - 1} more` : "";
  const cells = Math.max(4, budget - visibleWidth(more));
  return (visibleWidth(first) <= cells ? first : clip === "middle" ? clipMiddle(first, cells) : clipPlain(first, cells)) + more;
}
/** `<verb> ×n · <names>`: the distinct names of the run in order. */
function named(name: (target: string) => string, clip: Clip): Summarize {
  return (entries, room) => {
    const words: Words = { verb: newest(entries).verb, target: `×${entries.length}`, ...viewOf(entries) };
    const names = [...new Set(entries.map(entry => name(entry.model.target ?? "")).filter(Boolean))];
    const outcome = names.length ? listing(names, outcomeRoom(room, words, entries), clip) : newest(entries).outcome;
    return outcome ? { ...words, outcome } : words;
  };
}
/**
 * One target read in parts: `Read T-935 report.md · 4 pages`, `read src/a.ts · 3 reads`. `base` is a target without
 * its part; reads of several targets list their `name`s.
 */
function paged(unit: string, base: (target: string) => string, name: (target: string) => string, clip: Clip): Summarize {
  const several = named(name, clip);
  return (entries, room) => {
    const targets = [...new Set(entries.map(entry => base(entry.model.target ?? "")))];
    if (targets.length !== 1 || !targets[0]) return several(entries, room);
    const last = newest(entries), more = /more to read/.test(last.outcome ?? "") ? " · more to read" : "";
    return { verb: last.verb, target: targets[0], targetClip: last.targetClip ?? "middle", outcome: plural(entries.length, unit) + more, ...viewOf(entries) };
  };
}

/** What a plan update did to its step, in the past tense. */
const TODO_WORD: Readonly<Record<string, string>> = {
  init: "opened", append: "added to", start: "started", done: "done", drop: "dropped",
};
/** `Plan 5/7 · done: Add regression · started: Implement verification`: the newest position and the steps by update. */
const plan: Summarize = entries => {
  const position = [...entries].reverse().map(entry => /plan (\d+\/\d+)/.exec(entry.model.outcome ?? "")?.[1]).find(Boolean);
  const byWord = new Map<string, string[]>();
  for (const entry of entries) {
    const word = TODO_WORD[stringArg(entry.call.args, "op") ?? ""] ?? "updated", label = entry.model.target ?? "";
    const labels = byWord.get(word) ?? [];
    if (label && !labels.includes(label)) labels.push(label);
    byWord.set(word, labels);
  }
  const outcome = [...byWord].map(([word, labels]) => labels.length ? `${word}: ${labels.join(", ")}` : word).join(" · ");
  return { verb: "Plan", target: position ?? plural(entries.length, "update"), ...(outcome ? { outcome } : {}), ...viewOf(entries) };
};
/** `Attached 5 artifacts · report, 3 logs, screenshot`. */
const attached: Summarize = entries => {
  const kinds = new Map<string, number>();
  for (const entry of entries) {
    const kind = stringArg(entry.call.args, "kind") ?? "file";
    kinds.set(kind, (kinds.get(kind) ?? 0) + 1);
  }
  const outcome = [...kinds].map(([kind, count]) => count === 1 ? kind : `${count} ${kind.endsWith("s") ? kind : `${kind}s`}`).join(", ");
  return { verb: newest(entries).verb, target: plural(entries.length, "artifact"), outcome, ...viewOf(entries) };
};
/** `<verb> ×n · <newest outcome>`. */
const generic: Summarize = entries => {
  const last = newest(entries);
  return { verb: last.verb, target: `×${entries.length}`, ...(last.outcome ? { outcome: last.outcome } : {}), ...viewOf(entries) };
};

const pages = paged("page", unpaged, unpaged, "middle");
const files = named(fileName, "middle");
const searches = named(pattern, "end");
const listed = named(target => target, "end");
/** Kinds with a summary of their own, by tool name. They fold from two rows. */
const SUMMARIES: Readonly<Record<string, Summary>> = {
  corral_todo: { key: "tool", summarize: plan },
  corral_artifact: { summarize: attached },
  corral_content_read: { summarize: pages },
  corral_show: { summarize: pages },
  corral_inbox_read: { summarize: pages },
  web_read: { summarize: pages },
  read_review_snapshot: { summarize: pages },
  read: { summarize: paged("read", wholeFile, fileName, "middle") },
  edit: { summarize: paged("edit", target => target, fileName, "middle") },
  write: { summarize: files },
  grep: { summarize: searches },
  find: { summarize: searches },
  ls: { summarize: listed },
  corral_search: { summarize: searches },
  corral_list: { summarize: listed },
  web_search: { summarize: searches },
  tool_search: { summarize: searches },
};

/** How the rows of tool `name` fold: one kind per verb (or per tool), from two rows with a summary of its own, else three. */
export function groupingFor(name: string): RowGrouping {
  const own = Object.hasOwn(SUMMARIES, name) ? SUMMARIES[name] : undefined;
  const summarize = own?.summarize ?? generic, min = own ? SUMMARY_MIN : GENERIC_MIN;
  return {
    kind: (entry): GroupKind => ({ key: own?.key === "tool" ? name : `${name}\u0000${entry.model.verb}`, min }),
    summary: (entries, room) => ({ state: "ok", ...summarize(entries, room), ...durationOf(entries) }),
  };
}
