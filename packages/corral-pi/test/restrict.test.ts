import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import {
  BUILTIN_EXTENSIONS, NetworkRefused, PROVIDER_API, TOKEN_REFRESH, guardedFetch, resourceLoaderOptions,
  restrictEnvironment, restrictedSettings,
} from "../src/host/restrict.ts";
import { PACKAGE_ROOT } from "./support/fixture-install.ts";

test("guarded fetch sends only POSTs to the pinned API and refresh endpoints, without redirects", async () => {
  const sent: { url: string; redirect?: RequestInit["redirect"] }[] = [];
  const guard = guardedFetch(async (input, init) => {
    sent.push({ url: String(input), ...(init?.redirect ? { redirect: init.redirect } : {}) });
    return new Response("{}");
  });
  await guard.fetch(PROVIDER_API, { method: "POST", body: "{}" });
  await guard.fetch(new URL(TOKEN_REFRESH), { method: "POST", body: "{}" });
  expect(sent).toEqual([{ url: PROVIDER_API, redirect: "error" }, { url: TOKEN_REFRESH, redirect: "error" }]);
  const refused = [
    () => guard.fetch(PROVIDER_API),
    () => guard.fetch("https://pi.dev/api/report-install?version=1.3.0"),
    () => guard.fetch("https://chatgpt.com/backend-api/codex/models", { method: "POST" }),
    () => guard.fetch(new Request("https://radius.example/v1/bug-reports", { method: "POST" })),
  ];
  for (const attempt of refused) {
    let error: unknown;
    try {
      await attempt();
    } catch (caught) {
      error = caught;
    }
    expect(error instanceof NetworkRefused).toBeTrue();
  }
  expect(guard.ledger()).toEqual({
    allowed: 2, refused: 4,
    refusedDestinations: [
      "https://chatgpt.com/backend-api/codex/responses", "https://pi.dev/api/report-install",
      "https://chatgpt.com/backend-api/codex/models", "https://radius.example/v1/bug-reports",
    ],
  });
  expect(sent).toHaveLength(2);
});

test("every upstream built-in extension is disabled and resource discovery is off", () => {
  const index = readFileSync(join(PACKAGE_ROOT,
    "node_modules/@earendil-works/pi-coding-agent/dist/extensions/index.js"), "utf8");
  const upstream = [...index.matchAll(/name: "([^"]+)"/g)].map((match) => match[1]);
  expect([...BUILTIN_EXTENSIONS].sort()).toEqual(upstream.sort());
  const options = resourceLoaderOptions([]);
  expect(options).toEqual({
    noExtensions: true, noSkills: true, noPromptTemplates: true, noThemes: true, noContextFiles: true,
    disabledBuiltinExtensions: [...BUILTIN_EXTENSIONS], extensionFactories: [],
  });
  expect(restrictedSettings()).toEqual({
    cacheWarming: "off", enableInstallTelemetry: false, enableAnalytics: false, enableSkillCommands: false,
    quietStartup: true, transport: "sse", extensions: [],
  });
});

test("the environment turns off upstream startup network and telemetry", () => {
  const env: NodeJS.ProcessEnv = { PI_EXPERIMENTAL: "1", PI_CODING_AGENT_DIR: "/home/owner/.pi/agent", PI_TELEMETRY: "1" };
  restrictEnvironment(env);
  expect(env).toEqual({ PI_OFFLINE: "1", PI_SKIP_VERSION_CHECK: "1", PI_TELEMETRY: "0" });
});
