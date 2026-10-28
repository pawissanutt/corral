// Bun's `with { type: "text" }` imports, which compiled.ts uses to embed the role assets.
declare module "*.md" {
  const text: string;
  export default text;
}
