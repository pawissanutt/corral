// The host pins Bun; declare only the TOML reader used for Corral's config output.
declare module "bun" {
  export const TOML: { parse(text: string): unknown };
}
