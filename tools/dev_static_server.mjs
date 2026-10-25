#!/usr/bin/env node

import fs from "node:fs";
import { clientBuildInfo, stampClientBuildInfo } from "./client_build_info.mjs";
import http from "node:http";
import { createGameHttpProxy } from "./game_http_proxy.mjs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const host = process.env.HOST || "127.0.0.1";
const port = Number(process.argv[2] || process.env.PORT || 7000);
const servedRoot = path.resolve(process.argv[3] || process.env.ROOT || repoRoot);
const startedAtMs = Date.now();
const gameProxy = createGameHttpProxy(process.env.RTC_OFFER_ENDPOINT || "http://127.0.0.1:9001/v1/rtc/offer");

function mimeType(filename) {
  if (filename.endsWith(".html")) return "text/html; charset=utf-8";
  if (filename.endsWith(".js") || filename.endsWith(".mjs")) return "text/javascript; charset=utf-8";
  if (filename.endsWith(".wasm")) return "application/wasm";
  if (filename.endsWith(".css")) return "text/css; charset=utf-8";
  if (filename.endsWith(".png")) return "image/png";
  if (filename.endsWith(".jpg") || filename.endsWith(".jpeg")) return "image/jpeg";
  if (filename.endsWith(".gif")) return "image/gif";
  if (filename.endsWith(".bmp") || filename.endsWith(".bm2")) return "image/bmp";
  if (filename.endsWith(".wav") || filename.endsWith(".wa2")) return "audio/wav";
  if (filename.endsWith(".txt")) return "text/plain; charset=utf-8";
  return "application/octet-stream";
}

function safePathForRequest(requestUrl) {
  let pathname;
  try {
    const url = new URL(requestUrl, `http://${host}`);
    pathname = decodeURIComponent(url.pathname === "/" ? "/index.html" : url.pathname);
  } catch {
    return null;
  }

  const resolved = path.resolve(servedRoot, `.${pathname}`);
  if (resolved !== servedRoot && !resolved.startsWith(servedRoot + path.sep)) {
    return null;
  }
  return resolved;
}

function requestPathname(requestUrl) {
  try {
    return new URL(requestUrl, `http://${host}`).pathname;
  } catch {
    return null;
  }
}

function statusBody() {
  return JSON.stringify({
    ok: true,
    service: "zubspace-client",
    uptime_seconds: Math.floor((Date.now() - startedAtMs) / 1000),
    started_at_ms: startedAtMs,
    root: servedRoot,
  });
}

const server = http.createServer((request, response) => {
  if (gameProxy(request, response)) return;
  if (request.method !== "GET" && request.method !== "HEAD") {
    response.writeHead(405, { allow: "GET, HEAD" });
    response.end("method not allowed");
    return;
  }

  if (requestPathname(request.url || "/") === "/client-status") {
    const body = statusBody();
    response.writeHead(200, {
      "content-type": "application/json; charset=utf-8",
      "content-length": Buffer.byteLength(body),
      "cache-control": "no-store",
      "access-control-allow-origin": "*",
    });
    response.end(request.method === "HEAD" ? undefined : body);
    return;
  }

  const resolved = safePathForRequest(request.url || "/");
  if (!resolved) {
    response.writeHead(403);
    response.end("forbidden");
    return;
  }

  fs.readFile(resolved, (error, data) => {
    if (error) {
      response.writeHead(404);
      response.end("not found");
      return;
    }

    if (servedRoot === repoRoot && resolved === path.join(repoRoot, "index.html")) {
      data = Buffer.from(stampClientBuildInfo(data.toString("utf8"), clientBuildInfo(repoRoot)));
    }
    response.writeHead(200, {
      "content-type": mimeType(resolved),
      "cache-control": "no-store",
    });
    if (request.method === "HEAD") {
      response.end();
    } else {
      response.end(data);
    }
  });
});

server.on("error", (error) => {
  if (error.code === "EADDRINUSE") {
    console.error(`${host}:${port} is already in use`);
  } else {
    console.error(error);
  }
  process.exit(1);
});

server.listen(port, host, () => {
  const address = server.address();
  console.log(`zubspace static server listening on http://${host}:${address.port}/index.html`);
  console.log(`serving ${servedRoot}`);
});
