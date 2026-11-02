import type { Theme } from "@earendil-works/pi-coding-agent";
import { DRACULA_BG, DRACULA_FG, type ThemeClass } from "./dracula.ts";

/** Packaged host theme only: ambient theme discovery remains disabled. */
export function panelTheme(ThemeCtor: ThemeClass, mode: ReturnType<Theme["getColorMode"]>): Theme {
  return new ThemeCtor({
    ...DRACULA_FG, accent: "#83a8ef", border: "#414752", borderAccent: "#83a8ef", borderMuted: "#414752",
    text: "#e0e3e9", muted: "#9ba2b0", dim: "#9ba2b0", success: "#9ac7a3", warning: "#e3bd7c", error: "#e699a3",
    userMessageText: "#e0e3e9", customMessageText: "#e0e3e9", customMessageLabel: "#83a8ef",
    mdHeading: "#e0e3e9", mdLink: "#83a8ef", toolTitle: "#83a8ef", toolOutput: "#9ba2b0",
    toolDiffAdded: "#9ac7a3", toolDiffRemoved: "#e699a3",
  }, { ...DRACULA_BG, selectedBg: "#2c3548", userMessageBg: "#191b20", customMessageBg: "#191b20",
    toolPendingBg: "#191b20", toolSuccessBg: "#191b20", toolErrorBg: "#35232a" }, mode,
  { name: "corral-panels", appearance: "dark" });
}
