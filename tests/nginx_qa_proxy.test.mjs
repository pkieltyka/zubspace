import test from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";

// Opt-in local integration check against the release image's actual nginx.
// No existing services, images or containers are modified.
test("release nginx proxies only QA upload and preserves host-local retrieval", { timeout: 20_000 }, async () => {
  const received = [];
  const backend = http.createServer(async (request, response) => {
    let body = ""; for await (const chunk of request) body += chunk;
    received.push({ path: request.url, headers: request.headers, body });
    response.writeHead(200, { "content-type": "application/json" }); response.end('{"ok":true}');
  });
  await new Promise(resolve => backend.listen(0, "127.0.0.1", resolve));
  const probe = http.createServer(); await new Promise(resolve => probe.listen(0, "127.0.0.1", resolve));
  const port = probe.address().port; await new Promise(resolve => probe.close(resolve));
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-nginx-qa-"));
  const assets = path.join(directory, "assets");
  fs.mkdirSync(assets);
  for (const [name, content] of Object.entries({ "index.html": "<!doctype html><title>nginx MIME probe</title>",
    "client.js": "void 0;", "client.wasm": Buffer.from([0, 97, 115, 109, 1, 0, 0, 0]), "sprite.png": Buffer.from([137, 80, 78, 71]) })) {
    fs.writeFileSync(path.join(assets, name), content);
  }
  const template = fs.readFileSync(new URL("../docker/nginx.conf.template", import.meta.url), "utf8")
    .replaceAll("${ZUBSPACE_SERVER_UPSTREAM}", `127.0.0.1:${backend.address().port}`)
    .replaceAll("${ZUBSPACE_CLIENT_LISTEN_ADDRESS}", "127.0.0.1")
    .replaceAll("${ZUBSPACE_CLIENT_LISTEN_PORT}", String(port))
    .replaceAll("${ZUBSPACE_ROBOTS_TAG}", "");
  fs.writeFileSync(path.join(directory, "nginx.conf"), `events {}\nhttp { ${template}\n}\n`);
  const image = fs.readFileSync(new URL("../Dockerfile.client", import.meta.url), "utf8").match(/^FROM (\S*nginx:\S+) AS runtime$/m)?.[1];
  assert.ok(image);
  const name = `zubspace-qa-proxy-${randomUUID()}`;
  const child = spawn("docker", ["run", "--rm", "--name", name, "--network", "host", "--volume", `${directory}:/qa-test:ro`,
    "--volume", `${assets}:/usr/share/nginx/html:ro`,
    "--entrypoint", "nginx", image, "-c", "/qa-test/nginx.conf", "-g", "daemon off;"], { stdio: ["ignore", "pipe", "pipe"] });
  let logs = ""; child.stdout.on("data", b => logs += b); child.stderr.on("data", b => logs += b);
  const base = `http://127.0.0.1:${port}`;
  try {
    const deadline = Date.now()+10_000; let started = false;
    while (Date.now()<deadline) {
      if (child.exitCode != null) assert.fail(`nginx exited ${child.exitCode}: ${logs}`);
      try { if ((await fetch(`${base}/healthz`)).status === 200) { started = true; break; } } catch {}
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    assert.ok(started, `nginx startup: ${logs}`);
    for (const [file, mime] of [["index.html", "text/html"], ["client.js", "application/javascript"],
      ["client.wasm", "application/wasm"], ["sprite.png", "image/png"]]) {
      const asset = await fetch(`${base}/${file}`);
      assert.equal(asset.status, 200, file);
      assert.equal(asset.headers.get("content-type"), mime, `${file} MIME type`);
      await asset.arrayBuffer();
    }
    assert.doesNotMatch(logs, /duplicate extension|\[warn\]/, "nginx should load the full MIME map without redeclarations");
    // The loopback caller stands in for cloudflared, which adds both headers.
    const response = await fetch(`${base}/v1/qa/trace`, { method: "POST", body: '{"trace":"payload"}', headers: {
      "content-type": "application/json", origin: base, "x-zubspace-qa-token": "test-token", "x-zubspace-qa-bundle": "test-bundle",
      "cf-connecting-ip": "203.0.113.7", "x-forwarded-proto": "https" } });
    assert.equal(response.status, 200); assert.deepEqual(await response.json(), { ok: true });
    assert.equal(received[0].path, "/v1/qa/trace"); assert.equal(received[0].body, '{"trace":"payload"}');
    assert.equal(received[0].headers["x-real-ip"], "203.0.113.7");
    assert.equal(received[0].headers["x-forwarded-for"], "203.0.113.7");
    assert.equal(received[0].headers["x-forwarded-proto"], "https", "HTTPS trust must survive the real_ip rewrite");
    assert.equal(received[0].headers.origin, base);
    assert.equal(received[0].headers["x-zubspace-qa-token"], "test-token");
    assert.equal(received[0].headers["x-zubspace-qa-bundle"], "test-bundle");
    assert.equal((await fetch(`${base}/v1/qa/trace`)).status, 403);
    assert.equal((await fetch(`${base}/v1/qa/trace/0123456789abcdef0123456789abcdef`)).status, 404);
    assert.equal((await fetch(`${base}/v1/qa/trace`, { method: "POST", body: "x".repeat(256*1024+1) })).status, 413);
    assert.equal(received.length, 1, "denied methods, retrieval and oversized upload must not reach backend");
    // The menu's public roster check and the liveness route reach the game server read-only.
    for (const route of ["/v1/players", "/v1/ping"]) {
      assert.equal((await fetch(`${base}${route}`)).status, 200, `${route} must be proxied`);
      assert.equal(received.at(-1).path, route);
      assert.equal((await fetch(`${base}${route}`, { method: "POST", body: "x" })).status, 403);
    }
    assert.equal(received.length, 3, "only the two public GETs may reach the backend");
    // API hostnames (server.zubspace.gg, dev-server.zubspace.gg) expose only the public API.
    const viaHost = (host, route, method = "GET") => new Promise((resolve, reject) => {
      const request = http.request({ host: "127.0.0.1", port, path: route, method, headers: { host, "cf-connecting-ip": "203.0.113.9" } },
        response => { response.resume(); response.on("end", () => resolve(response.statusCode)); });
      request.on("error", reject); request.end();
    });
    for (const host of ["server.zubspace.gg", "dev-server.zubspace.gg"]) {
      for (const route of ["/v1/status", "/v1/players", "/v1/ping"]) {
        assert.equal(await viaHost(host, route), 200, `${host}${route} must be proxied`);
        assert.equal(received.at(-1).path, route);
        assert.equal(received.at(-1).headers["x-real-ip"], "203.0.113.9");
        assert.equal(await viaHost(host, route, "POST"), 403);
      }
      for (const route of ["/", "/index.html", "/v1/rtc/offer", "/v1/qa/trace", "/healthz", "/v1/status/x",
        "/status", "/players", "/ping"]) {
        assert.equal(await viaHost(host, route), 404, `${host}${route} must not be served`);
      }
    }
    assert.equal(received.length, 9, "API hostnames may reach only the three public GETs");
    for (const host of ["server.zubspace.gg", "dev-server.zubspace.gg", "dev.zubspace.gg"]) {
      for (const route of ["/v1/zones", "/v1/zones/chaos-zone", "/v1/zones/extreme/players", "/v1/zones/extreme/status"]) {
        assert.equal(await viaHost(host, route), 200, `${host}${route} must reach the game server`);
        assert.equal(received.at(-1).path, route);
        assert.equal(await viaHost(host, route, "POST"), 403);
      }
      for (const route of ["/v1/zones/chaos-zone/extra", "/v1/zones/extreme/players/extra"]) {
        assert.equal(await viaHost(host, route),404);
      }
    }
    assert.equal(await viaHost("dev.zubspace.gg", "/healthz"), 200, "the site hostname keeps the default server");
    // The game server API lives only under /v1; the unversioned paths are gone.
    for (const route of ["/status", "/players", "/ping", "/zones", "/rtc/offer", "/qa/trace", "/debug/client-trace"]) {
      assert.equal(await viaHost("dev.zubspace.gg", route), 404, `${route} must not be proxied without /v1`);
    }
    assert.equal(received.length, 21, "unversioned paths must not reach the backend");
    console.log(JSON.stringify({ image, upload: 200, retrieval: 404, wrongMethod: 403, oversized: 413, players: 200, ping: 200,
      apiHosts: "status/players/ping/zones and scoped zone reads" }));
  } finally {
    spawnSync("docker", ["rm", "--force", name], { stdio: "ignore" });
    backend.closeAllConnections(); await new Promise(resolve => backend.close(resolve));
    fs.rmSync(directory, { recursive: true, force: true });
  }
});
