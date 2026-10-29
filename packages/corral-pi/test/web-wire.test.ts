import { expect, test } from "bun:test";
import { createServer } from "node:http";

import { createWebAccess } from "../src/extensions/web/client.ts";
import { guardedFetch } from "../src/host/restrict.ts";

const settings = { hosts: ["*"], max_bytes: 1024, timeout_ms: 5000, max_redirects: 3 };

test("configured search sends real GET requests without retaining server cookies", async () => {
  // Given a real local HTTP SearXNG fixture that tries to set a cookie.
  const requests: { readonly method?: string; readonly cookie?: string; readonly authorization?: string; readonly url?: string }[] = [];
  const server = createServer((request, response) => {
    requests.push({
      method: request.method, cookie: request.headers.cookie, authorization: request.headers.authorization, url: request.url,
    });
    response.writeHead(200, { "content-type": "application/json", "set-cookie": "owner=private; Path=/" });
    response.end(JSON.stringify({ results: [{ title: "Docs", url: "https://example.org/", content: "Found" }] }));
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    const address = server.address();
    if (!address || typeof address === "string") throw new Error("fixture needs an ephemeral TCP port");
    const web = createWebAccess({ ...settings, searxng_url: `http://127.0.0.1:${address.port}/` }, globalThis.fetch);
    // When two searches share the same transport.
    for (const query of ["first", "second"]) expect((await web.search(query, 5)).results).toHaveLength(1);
    // Then there is no cookie jar or inherited authorization on either real wire request.
    expect(requests).toEqual(["first", "second"].map((query) => ({
      method: "GET", cookie: undefined, authorization: undefined, url: `/search?q=${query}&format=json`,
    })));
  } finally {
    await new Promise<void>((resolve, reject) => {
      server.close((error) => error ? reject(error) : resolve());
      server.closeAllConnections();
    });
  }
});

test("search exceptions and public wildcard never widen provider fetch", async () => {
  // Given separate provider and web transports on one guard.
  let providerCalls = 0;
  let webCalls = 0;
  const network = guardedFetch(async () => { providerCalls++; return new Response("provider"); },
    undefined, "openai-codex", undefined, {
      settings: { ...settings, searxng_url: "http://127.0.0.1:8888/" },
      send: async () => { webCalls++; return Response.json({ results: [] }); },
    });
  // When only the purpose-specific search tool reaches its private configured endpoint.
  await expect(network.fetch("http://127.0.0.1:8888/search")).rejects.toThrow(/network_refused/);
  await expect(network.fetch("https://docs.example.org/")).rejects.toThrow(/network_refused/);
  expect(await network.web?.search("query", 5)).toEqual({ configured: true, results: [], truncated: false });
  await expect(network.web?.fetch("http://127.0.0.1:8888/")).rejects.toThrow(/web_target_refused/);
  await network.fetch("https://chatgpt.com/backend-api/codex/responses", { method: "POST" });
  // Then provider calls keep their old policy and all network activity shares the ledger.
  expect(providerCalls).toBe(1);
  expect(webCalls).toBe(1);
  expect(network.ledger().allowed).toBe(2);
  expect(network.ledger().refused).toBe(3);
});
