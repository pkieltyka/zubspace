import http from "node:http";
import https from "node:https";

const hopHeaders = new Set(["connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "te", "trailer", "transfer-encoding", "upgrade"]);
function endToEndHeaders(headers) {
  const excluded = new Set([...hopHeaders, ...String(headers.connection || "").split(",").map(value => value.trim().toLowerCase())]);
  return Object.fromEntries(Object.entries(headers).filter(([name]) => !excluded.has(name.toLowerCase())));
}

// Only the configured game's signaling/discovery/status and QA upload are proxied. The request
// URL can never select a host, arbitrary upstream path, or WebSocket tunnel.
// Never proxy /v1/qa/trace/<id>: retrieval is host-local on the game server.
export function createGameHttpProxy(offerEndpoint) {
  const offer = new URL(offerEndpoint);
  if (!["http:", "https:"].includes(offer.protocol) || offer.username || offer.password || offer.search || offer.hash) {
    throw new Error("RTC offer endpoint must be an HTTP(S) URL without credentials, query or fragment");
  }
  return (request, response) => {
    let incoming;
    try { incoming = new URL(request.url || "/", "http://local.invalid"); }
    catch { response.writeHead(400); response.end("invalid request URL"); return true; }
    const scopedZone = /^\/v1\/zones\/[a-z0-9](?:[a-z0-9-]{0,13}[a-z0-9])?(?:\/(?:players|status))?$/.test(incoming.pathname);
    if (!scopedZone && !["/v1/rtc/offer", "/v1/status", "/v1/players", "/v1/ping", "/v1/zones", "/v1/qa/trace", "/v1/debug/client-trace"].includes(incoming.pathname)) return false;
    const allowed = incoming.pathname === "/v1/debug/client-trace" ? ["GET", "POST"] : ["/v1/rtc/offer", "/v1/qa/trace"].includes(incoming.pathname) ? ["POST"] : ["GET", "HEAD", "OPTIONS"];
    if (!allowed.includes(request.method)) {
      response.writeHead(405, { allow: allowed.join(", ") });
      response.end("method not allowed");
      return true;
    }
    const target = new URL(offer);
    if (incoming.pathname !== "/v1/rtc/offer") target.pathname = incoming.pathname;
    const headers = endToEndHeaders(request.headers);
    headers.host = target.host;
    headers["x-forwarded-for"] = request.socket.remoteAddress || "";
    headers["x-forwarded-proto"] = request.socket.encrypted ? "https" : "http";
    const transport = target.protocol === "https:" ? https : http;
    const upstream = transport.request(target, { method: request.method, headers }, reply => {
      response.writeHead(reply.statusCode || 502, endToEndHeaders(reply.headers));
      reply.on("error", () => response.destroy());
      reply.pipe(response);
    });
    upstream.setTimeout(30_000, () => upstream.destroy(new Error("game signaling timed out")));
    upstream.on("error", () => {
      if (response.headersSent) response.destroy();
      else {
        response.writeHead(502, { "content-type": "text/plain", "cache-control": "no-store" });
        response.end("Game server unavailable. Please try again.");
      }
    });
    request.on("aborted", () => upstream.destroy());
    response.on("close", () => { if (!response.writableFinished) upstream.destroy(); });
    request.pipe(upstream);
    return true;
  };
}
