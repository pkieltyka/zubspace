#!/usr/bin/env node
// Bring the Cloudflare zone in line with deploy/cloudflare/<zone>.json.
//
//   node scripts/cloudflare.mjs plan     # show differences, change nothing
//   node scripts/cloudflare.mjs apply    # make Cloudflare match the file
//
// Manages zone settings, tunnel ingress, DNS records and the redirect and rate
// limit rulesets. A DNS record that the file does not list is reported, never
// deleted. Tunnel tokens are not managed: they live only on the host.
//
//   CLOUDFLARE_TOKEN_FILE   API token file (default .secrets/cloudflare-token)
//   CLOUDFLARE_CONFIG       desired state (default deploy/cloudflare/zubspace.gg.json)
//
// Exit status: 0 when Cloudflare matches (plan) or was updated (apply), 2 when
// plan found differences, 1 on errors.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const mode = process.argv[2];
if (!["plan", "apply"].includes(mode)) {
  console.error("usage: node scripts/cloudflare.mjs plan|apply");
  process.exit(1);
}
const config = JSON.parse(fs.readFileSync(path.resolve(root, process.env.CLOUDFLARE_CONFIG ?? "deploy/cloudflare/zubspace.gg.json"), "utf8"));
// Cloudflare rejects DNS comments over 100 characters; fail before changing anything.
for (const record of config.dns ?? []) {
  if ((record.comment ?? "").length > 100) {
    console.error(`dns ${record.name}: comment is ${record.comment.length} characters; Cloudflare allows 100`);
    process.exit(1);
  }
}
// The token travels only in a request header, never in argv or output.
const token = fs.readFileSync(path.resolve(root, process.env.CLOUDFLARE_TOKEN_FILE ?? ".secrets/cloudflare-token"), "utf8").trim();

async function cf(method, pathname, body) {
  const response = await fetch(`https://api.cloudflare.com/client/v4${pathname}`, {
    method,
    headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const json = await response.json().catch(() => ({}));
  if (!json.success) {
    const error = new Error(`${method} ${pathname}: HTTP ${response.status} ${JSON.stringify(json.errors ?? json)}`);
    error.status = response.status;
    throw error;
  }
  return json.result;
}

// Equal after dropping null/undefined values and ordering object keys.
const canonical = (value) => JSON.stringify(value, (_, v) =>
  v && typeof v === "object" && !Array.isArray(v)
    ? Object.fromEntries(Object.keys(v).sort().filter((k) => v[k] != null).map((k) => [k, v[k]]))
    : v);
const short = (value) => { const text = canonical(value); return text.length > 160 ? `${text.slice(0, 157)}...` : text; };

let differences = 0;
async function reconcile(label, current, desired, write) {
  if (canonical(current) === canonical(desired)) {
    console.log(`ok      ${label}`);
    return;
  }
  differences += 1;
  console.log(`${current === undefined ? "create " : "change "} ${label}`);
  if (current !== undefined) console.log(`          now:  ${short(current)}`);
  console.log(`          want: ${short(desired)}`);
  if (mode === "apply") {
    await write();
    console.log("          applied");
  }
}

function composeEnv(env) {
  const file = path.join(root, "deploy", env, "compose.env");
  return Object.fromEntries(fs.readFileSync(file, "utf8").split("\n")
    .map((line) => line.match(/^([A-Z0-9_]+)=(.*)$/)).filter(Boolean).map((m) => [m[1], m[2]]));
}
function expand(text, values, where) {
  return text.replace(/\$\{([A-Z0-9_]+)\}/g, (_, name) => {
    if (values[name] === undefined) throw new Error(`${where}: ${name} is not set in compose.env`);
    return values[name];
  });
}

const [zone] = await cf("GET", `/zones?name=${encodeURIComponent(config.zone)}`);
if (!zone) throw new Error(`zone ${config.zone} not found`);
const zonePath = `/zones/${zone.id}`;
const accountPath = `/accounts/${zone.account.id}`;
console.log(`${mode} ${config.zone}`);

// Zone settings.
for (const [name, value] of Object.entries(config.settings)) {
  const current = await cf("GET", `${zonePath}/settings/${name}`);
  await reconcile(`setting ${name}`, current.value, value, () => cf("PATCH", `${zonePath}/settings/${name}`, { value }));
}

// Tunnel ingress.
const tunnelIds = {};
for (const [name, tunnel] of Object.entries(config.tunnels)) {
  const [found] = await cf("GET", `${accountPath}/cfd_tunnel?name=${encodeURIComponent(name)}&is_deleted=false`);
  if (!found) throw new Error(`tunnel ${name} not found; create it in the dashboard and put its token on the host`);
  tunnelIds[name] = found.id;
  const values = composeEnv(tunnel.env);
  const ingress = tunnel.ingress.map((rule) => ({ ...rule, service: expand(rule.service, values, `tunnel ${name}`) }));
  const current = (await cf("GET", `${accountPath}/cfd_tunnel/${found.id}/configurations`)).config ?? {};
  const keep = (rules) => (rules ?? []).map(({ hostname, path: rulePath, service, originRequest }) =>
    ({ hostname, path: rulePath, service, originRequest: originRequest && Object.keys(originRequest).length ? originRequest : undefined }));
  await reconcile(`tunnel ${name} ingress`, keep(current.ingress), keep(ingress), () =>
    cf("PUT", `${accountPath}/cfd_tunnel/${found.id}/configurations`, { config: { ...current, ingress } }));
}

// DNS records. An address record (A, AAAA, CNAME) is matched by name, so one of
// another address type under that name is replaced. Other types, such as TXT,
// can share a name with each other and with an address record, so they are
// matched by name, type and content.
const records = await cf("GET", `${zonePath}/dns_records?per_page=500`);
const managed = new Set();
const isAddress = (type) => ["A", "AAAA", "CNAME"].includes(type);
for (const record of config.dns) {
  const content = record.tunnel ? `${tunnelIds[record.tunnel] ?? (() => { throw new Error(`${record.name}: unknown tunnel ${record.tunnel}`); })()}.cfargotunnel.com` : record.content;
  const desired = { type: record.type, name: record.name, content, proxied: record.proxied, ttl: record.ttl ?? 1, comment: record.comment };
  const existing = records.filter((r) => r.name === record.name &&
    (isAddress(record.type) ? isAddress(r.type) : r.type === record.type && r.content === content));
  if (existing.length > 1) throw new Error(`${record.name}: ${existing.length} address records; resolve by hand`);
  const [current] = existing;
  if (current) managed.add(current.id);
  const pick = (r) => r && { type: r.type, name: r.name, content: r.content, proxied: r.proxied, ttl: r.ttl, comment: r.comment ?? undefined };
  await reconcile(`dns ${record.name}${isAddress(record.type) ? "" : ` ${record.type}`}`, pick(current), desired, () => current
    ? cf("PUT", `${zonePath}/dns_records/${current.id}`, desired)
    : cf("POST", `${zonePath}/dns_records`, desired));
}
for (const record of records) {
  if (!managed.has(record.id)) {
    console.log(`extra   dns ${record.name} ${record.type} ${record.content} (not in the file; left alone)`);
  }
}

// Zone rulesets, one entrypoint per phase; the file holds the complete rule list.
for (const [phase, rules] of Object.entries(config.rulesets)) {
  let current = [];
  try {
    current = (await cf("GET", `${zonePath}/rulesets/phases/${phase}/entrypoint`)).rules ?? [];
  } catch (error) {
    if (error.status !== 404) throw error;
  }
  const pick = (rule) => ({ description: rule.description, expression: rule.expression, action: rule.action,
    action_parameters: rule.action_parameters, ratelimit: rule.ratelimit, enabled: rule.enabled ?? true });
  await reconcile(`ruleset ${phase}`, current.map(pick), rules.map(pick), () =>
    cf("PUT", `${zonePath}/rulesets/phases/${phase}/entrypoint`, { rules: rules.map(pick) }));
}

if (mode === "plan") {
  console.log(differences ? `${differences} difference(s); run apply to make them` : "Cloudflare matches the file");
  process.exit(differences ? 2 : 0);
}
console.log(differences ? `applied ${differences} change(s)` : "nothing to change");
