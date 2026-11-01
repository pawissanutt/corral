// Bun imports used by compiled.ts to embed role text.
declare module "*.md" {
  const text: string;
  export default text;
}
