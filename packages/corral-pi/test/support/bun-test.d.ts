// The subset of `bun:test` these tests use. The package keeps the GO lock byte-identical, so it
// carries no @types/bun; this declaration gives tsc the test API's shape.
declare module "bun:test" {
  interface Matchers {
    readonly not: Matchers;
    toBe(expected: unknown): void;
    toEqual(expected: unknown): void;
    toMatch(expected: RegExp | string): void;
    toContain(expected: unknown): void;
    toBeDefined(): void;
    toBeUndefined(): void;
    toBeNull(): void;
    toBeTrue(): void;
    toBeFalse(): void;
    toBeGreaterThan(expected: number): void;
    toBeGreaterThanOrEqual(expected: number): void;
    toBeLessThanOrEqual(expected: number): void;
    toHaveLength(expected: number): void;
    toThrow(expected?: RegExp | string | (new (...args: never[]) => Error)): void;
    readonly rejects: Matchers;
  }
  export function expect(actual: unknown): Matchers;
  export function test(name: string, run: () => unknown, timeout?: number): void;
  export function describe(name: string, run: () => void): void;
  export function beforeAll(run: () => unknown, timeout?: number): void;
  export function afterAll(run: () => unknown, timeout?: number): void;
  export function afterEach(run: () => unknown): void;
}
