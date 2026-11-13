import type { MarkdownTransformer, Theme } from "@earendil-works/pi-coding-agent";
import { DRACULA_BG, DRACULA_FG, type ThemeClass } from "./dracula.ts";
import { PACKAGED, type Appearance } from "./appearance.ts";
import { PANEL_MUTED, PANEL_TEXT } from "./panels-view.ts";

/** Markdown that opens with block syntax keeps it: the label then stands on its own line. */
const BLOCK_START = /^\s*(#|>|[-*+]\s|\d+[.)]\s|```|~~~|\|)/;
/**
 * The owner's messages in the session transcript open with `you` in muted, as the mock's user bubble does.
 * Registered for user Markdown only; a view that labels a user-style message itself filters this one out.
 */
export const userLabel: MarkdownTransformer = (markdown, context) => context.messageType !== "user" ? markdown
  : `${PANEL_MUTED}you${PANEL_TEXT}${BLOCK_START.test(markdown) ? "\n\n" : " "}${markdown}`;

/**
 * Packaged host theme only: ambient theme discovery remains disabled. Every surface Pi paints is the one
 * canvas background (assistant text, tool cards, custom messages) except the user message bubble and the
 * selection and search highlights.
 */
export function panelTheme(ThemeCtor: ThemeClass, mode: ReturnType<Theme["getColorMode"]>, appearance: Appearance = "dark"): Theme {
  const p = PACKAGED[appearance];
  // Set all syntax/markdown tokens too: dark-theme inherited colors are unreadable on light panels.
  const fg = Object.fromEntries(Object.keys(DRACULA_FG).map(key => [key, p.text])) as typeof DRACULA_FG;
  const bg = Object.fromEntries(Object.keys(DRACULA_BG).map(key => [key, p.background])) as typeof DRACULA_BG;
  return new ThemeCtor({ ...fg, accent: p.accent, border: p.line, borderAccent: p.accent, borderMuted: p.line,
    muted: p.muted, dim: p.muted, success: p.success, warning: p.warning, error: p.error,
    mdLink: p.accent, mdCode: p.code, mdHeading: p.head, toolTitle: p.accent, toolDiffAdded: p.success, toolDiffRemoved: p.error,
    searchMatchText: p.text, scrollbarTrack: p.line, scrollbarThumb: p.rule,
  }, { ...bg, userMessageBg: p.bubble, selectedBg: p.selected, searchMatchBg: p.selected }, mode,
  { name: `corral-${appearance}`, appearance: appearance === "light" ? "light" : "dark" });
}
