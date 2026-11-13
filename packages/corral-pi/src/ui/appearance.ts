// Session-local host preferences. No filesystem/resource lookup, and no authority-bearing state.
export type Appearance = "dark" | "light" | "contrast";
export type Density = "comfortable" | "compact";
export const PACKAGED = {
  dark: { background: "#191b20", text: "#e0e3e9", muted: "#9ba2b0", accent: "#83a8ef", rule: "#767e90", success: "#9ac7a3", warning: "#e3bd7c", error: "#e699a3", selected: "#2c3548" },
  light: { background: "#ffffff", text: "#17212f", muted: "#485568", accent: "#234f96", rule: "#627188", success: "#216239", warning: "#754900", error: "#9b2338", selected: "#e3eaf5" },
  contrast: { background: "#000000", text: "#ffffff", muted: "#dedede", accent: "#9cceff", rule: "#ffffff", success: "#a4ffaf", warning: "#ffe18a", error: "#ffb9c4", selected: "#262626" },
} as const;
/**
 * Eight name colours per appearance for telling agents apart. Each keeps at least 4.5:1 contrast on the
 * panel `background` and on the `selected` row, so a hued name stays readable on a selected row.
 */
export const AGENT_HUES: Readonly<Record<Appearance, readonly string[]>> = {
  dark: ["#8fa3c7", "#b9a0f5", "#7fd4e8", "#f08fc0", "#f2a97e", "#a9dcb4", "#e3bd7c", "#83a8ef"],
  light: ["#3d5280", "#6b3fc4", "#0b6577", "#a8286a", "#a3460f", "#256b3a", "#7a5300", "#1f55b8"],
  contrast: ["#a9bde6", "#cdb6ff", "#8fe6f7", "#ff9fd0", "#ffb98e", "#b5f0c0", "#ffd88a", "#9cc4ff"],
};
/** `amount` of `over` mixed into `base`: derived tints stay on the packaged palette in every appearance. */
export function tint(base: string, over: string, amount: number): string {
  return "#" + [1, 3, 5].map(i => {
    const a = Number.parseInt(base.slice(i, i + 2), 16), b = Number.parseInt(over.slice(i, i + 2), 16);
    return Math.round(a + (b - a) * amount).toString(16).padStart(2, "0");
  }).join("");
}
export function ansi(hex: string, background = false): string {
  return `\x1b[${background ? 48 : 38};2;${[1, 3, 5].map(i => Number.parseInt(hex.slice(i, i + 2), 16)).join(";")}m`;
}
