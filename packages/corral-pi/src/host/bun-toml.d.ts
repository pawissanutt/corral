// Bun parsing APIs used without depending on the full Bun type package.
declare module "bun" {
  export const TOML: { parse(text: string): unknown };
  export const JSONC: { parse(text: string): unknown };
}
