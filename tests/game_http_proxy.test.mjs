import assert from "node:assert/strict";
import http from "node:http";
import test from "node:test";
import { createGameHttpProxy } from "../tools/game_http_proxy.mjs";

async function listen(server) {
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  return `http://127.0.0.1:${server.address().port}`;
}
async function close(server) {
  server.closeAllConnections();
  await new Promise(resolve => server.close(resolve));
}

test("clean-URL proxy relays signaling/status and authenticated QA uploads to its configured game server", async () => {
  const received = [];
  const game = http.createServer(async (request, response) => {
    let body = "";
    for await (const part of request) body += part;
    received.push({ path: request.url, method: request.method, body, headers: request.headers });
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify({ ok: true, path: request.url }));
  });
  const upstream = await listen(game);
  const proxy = createGameHttpProxy(`${upstream}/v1/rtc/offer`);
  const front = http.createServer((request, response) => {
    if (!proxy(request, response)) { response.writeHead(404); response.end(); }
  });
  const base = await listen(front);
  try {
    const answer = await fetch(`${base}/v1/rtc/offer?host=evil.invalid`, {
      method: "POST", body: '{"sdp":"real offer"}',
      headers: { "content-type": "application/json", "x-forwarded-for": "forged", "x-forwarded-proto": "https" },
    });
    assert.equal(answer.status, 200);
    assert.deepEqual(await answer.json(), { ok: true, path: "/v1/rtc/offer" });
    assert.equal(received[0].body, '{"sdp":"real offer"}');
    assert.equal(received[0].headers["x-forwarded-for"], "127.0.0.1");
    assert.equal(received[0].headers["x-forwarded-proto"], "http");
    assert.equal((await fetch(`${base}/v1/status`)).status, 200);
    assert.equal((await fetch(`${base}/v1/zones`)).status, 200);
    const traceBody = '{"schema":"zubspace-qa-trace-browser-v1"}';
    const trace = await fetch(`${base}/v1/qa/trace?host=evil.invalid`, { method: "POST", body: traceBody,
      headers: { "content-type": "application/json", "x-zubspace-qa-token": "session-token",
        "x-zubspace-qa-bundle": "0123456789abcdef0123456789abcdef", origin: base } });
    assert.equal(trace.status, 200);
    assert.equal(received[3].body, traceBody);
    assert.equal(received[3].headers["x-zubspace-qa-token"], "session-token");
    assert.equal(received[3].headers["x-zubspace-qa-bundle"], "0123456789abcdef0123456789abcdef");
    assert.equal(received[3].headers.origin, base);
    assert.equal((await fetch(`${base}/v1/qa/trace`)).status, 405);
    assert.equal((await fetch(`${base}/v1/qa/trace/0123456789abcdef0123456789abcdef`)).status, 404,
      "host-local trace retrieval cannot be exposed through the public frontend");
    assert.equal((await fetch(`${base}/v1/qa/trace/0123456789abcdef0123456789abcdef`, { method: "POST" })).status, 404);
    assert.equal((await fetch(`${base}/private`)).status, 404);
    assert.equal((await fetch(`${base}/v1/rtc/offer`)).status, 405);
    assert.equal((await fetch(`${base}/v1/status`, { method: "POST" })).status, 405);
    for (const route of ["/v1/players", "/v1/ping"]) {
      assert.equal((await fetch(`${base}${route}?host=evil.invalid`)).status, 200);
      assert.equal((await fetch(`${base}${route}`, { method: "HEAD" })).status, 200);
      assert.equal((await fetch(`${base}${route}`, { method: "POST", body: "x" })).status, 405);
    }
    for (const route of ["/players", "/ping"]) assert.equal((await fetch(`${base}${route}`)).status, 404);
    assert.deepEqual(received.map(value => value.path), ["/v1/rtc/offer", "/v1/status", "/v1/zones", "/v1/qa/trace",
      "/v1/players", "/v1/players", "/v1/ping", "/v1/ping"]);
    await close(game);
    assert.equal((await fetch(`${base}/v1/status`)).status, 502);
  } finally {
    await close(front);
    if (game.listening) await close(game);
  }
});

test("proxy rejects ambiguous or credential-bearing endpoint configuration", () => {
  for (const endpoint of ["file:///tmp/data", "http://user:password@localhost/v1/rtc/offer", "http://localhost/v1/rtc/offer?token=secret", "http://localhost/v1/rtc/offer#fragment"]) {
    assert.throws(() => createGameHttpProxy(endpoint));
  }
});

test("client debug policy/upload use only the configured upstream and preserve diagnostic credentials", async () => {
  const requests=[];
  const game=http.createServer(async (request,response)=>{
    let body="";for await (const chunk of request) body+=chunk;
    requests.push({url:request.url,method:request.method,token:request.headers["x-zubspace-qa-token"],body});
    response.writeHead(200,{"content-type":"application/json"});response.end('{"enabled":true}');
  });
  const base=await listen(game), proxy=createGameHttpProxy(`${base}/v1/rtc/offer`);
  const front=http.createServer((request,response)=>{if(!proxy(request,response)){response.writeHead(404);response.end();}});
  const url=await listen(front);
  try {
    assert.equal((await fetch(`${url}/v1/debug/client-trace?host=evil.invalid`,{headers:{"x-zubspace-qa-token":"ephemeral-token"}})).status,200);
    assert.equal((await fetch(`${url}/v1/debug/client-trace`,{method:"POST",headers:{"content-type":"application/json","x-zubspace-qa-token":"ephemeral-token"},body:'{"sequence":1}'})).status,200);
    assert.equal((await fetch(`${url}/debug/anything-else`)).status,404);
    assert.deepEqual(requests,[{url:"/v1/debug/client-trace",method:"GET",token:"ephemeral-token",body:""},{url:"/v1/debug/client-trace",method:"POST",token:"ephemeral-token",body:'{"sequence":1}'}]);
  } finally {await close(front);await close(game);}
});

test("zone-scoped public proxy paths remain pinned to the configured upstream", async () => {
  const received=[];
  const game=http.createServer((request,response)=>{received.push(request.url);response.writeHead(200);response.end("ok");});
  const upstream=await listen(game),proxy=createGameHttpProxy(`${upstream}/v1/rtc/offer`);
  const front=http.createServer((request,response)=>{if(!proxy(request,response)){response.writeHead(404);response.end();}});
  const base=await listen(front);
  try {
    for(const route of ["/v1/zones/chaos-zone","/v1/zones/extreme/players","/v1/zones/custom/status"]) {
      assert.equal((await fetch(`${base}${route}?host=evil.invalid`)).status,200);
      assert.equal(received.at(-1),route);
      assert.equal((await fetch(`${base}${route}`,{method:"POST"})).status,405);
    }
    const count=received.length;
    for(const route of ["/v1/zones/chaos-zone/extra","/v1/zones/chaos-zone/players/extra","/v1/zones/Upper","/v1/zones/%63haos-zone","/v1/zones/-bad"]) {
      assert.equal((await fetch(`${base}${route}`)).status,404,route);
    }
    assert.equal(received.length,count);
  } finally {await close(front);await close(game);}
});
