// The rich UI's palette: Dracula (https://draculatheme.com) mapped onto every Pi 1.3.0 theme
// token. The Theme class comes from the SDK the host already loaded, so this module never imports
// the SDK itself before the host restricts its environment.
import type { Theme, ThemeBg, ThemeColor } from "@earendil-works/pi-coding-agent";

export const DRACULA = {
  background: "#282a36", currentLine: "#44475a", foreground: "#f8f8f2", comment: "#6272a4",
  cyan: "#8be9fd", green: "#50fa7b", orange: "#ffb86c", pink: "#ff79c6", purple: "#bd93f9",
  red: "#ff5555", yellow: "#f1fa8c",
} as const;

const d = DRACULA;
const MUTED = "#bcc2dc";

export const DRACULA_FG: Record<ThemeColor, string> = {
  accent: d.purple, border: d.comment, borderAccent: d.purple, borderMuted: d.currentLine,
  success: d.green, error: d.red, warning: d.orange, muted: MUTED, dim: d.comment, text: d.foreground,
  thinkingText: "#9ea8c7", scrollbarTrack: d.currentLine, scrollbarThumb: d.comment,
  searchMatchText: d.yellow, userMessageText: d.foreground, customMessageText: MUTED,
  customMessageLabel: d.pink, toolTitle: d.foreground, toolOutput: MUTED,
  mdHeading: d.purple, mdLink: d.cyan, mdLinkUrl: d.comment, mdCode: d.green, mdCodeBlock: d.foreground,
  mdCodeBlockBorder: d.comment, mdQuote: d.yellow, mdQuoteBorder: d.comment, mdHr: d.comment,
  mdListBullet: d.pink, toolDiffAdded: d.green, toolDiffRemoved: d.red, toolDiffContext: d.comment,
  syntaxComment: d.comment, syntaxKeyword: d.pink, syntaxFunction: d.green, syntaxVariable: d.foreground,
  syntaxString: d.yellow, syntaxNumber: d.purple, syntaxType: d.cyan, syntaxOperator: d.pink,
  syntaxPunctuation: d.foreground, thinkingOff: d.comment, thinkingMinimal: "#8b95c9", thinkingLow: d.cyan,
  thinkingMedium: d.purple, thinkingHigh: d.pink, thinkingXhigh: d.orange, thinkingMax: d.red,
  bashMode: d.green,
};

export const DRACULA_BG: Record<ThemeBg, string> = {
  selectedBg: d.currentLine, searchMatchBg: "#4f4b2c", userMessageBg: "#363948",
  customMessageBg: "#3a3350", toolPendingBg: "#343746", toolSuccessBg: "#263a30", toolErrorBg: "#40292f",
};

export type ThemeClass = new (...args: ConstructorParameters<typeof Theme>) => Theme;

export function draculaTheme(ThemeCtor: ThemeClass, mode: ReturnType<Theme["getColorMode"]>): Theme {
  return new ThemeCtor(DRACULA_FG, DRACULA_BG, mode, { name: "dracula", appearance: "dark" });
}
