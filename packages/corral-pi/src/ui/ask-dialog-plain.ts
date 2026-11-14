// The stock text layout of the owner's question dialog, for hosts without native panels: bracketed tabs, a `>`
// cursor, `(•)`/`[x]` boxes and one line of keys. It reads the dialog's own view and draws it; the native card
// is ask-dialog-view.ts.
import { truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import type { AskDialogView } from "./ask-dialog-view.ts";

/** `escape` is what Esc does, worded by the caller (`options.escape`, else "Esc send answered only"). */
export function askDialogPlainLines(view: AskDialogView, escape: string, width: number): string[] {
  const w = Math.max(1, width);
  const fit = (line: string) => truncateToWidth(line, w);
  const wrap = (line: string) => wrapTextWithAnsi(line, Math.max(10, w)).map(fit);
  const tabs = view.tabs.map((tab) => (tab.submit ? "Submit" : `${tab.answered ? "✓" : "○"} ${tab.label}`))
    .map((text, i) => (view.tabs[i]!.active ? `[${text}]` : ` ${text} `)).join(" ");
  const out = [fit(tabs), ""];
  const review = view.review;
  if (review) {
    // Answers wrap in full here: this is where the owner inspects them before sending.
    for (const row of review.rows) out.push(...wrap(`${row.label}: ${row.answer ?? "unanswered"}`));
    out.push("", ...wrap(`Enter submit · ←/→ revise · ${escape}`));
    return out;
  }
  const q = view.question!;
  out.push(...wrapTextWithAnsi(q.prompt, Math.max(10, w)).map(fit), "");
  q.options.forEach((o) => {
    const box = q.multiSelect ? (o.on ? "[x]" : "[ ]") : o.on ? "(•)" : "( )";
    out.push(fit(`${o.cursor ? ">" : " "} ${box} ${o.label}`));
    if (o.description) out.push(...wrapTextWithAnsi(o.description, Math.max(10, w - 8)).map((l) => fit(`        ${l}`)));
  });
  const own = q.own;
  const onText = own?.cursor ?? q.options.every((o) => !o.cursor);
  const noting = own?.kind === "note";
  if (own) {
    const box = noting ? "   " : own.on ? "(•)" : "( )";
    const prefix = `${onText ? ">" : " "} ${box} ${noting ? "Note" : "Other"}: `;
    const room = Math.max(1, w - visibleWidth(prefix));
    // The input scrolls horizontally, keeping the caret and the text around it visible.
    const field = onText ? own.renderInput(room)
      : own.draft ? truncateToWidth(own.draft, room) : noting ? "add a note" : "type your own answer";
    out.push(fit(prefix + field));
  }
  const chosen = !q.multiSelect && q.options.some((o) => o.cursor && o.on);
  const keys = onText ? `Enter ${noting ? "save note" : "use answer"} · ←/→ edit · Tab questions`
    : q.multiSelect ? "Space toggle · Enter confirm · ←/→ questions"
      : `Enter choose${chosen ? " · Space clear" : ""} · ←/→ questions`;
  out.push("", ...wrap(`↑/↓ move · ${keys} · ${escape}`));
  return out;
}
