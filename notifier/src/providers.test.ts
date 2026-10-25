import assert from "node:assert/strict";
import { test } from "node:test";
import { createSender, slackPayload } from "./providers.ts";

interface Call {
  url: string;
  method: string | undefined;
  contentType: string | null;
  body: string;
}

function fakeFetch(responses: (Response | Error)[]) {
  const calls: Call[] = [];
  const fetch = async (input: string | URL | Request, init?: RequestInit) => {
    calls.push({
      url: String(input),
      method: init?.method,
      contentType: new Headers(init?.headers).get("content-type"),
      body: String(init?.body),
    });
    const next = responses.shift() ?? new Response("ok");
    if (next instanceof Error) throw next;
    return next;
  };
  return { calls, fetch: fetch as typeof globalThis.fetch };
}

test("ntfy posts the text to the topic", async () => {
  const { calls, fetch } = fakeFetch([]);
  await createSender({ kind: "ntfy", server: new URL("https://ntfy.sh"), topic: "zub" }, { fetch })("Zub has joined Chaos Zone");
  assert.deepEqual(calls, [
    { url: "https://ntfy.sh/zub", method: "POST", contentType: "text/plain; charset=utf-8", body: "Zub has joined Chaos Zone" },
  ]);
});

test("a self-hosted ntfy server under a path keeps its path", async () => {
  const { calls, fetch } = fakeFetch([]);
  await createSender({ kind: "ntfy", server: new URL("https://example.com/ntfy"), topic: "zub" }, { fetch })("hi");
  assert.equal(calls[0]?.url, "https://example.com/ntfy/zub");
});

test("slack posts plain text JSON to the webhook", async () => {
  const { calls, fetch } = fakeFetch([]);
  const webhookUrl = new URL("https://hooks.slack.com/services/T/B/x");
  await createSender({ kind: "slack", webhookUrl }, { fetch })("*Zub* has left Chaos Zone");
  assert.equal(calls[0]?.url, webhookUrl.href);
  assert.equal(calls[0]?.contentType, "application/json");
  assert.deepEqual(JSON.parse(calls[0]?.body ?? ""), { text: "*Zub* has left Chaos Zone", mrkdwn: false });
  assert.deepEqual(slackPayload("a<b>&c"), { text: "a&lt;b&gt;&amp;c", mrkdwn: false });
});

test("network errors, 429 and 5xx are retried up to three attempts", async () => {
  const ntfy = { kind: "ntfy", server: new URL("https://ntfy.sh"), topic: "zub" } as const;

  const flaky = fakeFetch([new TypeError("fetch failed"), new Response("busy", { status: 429 }), new Response("ok")]);
  await createSender(ntfy, { fetch: flaky.fetch, retryDelayMs: 0 })("hi");
  assert.equal(flaky.calls.length, 3);

  const down = fakeFetch([new Response("", { status: 502 }), new Response("", { status: 503 }), new Response("", { status: 500 })]);
  await assert.rejects(createSender(ntfy, { fetch: down.fetch, retryDelayMs: 0 })("hi"), /HTTP 500/);
  assert.equal(down.calls.length, 3);
});

test("other client errors are not retried", async () => {
  const { calls, fetch } = fakeFetch([new Response("no_service", { status: 404 })]);
  const send = createSender({ kind: "slack", webhookUrl: new URL("https://hooks.slack.com/services/T/B/x") }, { fetch, retryDelayMs: 0 });
  await assert.rejects(send("hi"), /HTTP 404 no_service/);
  assert.equal(calls.length, 1);
});
