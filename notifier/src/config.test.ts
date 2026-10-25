import assert from "node:assert/strict";
import { test } from "node:test";
import { DEFAULT_ENDPOINT, loadConfig, parseConfig } from "./config.ts";

const ntfy = { provider: "ntfy", ntfy: { topic: "zub-test" } };

test("defaults fill everything but the provider", () => {
  const { config, warnings } = parseConfig(ntfy);
  assert.equal(config.endpoint.href, DEFAULT_ENDPOINT);
  assert.equal(config.pollSeconds, 5);
  assert.deepEqual(config.ignoreNames, []);
  assert.deepEqual(config.provider, { kind: "ntfy", server: new URL("https://ntfy.sh"), topic: "zub-test" });
  assert.deepEqual(warnings, []);
});

test("poll_seconds below one second is clamped to one", () => {
  for (const poll of [0.2, 0, -3]) {
    const { config, warnings } = parseConfig({ ...ntfy, poll_seconds: poll });
    assert.equal(config.pollSeconds, 1);
    assert.match(warnings[0] ?? "", /below the 1 second minimum/);
  }
  assert.equal(parseConfig({ ...ntfy, poll_seconds: 1.5 }).config.pollSeconds, 1.5);
  assert.deepEqual(parseConfig({ ...ntfy, poll_seconds: 2 }).warnings, []);
});

test("invalid configs are rejected", () => {
  const cases: [unknown, RegExp][] = [
    [{}, /provider must be "ntfy" or "slack"/],
    [{ ...ntfy, provider: "nfty" }, /provider must be/],
    [{ ...ntfy, poll_second: 2 }, /unknown key "poll_second"/],
    [{ ...ntfy, poll_seconds: "2" }, /poll_seconds must be a number/],
    [{ ...ntfy, endpoint: "ftp://example.com/v1/players" }, /endpoint must be an http\(s\) URL/],
    [{ ...ntfy, ignore_names: ["("] }, /invalid regular expression/],
    [{ provider: "ntfy" }, /ntfy.topic/],
    [{ provider: "ntfy", ntfy: { topic: "has/slash" } }, /ntfy.topic/],
    [{ provider: "slack", slack: { webhook_url: "" } }, /slack.webhook_url/],
    [[], /config must be an object/],
  ];
  for (const [raw, error] of cases) assert.throws(() => parseConfig(raw), error, JSON.stringify(raw));
});

test("only the selected provider's section is read", () => {
  const { config } = parseConfig({ ...ntfy, slack: { webhook_url: "" } });
  assert.equal(config.provider.kind, "ntfy");
  const slack = parseConfig({ provider: "slack", ntfy: {}, slack: { webhook_url: "https://hooks.slack.com/services/T/B/x" } });
  assert.deepEqual(slack.config.provider, { kind: "slack", webhookUrl: new URL("https://hooks.slack.com/services/T/B/x") });
});

test("secrets can come from the environment", () => {
  const env = { NOTIFIER_SLACK_WEBHOOK_URL: "https://hooks.slack.com/services/T/B/env", NOTIFIER_NTFY_TOPIC: "from-env" };
  const slack = parseConfig({ provider: "slack", slack: { webhook_url: "" } }, env).config.provider;
  assert.equal(slack.kind === "slack" && slack.webhookUrl.href, env.NOTIFIER_SLACK_WEBHOOK_URL);
  const topic = parseConfig(ntfy, env).config.provider;
  assert.equal(topic.kind === "ntfy" && topic.topic, "from-env");
});

test("the sample polls every 2 seconds, posts to Slack and ignores deploy probes", async () => {
  const path = new URL("../etc/notifier.sample.json", import.meta.url);
  // The sample carries no webhook; a real config (git-ignored) or the environment supplies it.
  await assert.rejects(loadConfig(path, {}), /NOTIFIER_SLACK_WEBHOOK_URL/);
  const env = { NOTIFIER_SLACK_WEBHOOK_URL: "https://hooks.slack.com/services/T/B/host" };
  const { config, warnings } = await loadConfig(path, env);
  assert.deepEqual(warnings, []);
  assert.equal(config.pollSeconds, 2);
  assert.equal(config.endpoint.href, "https://server.zubspace.gg/v1/players");
  assert.deepEqual(config.provider, { kind: "slack", webhookUrl: new URL(env.NOTIFIER_SLACK_WEBHOOK_URL) });
  const ignored = (name: string) => config.ignoreNames.some((pattern) => pattern.test(name));
  assert.ok(ignored("probe0") && ignored("probe999"));
  assert.ok(!ignored("probert") && !ignored("probe1234") && !ignored("Zub"));
});
