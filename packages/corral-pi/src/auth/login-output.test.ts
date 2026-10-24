// The real `src/auth/login.ts` entry point never prints a token (review F2): the only substitution
// is a preloaded offline fetch whose OAuth responses carry synthetic secrets. Device-code login is
// selected through stdin; browser login shares the same token exchange. `corral agent install`
// never copies this file.
import { afterEach, describe, expect, test } from "bun:test";
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { loginDiagnostic, redactNotice } from "./login.ts";

const HERE = dirname(fileURLToPath(import.meta.url));
const SECRETS = ["SYNTHETIC-ACCESS-SECRET", "SYNTHETIC-REFRESH-SECRET", "SYNTHETIC-BODY-SECRET"];
const temps: string[] = [];

afterEach(() => {
  for (const dir of temps.splice(0)) rmSync(dir, { recursive: true, force: true });
});

type Scenario = "exchange_missing_fields" | "exchange_rejected" | "usercode_rejected";

async function runLogin(scenario: Scenario): Promise<{ code: number | null; output: string; dir: string }> {
  const root = mkdtempSync(join(tmpdir(), "corral-pi-login-"));
  temps.push(root);
  const preload = join(root, "fake-oauth.ts");
  writeFileSync(preload, `
    const scenario = ${JSON.stringify(scenario)};
    const json = (body, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
    globalThis.fetch = async (input) => {
      const url = String(input instanceof Request ? input.url : input);
      if (url.endsWith("/deviceauth/usercode")) {
        return scenario === "usercode_rejected"
          ? json({ error: "SYNTHETIC-BODY-SECRET", access_token: "SYNTHETIC-ACCESS-SECRET" }, 500)
          : json({ device_auth_id: "dev", user_code: "CODE-1", interval: 0 });
      }
      if (url.endsWith("/deviceauth/token")) return json({ authorization_code: "code", code_verifier: "verifier" });
      if (scenario === "exchange_missing_fields") {
        return json({ access_token: "SYNTHETIC-ACCESS-SECRET", refresh_token: "SYNTHETIC-REFRESH-SECRET" });
      }
      return json({ error: "invalid_grant", detail: "SYNTHETIC-BODY-SECRET SYNTHETIC-REFRESH-SECRET" }, 400);
    };
  `);
  const dir = join(root, "auth");
  const child = spawn(process.execPath, ["--no-env-file", "--preload", preload, join(HERE, "login.ts"), "--auth-dir", dir], {
    cwd: join(HERE, "../.."),
    env: { PATH: process.env.PATH ?? "", HOME: root, XDG_DATA_HOME: join(root, "data") },
    stdio: ["pipe", "pipe", "pipe"],
  });
  let output = "";
  let answered = false;
  const record = (chunk: Buffer) => {
    output += chunk.toString();
    if (!answered && output.includes("Select OpenAI Codex login method:")) {
      answered = true;
      child.stdin.write("2\n");
    }
  };
  child.stdout.on("data", record);
  child.stderr.on("data", record);
  const code = await new Promise<number | null>((resolve) => child.on("close", resolve));
  child.stdin.destroy();
  return { code, output, dir };
}

describe("corral agent login output", () => {
  test("a token response missing fields fails without printing either token", async () => {
    const { code, output, dir } = await runLogin("exchange_missing_fields");
    expect(code).toBe(1);
    expect(output).toContain("corral agent login failed: OpenAI login failed at the token exchange; response content is not shown");
    for (const secret of SECRETS) expect(output).not.toContain(secret);
    expect(existsSync(join(dir, "auth.json"))).toBeFalse();
  }, 60_000);

  test("a rejected token exchange reports its HTTP status but never the response body", async () => {
    const { code, output } = await runLogin("exchange_rejected");
    expect(code).toBe(1);
    expect(output).toContain("OpenAI login failed at the token exchange (HTTP 400)");
    for (const secret of SECRETS) expect(output).not.toContain(secret);
  }, 60_000);

  test("a rejected device-code request reports its status but never the response body", async () => {
    const { code, output } = await runLogin("usercode_rejected");
    expect(code).toBe(1);
    expect(output).toContain("OpenAI login failed at the device-code request (HTTP 500)");
    for (const secret of SECRETS) expect(output).not.toContain(secret);
  }, 60_000);

  test("diagnostics keep corral's own refusals and fixed phrases, and notices are redacted", () => {
    expect(loginDiagnostic(new Error("Login cancelled"))).toBe("Login cancelled");
    expect(loginDiagnostic(new Error("something {\"access_token\":\"x\"}"))).toBe(
      "OpenAI login failed at the OAuth flow; response content is not shown");
    expect(loginDiagnostic("not an error")).toBe("OpenAI login failed at the OAuth flow; response content is not shown");
    expect(redactNotice("Exchanged eyJhbGciOiJub25lIn0.eyJzdWIiOiJ4In0.sig for a token")).toBe("Exchanged <redacted> for a token");
    expect(redactNotice("x".repeat(500))).toBe("<redacted>");
    expect(redactNotice("ab ".repeat(200))).toHaveLength(240);
  });
});
