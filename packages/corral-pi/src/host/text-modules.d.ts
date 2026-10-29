// Bun imports used by compiled.ts to embed role text and QuickJS's WASM binary.
declare module "*.wasm" {
  const path: string;
  export default path;
}
declare module "*.md" {
  const text: string;
  export default text;
}
