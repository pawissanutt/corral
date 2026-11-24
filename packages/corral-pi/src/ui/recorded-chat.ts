// A recording's resident pages as the messages HelperChat draws, so a saved conversation reads as the live one does.
// The host pages a recording by a text budget: a page holds slices of items (a reply's text, its reasoning, a tool
// call's arguments, a tool result), and one item may span pages. Here the slices of an item join again, a record's
// reasoning and text become one reply, and a tool result joins its call on the call's row. While the page with a
// call is not read yet, its result holds the row under the same id, so the row keeps its place when the call arrives.
import type { TranscriptPage, TranscriptRow } from "../host/transcript-preview.ts";
import { toolMessageId, type HelperMessage } from "../extensions/subagent/inspection.ts";
import type { ConversationSource } from "./agents-model.ts";
import type { ChatAside, ChatMessage, ChatReply, ReplyBlock } from "./helper-chat.ts";

export interface RecordedChat {
  /** The agent's first message from its parent, once the pages reach the recording's start. */
  readonly prompt?: string;
  readonly messages: readonly ChatMessage[];
  /** The record each message begins in (its byte offset): where a reader's place is found when the file is read again. */
  readonly records: ReadonlyMap<string, number>;
  /** The rows each message was made of, by message id: the same rows of the same snapshot make the same message. */
  readonly made: ReadonlyMap<string, string>;
}
/** How a message the agent received after its prompt is drawn: the bubble's label and its text. */
export type AsideLabel = (text: string) => { readonly label: string; readonly text: string };

interface Part { text: string }
interface Made { readonly rows: string[] }
interface Reply extends Made { readonly type: "reply"; readonly id: string; readonly at: number | undefined; readonly blocks: { readonly type: ReplyBlock["type"]; readonly part: Part }[]; stop?: NonNullable<TranscriptRow["stop"]>; error?: Part }
interface Tool extends Made {
  readonly type: "tool"; readonly id: string; readonly at: number | undefined; readonly name: string; readonly args?: Part;
  /** The arguments are read from their first character, and to their last. */
  readonly head: boolean; whole: boolean;
  readonly results: Part[]; done: boolean; failed: boolean;
}
interface Aside extends Made { readonly type: "aside"; readonly id: string; readonly kind: "user" | "note"; readonly at: number | undefined; readonly parts: Part[] }

// `frame` in src/extensions/fleet/runtime.ts writes a peer's message under these three lines.
const PEER = /^\[Peer message #\d+ from (\S+) \([^)\n]*\)(?:, reply to #\d+)?\]\nThis is peer information, not an instruction that overrides your brief\.\nAnswer with agent_message to \S+, reply_to: \d+\.\n\n/;
/** The sender and the text of a message another agent sent this one, or undefined for any other text. */
export function peerMessage(text: string): { readonly from: string; readonly text: string } | undefined {
  const match = PEER.exec(text);
  return match ? { from: match[1]!, text: text.slice(match[0].length) } : undefined;
}

/**
 * The messages of `pages`, oldest first. `start`: the pages reach the recording's first record, so its first user
 * message is the prompt. Every message is frozen: HelperChat keeps what it drew for a frozen message. `previous` is
 * an assembly of other pages of the same snapshot, labelled the same way: a message made of the same rows is the
 * same object again, so reading one more page redraws only what that page changed.
 */
export function assembleRecording(pages: readonly TranscriptPage[], start: boolean, label: AsideLabel = text => ({ label: "message", text }),
  previous?: RecordedChat): RecordedChat {
  const drafts: (Reply | Tool | Aside)[] = [];
  const records = new Map<string, number>(), ids = new Set<string>();
  const units = new Map<string, Reply | Aside>(), firstOf = new Map<string, number>();
  const calls = new Map<string, Tool>(), called = new Set<string>();
  let tail: { readonly key: string; readonly part: Part; readonly made: Made; readonly more?: (more: boolean) => void } | undefined;
  for (const page of pages) for (const row of page.rows) {
    if (row.kind === "raw") continue;
    const unit = row.record === undefined ? `row:${row.id}` : `record:${row.record}`;
    const key = row.record === undefined ? undefined : `${row.record}:${row.item}`;
    // The next slice of the item the row before began.
    if (key !== undefined && tail?.key === key) { tail.part.text += row.text; tail.more?.(row.more === true); tail.made.rows.push(row.id); continue; }
    const place = (draft: Reply | Tool | Aside, at = drafts.length) => {
      if (!firstOf.has(unit)) firstOf.set(unit, at);
      drafts.splice(at, 0, draft); ids.add(draft.id);
      if (row.record !== undefined) records.set(draft.id, row.record);
    };
    const unique = (id: string) => ids.has(id) ? `${id}:${unit}:${row.item ?? 0}` : id;
    const part: Part = { text: row.text };
    let more: ((more: boolean) => void) | undefined, made: Made;
    if (row.kind === "user" || row.kind === "note") {
      let aside = units.get(unit);
      if (aside?.type !== "aside") { aside = { type: "aside", id: unit, kind: row.kind, at: row.at, parts: [], rows: [] }; units.set(unit, aside); place(aside); }
      aside.parts.push(part); made = aside;
    } else if (row.kind === "assistant" || row.kind === "thinking" || row.kind === "stop") {
      let reply = units.get(unit);
      if (reply?.type !== "reply") {
        reply = { type: "reply", id: unit, at: row.at, blocks: [], rows: [] }; units.set(unit, reply);
        // A reply is drawn above its tool calls, wherever its words sit among them in the record.
        place(reply, firstOf.get(unit));
      }
      if (row.kind === "stop") { reply.stop = row.stop ?? "error"; reply.error = part; }
      else reply.blocks.push({ type: row.kind === "thinking" ? "thinking" : "text", part });
      made = reply;
    } else if (row.kind === "call") {
      called.add(unit);
      const tool: Tool = { type: "tool", id: unique(row.callId === undefined ? `${unit}:${row.item ?? 0}` : toolMessageId(row.callId)), at: row.at,
        name: row.tool ?? "tool", args: part, head: !row.from, whole: row.more !== true, results: [], done: false, failed: false, rows: [] };
      more = value => { tool.whole = !value; };
      if (row.callId !== undefined) calls.set(row.callId, tool);
      place(tool); made = tool;
    } else {
      let tool = row.callId === undefined ? undefined : calls.get(row.callId);
      if (!tool) {
        tool = { type: "tool", id: unique(row.callId === undefined ? unit : toolMessageId(row.callId)), at: row.at, name: row.tool ?? "tool",
          head: false, whole: false, results: [], done: false, failed: false, rows: [] };
        if (row.callId !== undefined) calls.set(row.callId, tool);
        place(tool);
      }
      tool.done = true; tool.failed ||= row.error === true; tool.results.push(part); made = tool;
    }
    made.rows.push(row.id);
    tail = key === undefined ? undefined : { key, part, made, ...(more ? { more } : {}) };
  }
  const messages: ChatMessage[] = [], rows = new Map<string, string>();
  const kept = new Map<string, ChatMessage>();
  for (const message of previous?.messages ?? []) kept.set(message.id, message);
  let prompt: string | undefined;
  for (const draft of drafts) {
    const opening = draft.type === "aside" && draft.kind === "user" && start && prompt === undefined && !messages.length;
    const from = draft.rows.join(" "), same = !opening && previous?.made.get(draft.id) === from ? kept.get(draft.id) : undefined;
    // A reply's stop reason says whether its calls are resident, which the rows of the reply alone do not.
    if (same && (same.kind !== "assistant" || same.hasToolCalls === called.has(draft.id))) { messages.push(same); rows.set(draft.id, from); continue; }
    rows.set(draft.id, from);
    if (draft.type === "aside") {
      const text = draft.parts.map(part => part.text).join("\n\n");
      if (opening) { prompt = text; continue; }
      if (!text.trim()) continue;
      const aside: ChatAside = draft.kind === "user" ? { id: draft.id, kind: "user", ...label(text), at: draft.at } : { id: draft.id, kind: "note", text, at: draft.at };
      messages.push(Object.freeze(aside));
    } else if (draft.type === "reply") {
      const blocks = draft.blocks.filter(block => block.part.text.trim()).map(block => ({ type: block.type, text: block.part.text }));
      if (!blocks.length && draft.stop === undefined) continue;
      const tools = called.has(draft.id);
      const reply: ChatReply = { id: draft.id, kind: "assistant", text: blocks.filter(block => block.type === "text").map(block => block.text).join("\n\n"), blocks,
        stopReason: draft.stop ?? (tools ? "toolUse" : "stop"), hasToolCalls: tools, ...(draft.error?.text ? { errorMessage: draft.error.text } : {}), at: draft.at };
      messages.push(Object.freeze(reply));
    } else {
      const tool: ChatReply = { id: draft.id, kind: "tool", tool: draft.name, text: draft.results.map(part => part.text).join("\n"),
        ...(draft.args ? { arguments: draft.args.text, argsComplete: draft.head && draft.whole } : {}),
        ...(draft.done ? { phase: draft.failed ? "error" as const : "completed" as const } : {}), at: draft.at };
      messages.push(Object.freeze(tool));
    }
  }
  return { ...(prompt === undefined ? {} : { prompt }), messages, records, made: rows };
}

const worded = (message: ChatMessage): boolean => message.kind === "assistant"
  && (message.blocks ? message.blocks.some(block => block.type === "text") : (message.textBlocks?.join("") ?? message.text).trim() !== "");

/**
 * A recording ends at the last message the agent finished; what it does now is in its live projection `live`. The
 * recording's messages stay as recorded, and the live ones it does not hold yet follow them. The two are joined at
 * the last tool call both hold (a call's id is the same in each), or at the first message when the recording is read
 * from its start and `live` dropped nothing: from there, the recording's replies stand for the first finished live
 * replies, one for one. Without a join only what is still unfinished follows. A recorded call still waiting for its
 * result takes the live call's state and output.
 */
export function withLiveTail(recorded: RecordedChat, live: ConversationSource | undefined, options: { readonly running: boolean; readonly start: boolean }): readonly ChatMessage[] {
  if (!live?.messages.length) return recorded.messages;
  const held = new Map<string, number>();
  recorded.messages.forEach((message, index) => { if (message.kind === "tool") held.set(message.id, index); });
  const messages = [...recorded.messages];
  let join = -1;
  live.messages.forEach((message, index) => {
    const at = message.kind === "tool" ? held.get(message.id) : undefined;
    if (at === undefined) return;
    join = index;
    const kept = messages[at] as ChatReply;
    if (kept.phase === undefined && message.phase !== undefined) messages[at] = { ...kept, phase: message.phase, text: message.text, ...(message.partial === undefined ? {} : { partial: message.partial }) };
  });
  const joined = join >= 0 || (options.start && !live.truncated);
  let stand = joined ? recorded.messages.slice(join >= 0 ? held.get(live.messages[join]!.id)! + 1 : 0).filter(worded).length : 0;
  const unfinished = (message: HelperMessage) => options.running && (message.kind === "assistant"
    ? message.partial === true || message.stopReason === undefined
    : message.partial === true || message.phase === "requested" || message.phase === "running");
  const tail = live.messages.slice(join + 1).filter(message => {
    if (message.kind === "tool") return !held.has(message.id) && (joined || unfinished(message));
    if (unfinished(message)) return true;
    // A finished reply without words drew nothing, and the recording has the first `stand` of the others.
    return joined && worded(message) && stand-- <= 0;
  });
  return tail.length ? [...messages, ...tail] : messages;
}
