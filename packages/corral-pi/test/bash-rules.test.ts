// Bash allow/ask/deny ordering and nested shells.
import { expect, test } from "bun:test";

import {
  type BashRuleSettings, BashRuleSettingsSchema, decideBash,
} from "../src/capabilities/bash-rules.ts";

const rules = (overrides: Partial<BashRuleSettings>): BashRuleSettings =>
  ({ allow: [], ask: [], deny: [], default: "allow", ...overrides });

test("deny wins over ask and allow, ask over allow, and unmatched commands take the default", () => {
  // Given overlapping rules for one command.
  const overlapping = rules({ allow: ["git *"], ask: ["git push*"], deny: ["git push --force*"] });
  // Then the strictest matching rule decides.
  expect(decideBash("git push --force origin", overlapping))
    .toEqual({ action: "deny", rule: "git push --force*", segment: "git push --force origin" });
  expect(decideBash("git push origin", overlapping)).toEqual({ action: "ask", rule: "git push*", segment: "git push origin" });
  expect(decideBash("git status", overlapping).action).toBe("allow");
  expect(decideBash("ls", rules({ default: "deny" })).action).toBe("deny");
  expect(decideBash("ls", rules({ default: "ask" })).action).toBe("ask");
});

test("compound lines, substitutions and nested shells are decided piece by piece", () => {
  // Given a deny rule and an allow-only default.
  const deny = rules({ deny: ["rm -rf *"] });
  const allowOnly = rules({ allow: ["cargo *", "echo *"], default: "deny" });
  // Then a denied command hidden behind an operator, substitution or sh -c is still denied.
  for (const command of [
    "echo hi && rm -rf target", "echo hi; rm  -rf   target", "echo $(rm -rf target)", "echo `rm -rf target`",
    "bash -c 'rm -rf target'", "sh -lc \"echo ok | rm -rf target\"", "(rm -rf target)",
  ]) {
    const decision = decideBash(command, deny);
    expect(decision.action).toBe("deny");
    expect(decision.segment).toContain("rm -rf target");
  }
  // And combined shell options carrying -c expose their command string to deny and ask alike.
  const denyRm = rules({ deny: ["rm *"] });
  const ask = rules({ ask: ["rm *"] });
  for (const shell of ["sh", "bash", "dash", "zsh", "/bin/bash", "env bash"]) {
    for (const options of ["-lc", "-ec", "-c", "-e -c", "-o pipefail -c", "--login -c", "-cl", "-c -e"]) {
      for (const quote of ["'", "\""]) {
        const command = `${shell} ${options} ${quote}rm notes.txt${quote}`;
        expect([command, decideBash(command, denyRm).action]).toEqual([command, "deny"]);
        expect([command, decideBash(command, ask).action]).toEqual([command, "ask"]);
      }
    }
  }
  // And a shell running a script file, without -c, is decided by its own line only.
  expect(decideBash("bash -l scripts/rm-notes.sh", ask).action).toBe("allow");
  expect(decideBash("bash -o pipefail scripts/check.sh", ask).action).toBe("allow");
  // And an allow-only default needs every piece allowed.
  expect(decideBash("cargo test && echo done", allowOnly).action).toBe("allow");
  expect(decideBash("cargo test && curl example.org", allowOnly)).toEqual({ action: "deny", segment: "curl example.org" });
  // And glob metacharacters other than * and ? are literal.
  expect(decideBash("rm a.b", rules({ deny: ["rm a?b"] })).action).toBe("deny");
  expect(decideBash("rm axb", rules({ deny: ["rm a.b"] })).action).toBe("allow");
});

test("rule settings parse only the resolved Rust shape", () => {
  expect(BashRuleSettingsSchema.safeParse(rules({ deny: ["x"] })).success).toBeTrue();
  for (const invalid of [{ ...rules({}), default: "maybe" }, rules({ deny: [" "] }), { ...rules({}), extra: 1 }]) {
    expect(BashRuleSettingsSchema.safeParse(invalid).success).toBeFalse();
  }
});

