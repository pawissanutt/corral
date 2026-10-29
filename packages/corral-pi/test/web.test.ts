import { expect, test } from "bun:test";

import { createWebAccess } from "../src/extensions/web/client.ts";
import type { WebSettings } from "../src/extensions/web/settings.ts";
import { WebError } from "../src/extensions/web/policy.ts";

const settings: WebSettings = { hosts: ["*"], max_bytes: 1024, timeout_ms: 5000, max_redirects: 3 };
const lookup = async () => [{ address: "93.184.216.34", family: 4 }];

test("public GET has bounded output and no inherited credentials or cookies", async () => {
  // Given an overlong public response and a tiny byte budget.
  let request: Request | undefined;
  let credentials: RequestInit["credentials"];
  const web = createWebAccess({ ...settings, max_bytes: 4 }, async (url, init) => {
    request = new Request(url, init);
    credentials = init?.credentials;
    return new Response("abcdef", { headers: { "set-cookie": "session=private", "content-type": "text/plain" } });
  }, { lookup });
  // When the public page is fetched.
  const result = await web.fetch("https://unlisted.example.org/doc");
  // Then output is capped and no credentials can cross into the request.
  expect(result).toEqual({
    url: "https://unlisted.example.org/doc", status: 200, content_type: "text/plain",
    body: "abcd", bytes: 4, truncated: true,
  });
  expect(request?.method).toBe("GET");
  expect(credentials).toBe("omit");
  expect(request?.redirect).toBe("manual");
  expect([...request?.headers.keys() ?? []]).toEqual(["accept"]);
});

test("redirects reapply public address and optional named-host checks", async () => {
  // Given an allowed public first hop.
  for (const location of ["http://127.0.0.1/", "https://other.example.org/", "https://private.example.org/"]) {
    const sent: string[] = [];
    const web = createWebAccess({ ...settings, hosts: ["docs.example.org", "private.example.org"] }, async (url) => {
      sent.push(String(url));
      return new Response(null, { status: 302, headers: { location } });
    }, { lookup: async (host) => host === "private.example.org" ? [{ address: "10.0.0.1", family: 4 }] : lookup() });
    // When it redirects to an unapproved or private destination.
    await expect(web.fetch("https://docs.example.org/")).rejects.toThrow(WebError);
    // Then the unsafe second hop is never sent.
    expect(sent).toEqual(["https://docs.example.org/"]);
  }
});

test("relative public redirects work and cyclic redirects stop at the configured cap", async () => {
  // Given a relative documentation redirect and a looping route.
  const sent: string[] = [];
  const web = createWebAccess(settings, async (url) => {
    sent.push(String(url));
    return String(url).endsWith("/done")
      ? new Response("ok")
      : new Response(null, { status: 302, headers: { location: String(url).includes("/loop") ? "/loop" : "/done" } });
  }, { lookup });
  // When the ordinary redirect is followed.
  expect((await web.fetch("https://docs.example.org/start")).body).toBe("ok");
  expect(sent).toHaveLength(2);
  // When a redirect cycle is requested, then it remains bounded.
  sent.length = 0;
  await expect(web.fetch("https://docs.example.org/loop")).rejects.toThrow(/web_redirect_limit/);
  expect(sent).toHaveLength(settings.max_redirects + 1);
});

test("missing search configuration returns setup while public fetch remains available", async () => {
  // Given an enabled web capability without SearXNG.
  const web = createWebAccess(settings, async () => new Response("docs"), { lookup });
  // When search is requested, then its setup state is explicit.
  const search = await web.search("Rust docs", 5);
  expect(search.configured).toBeFalse();
  expect(search.results).toEqual([]);
  // And fetch needs no search endpoint.
  expect((await web.fetch("https://unlisted.example.org/")).body).toBe("docs");
});

test("SearXNG has a separate configured origin and bounded structured results", async () => {
  // Given a local search service and a fetch host restriction.
  let requested = "";
  const web = createWebAccess({ ...settings, hosts: ["docs.example.org"], searxng_url: "http://127.0.0.1:8888/prefix/" },
    async (url, init) => {
      requested = String(url);
      expect(init?.method).toBe("GET");
      expect(init?.credentials).toBe("omit");
      return Response.json({ results: [
        { title: "Documentation", url: "https://docs.example.org/", content: "Guide", ignored: "not returned" },
        { title: "Other", url: "https://other.example.org/", content: "Second" },
      ] });
    }, { lookup });
  // When the query is searched.
  const result = await web.search("a & b", 1);
  // Then only the configured search origin is contacted and result count is bounded.
  expect(requested).toBe("http://127.0.0.1:8888/prefix/search?q=a+%26+b&format=json");
  expect(result).toEqual({ configured: true, results: [
    { title: "Documentation", url: "https://docs.example.org/", content: "Guide" },
  ], truncated: true });
  await expect(web.fetch("http://127.0.0.1:8888/")).rejects.toThrow(WebError);
});

test("search never follows redirects to an unconfigured origin", async () => {
  // Given a search endpoint that redirects somewhere else.
  let sent = 0;
  const web = createWebAccess({ ...settings, searxng_url: "http://127.0.0.1:8888/search" }, async () => {
    sent++;
    return new Response(null, { status: 302, headers: { location: "http://127.0.0.2/admin" } });
  }, { lookup });
  // When search runs, then its exception does not authorize the next origin.
  await expect(web.search("query", 5)).rejects.toThrow(/web_search_failed/);
  expect(sent).toBe(1);
});

test("cancellation covers DNS before any network request", async () => {
  // Given a blocked resolver and a subscribed lookup-start barrier.
  const started = Promise.withResolvers<void>();
  const resolved = Promise.withResolvers<{ address: string; family: number }[]>();
  const controller = new AbortController();
  let sent = 0;
  const web = createWebAccess(settings, async () => { sent++; return new Response("unexpected"); }, {
    lookup: () => { started.resolve(); return resolved.promise; },
  });
  // When cancellation follows the exact lookup event.
  const request = web.fetch("https://docs.example.org/", controller.signal);
  await started.promise;
  controller.abort(new Error("fixture cancelled"));
  // Then the operation stops without waiting for DNS completion or sending a request.
  await expect(request).rejects.toThrow("fixture cancelled");
  expect(sent).toBe(0);
  resolved.resolve([]);
});

test("oversized search JSON and malformed result shapes are useful tool errors", async () => {
  // Given responses that cannot provide bounded valid search results.
  for (const body of ["x".repeat(1025), '{"results":42}']) {
    const web = createWebAccess({ ...settings, searxng_url: "http://127.0.0.1:8888" },
      async () => new Response(body), { lookup });
    // When parsing a search response, then invalid or truncated JSON is not reported as success.
    await expect(web.search("query", 5)).rejects.toThrow(WebError);
  }
});

test("truncation never expands an incomplete UTF-8 character beyond the byte cap", async () => {
  // Given a multibyte character cut in the middle of its encoding.
  const web = createWebAccess({ ...settings, max_bytes: 1 }, async () => new Response("漢字"), { lookup });
  // When the response is capped, then replacement characters cannot grow its output.
  const result = await web.fetch("https://docs.example.org/");
  expect(result.truncated).toBeTrue();
  expect(Buffer.byteLength(result.body)).toBeLessThanOrEqual(1);
});

test("cancelling a stalled body releases its stream without waiting for EOF", async () => {
  // Given a response whose body has started but cannot finish.
  const reading = Promise.withResolvers<void>();
  const blocked = Promise.withResolvers<void>();
  let cancelled = false;
  const body = new ReadableStream<Uint8Array>({
    pull() { reading.resolve(); return blocked.promise; },
    cancel() { cancelled = true; },
  }, { highWaterMark: 0 });
  const controller = new AbortController();
  const web = createWebAccess(settings, async () => new Response(body), { lookup });
  // When the exact body-read event is followed by cancellation.
  const result = web.fetch("https://docs.example.org/", controller.signal);
  await reading.promise;
  controller.abort(new Error("body cancelled"));
  // Then the call ends and cleans up its reader.
  await expect(result).rejects.toThrow("body cancelled");
  expect(cancelled).toBeTrue();
  blocked.resolve();
});
