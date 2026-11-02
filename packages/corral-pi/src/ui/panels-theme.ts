import type { Theme } from "@earendil-works/pi-coding-agent";
import { DRACULA_BG, DRACULA_FG, type ThemeClass } from "./dracula.ts";
import { PACKAGED, type Appearance } from "./appearance.ts";

/** Packaged host theme only: ambient theme discovery remains disabled. */
export function panelTheme(ThemeCtor: ThemeClass, mode: ReturnType<Theme["getColorMode"]>, appearance: Appearance = "dark"): Theme {
  const p = PACKAGED[appearance];
  // Set all syntax/markdown tokens too: dark-theme inherited colors are unreadable on light panels.
  const fg = Object.fromEntries(Object.keys(DRACULA_FG).map(key => [key, p.text])) as typeof DRACULA_FG;
  const bg = Object.fromEntries(Object.keys(DRACULA_BG).map(key => [key, p.background])) as typeof DRACULA_BG;
  return new ThemeCtor({ ...fg, accent: p.accent, border: p.rule, borderAccent: p.accent, borderMuted: p.rule,
    muted: p.muted, dim: p.muted, success: p.success, warning: p.warning, error: p.error,
    mdLink: p.accent, toolDiffAdded: p.success, toolDiffRemoved: p.error, searchMatchText: p.text,
  }, { ...bg, selectedBg: p.selected, searchMatchBg: p.selected }, mode,
  { name: `corral-${appearance}`, appearance: appearance === "light" ? "light" : "dark" });
}
