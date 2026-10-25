import assert from "node:assert/strict";
import http from "node:http";
import test from "node:test";
import { holdFirefoxKeys } from "../../tools/browser_firefox_rtc_smoke.mjs";

test("the real Firefox key helper encodes Escape as one WebDriver key", async () => {
  const requests = [];
  const server = http.createServer(async (request, response) => {
    let body = "";
    for await (const chunk of request) body += chunk;
    requests.push({ path: request.url, method: request.method, body: body ? JSON.parse(body) : null });
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify({ value: null }));
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  try {
    await holdFirefoxKeys({ port: server.address().port, sessionId: "native", label: "native" }, ["Escape"], 1);
    const actions = requests.filter(request => request.method === "POST" && request.path.endsWith("/actions"));
    assert.equal(actions.length, 1);
    const keys = actions[0].body.actions.flatMap(source => source.actions).filter(action => action.type.startsWith("key"));
    assert.deepEqual(keys, [{ type: "keyDown", value: "\uE00C" }, { type: "keyUp", value: "\uE00C" }]);
    assert.ok(requests.some(request => request.method === "DELETE" && request.path.endsWith("/actions")));
  } finally { await new Promise(resolve => server.close(resolve)); }
});
