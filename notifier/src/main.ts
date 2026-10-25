// zubspace-notifier: polls a Zubspace /v1/players list and sends a message
// when a human player joins or leaves a zone.
//
//   node src/main.ts [--config etc/notifier.json]
import { setTimeout as sleep } from "node:timers/promises";
import { parseArgs } from "node:util";
import { loadConfig } from "./config.ts";
import { createSender } from "./providers.ts";
import { createWatcher, describe } from "./watcher.ts";

const log = (line: string) => console.log(`notifier: ${line}`);

const { values } = parseArgs({
  options: { config: { type: "string", short: "c", default: "etc/notifier.json" } },
});

let loaded: Awaited<ReturnType<typeof loadConfig>>;
try {
  loaded = await loadConfig(values.config);
} catch (error) {
  console.error(`notifier: ${describe(error)}`);
  process.exit(1);
}
const { config, warnings } = loaded;
warnings.forEach(log);

const target = config.provider.kind === "ntfy" ? `ntfy at ${config.provider.server.origin}` : "a Slack webhook";
log(`watching ${config.endpoint.href} every ${config.pollSeconds}s, notifying via ${target}`);

const stop = new AbortController();
for (const signal of ["SIGINT", "SIGTERM"] as const) process.once(signal, () => stop.abort());

const watcher = createWatcher(config, { send: createSender(config.provider), log });
const intervalMs = config.pollSeconds * 1000;
let failures = 0;
while (!stop.signal.aborted) {
  const started = performance.now();
  try {
    await watcher.poll(stop.signal);
    if (failures > 0) log(`player list readable again after ${failures} failed poll(s)`);
    failures = 0;
  } catch (error) {
    if (stop.signal.aborted) break;
    // The first failure and every 30th after it, so an outage cannot flood the log.
    if (failures++ % 30 === 0) log(`cannot read player list: ${describe(error)}`);
  }
  const remaining = Math.max(0, intervalMs - (performance.now() - started));
  await sleep(remaining, undefined, { signal: stop.signal }).catch(() => {});
}
log("stopped");
