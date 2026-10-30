// The subset of `bun:ffi` the credential lock uses. The package keeps the GO lock byte-identical,
// so it carries no @types/bun; this declaration gives tsc the API's shape.
declare module "bun:ffi" {
  export const FFIType: { readonly i32: number; readonly ptr: number };
  interface Symbol {
    readonly args: readonly number[];
    readonly returns: number;
  }
  export function dlopen<T extends Record<string, Symbol>>(
    path: string,
    symbols: T,
  ): { readonly symbols: { readonly [K in keyof T]: (...args: number[]) => unknown }; close(): void };
  export const read: { i32(pointer: number, offset?: number): number };
}
