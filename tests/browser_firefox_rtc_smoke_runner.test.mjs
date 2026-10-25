import assert from "node:assert/strict";
import http from "node:http";
import test from "node:test";
import vm from "node:vm";

import { webdriver } from "../tools/browser_firefox_rtc_smoke.mjs";
import * as firefox from "../tools/browser_firefox_rtc_smoke.mjs";

test("server profile selects the same explicit binary for launch and provenance", () => {
  assert.equal(firefox.serverBinaryForProfile(), "target/debug/zubspace_server");
  assert.equal(firefox.serverBinaryForProfile("release"), "target/release/zubspace_server");
  for (const profile of ["", "Release", "../release", "custom"]) {
    assert.throws(() => firefox.serverBinaryForProfile(profile), /unsupported E2E server profile/);
  }
});

test("Firefox chord can retain held keys for a separately observed release", async () => {
  const requests = [];
  const server = http.createServer(async (request, response) => {
    let body = "";
    for await (const chunk of request) body += chunk;
    requests.push({ method: request.method, url: request.url, body: body ? JSON.parse(body) : null });
    response.setHeader("content-type", "application/json");
    response.end(JSON.stringify({ value: true }));
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const driver = { port: server.address().port, sessionId: "chord" };
  try {
    await firefox.holdThenChordFirefoxKeys(driver, "Control", "ArrowLeft", 180, 520, []);
    assert.deepEqual(requests.map(x => [x.method, x.url]), [
      ["POST", "/session/chord/execute/sync"], ["POST", "/session/chord/actions"],
    ], "DELETE /actions would synthesize both key releases before the intermediate proof");
    assert.equal(requests[1].body.actions[0].actions.filter(x => x.type === "keyUp").length, 0);
    requests.length = 0;
    await firefox.holdThenChordFirefoxKeys(driver, "Control", "ArrowLeft");
    assert.equal(requests.at(-1).method, "DELETE", "ordinary complete chords still release their input source");
    assert.equal(requests[1].body.actions[0].actions.filter(x => x.type === "keyUp").length, 2);
  } finally {
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  }
});

test("static QA listener defaults to loopback and records an explicit local bind", async () => {
  for (const options of [undefined, { host: "127.0.0.2" }]) {
    const fixture = await firefox.startStaticServer(options);
    try {
      assert.equal(fixture.server.address().address, options?.host ?? "127.0.0.1");
      assert.equal(fixture.host, options?.host ?? "127.0.0.1");
      const response = await fetch(`http://${fixture.host}:${fixture.port}/v1/status`);
      assert.equal(response.status, 200);
    } finally {
      fixture.server.closeAllConnections();
      await new Promise(resolve => fixture.server.close(resolve));
    }
  }
});

async function withWebdriverPage(window, run) {
  const requests = [];
  const server = http.createServer(async (request, response) => {
    try {
      let body = "";
      for await (const chunk of request) body += chunk;
      const { script, args = [] } = JSON.parse(body);
      requests.push(request.url);
      const execute = vm.runInNewContext(`(function() { ${script} })`, { window });
      const value = request.url.endsWith("/execute/async")
        ? await new Promise((resolve, reject) => {
          try { execute(...args, resolve); } catch (error) { reject(error); }
        })
        : await execute(...args);
      response.end(JSON.stringify({ value }));
    } catch (error) {
      response.statusCode = 500;
      response.end(JSON.stringify({ value: { error: "javascript error", message: error.message } }));
    }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    await run({ port: server.address().port, sessionId: "trace-test" }, requests);
  } finally {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
}

test("Firefox full artifact state includes the safe QA state and frozen bundle", async () => {
  const qaState = { enabled: true, ready: true, bundleId: "test-bundle" };
  const bundle = { schema: "zubspace-qa-trace-v1", bundleId: "test-bundle" };
  await withWebdriverPage({
    location: { href: "http://localhost/index.html?trace=1" },
    __zubspaceQaTraceStateForTest: () => qaState,
    __zubspaceLastQaTraceBundle: bundle,
  }, async (driver) => {
    const state = await firefox.pageState(driver);
    assert.deepEqual(state.qaTraceState, qaState);
    assert.deepEqual(state.qaTraceBundle, bundle);
  });
});

test("Firefox marker waits for completion and uses non-uploading artifact defaults", async () => {
  assert.equal(typeof firefox.freezeQaTrace, "function");
  let calls = 0;
  await withWebdriverPage({
    __zubspaceQaTraceStateForTest: () => ({ enabled: true, ready: true }),
    __zubspaceMarkQaIssue: async (options) => {
      calls += 1;
      assert.deepEqual(JSON.parse(JSON.stringify(options)), {
        postTailMs: 0, download: false, upload: false,
      });
      await new Promise((resolve) => setTimeout(resolve, 10));
      return { bundleId: "completed" };
    },
  }, async (driver, requests) => {
    assert.deepEqual(await firefox.freezeQaTrace(driver), { bundleId: "completed" });
    assert.deepEqual(requests, ["/session/trace-test/execute/async"]);
    assert.equal(calls, 1);
  });
});

test("Firefox marker propagates rejection and never enables disabled tracing", async () => {
  assert.equal(typeof firefox.freezeQaTrace, "function");
  let enabled = true;
  let calls = 0;
  await withWebdriverPage({
    __zubspaceQaTraceStateForTest: () => ({ enabled, ready: enabled }),
    __zubspaceMarkQaIssue: async () => {
      calls += 1;
      throw new Error("qa_trace_rate_limited");
    },
  }, async (driver) => {
    await assert.rejects(firefox.freezeQaTrace(driver), /qa_trace_rate_limited/);
    enabled = false;
    assert.equal(await firefox.freezeQaTrace(driver), null);
    assert.equal(calls, 1);
  });
});

test("Firefox WebDriver requests abort a stalled command at their owned deadline", async () => {
  const server = http.createServer(() => {});
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const port = server.address().port;
  const startedAt = Date.now();
  try {
    await assert.rejects(
      webdriver(
        { port, label: "deadline-test" },
        "POST",
        "/session/test/actions",
        { actions: [] },
        { timeoutMs: 40 },
      ),
      /deadline-test WebDriver POST \/session\/test\/actions timed out after 40ms/,
    );
    assert.ok(Date.now() - startedAt < 1_000);
  } finally {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
});
