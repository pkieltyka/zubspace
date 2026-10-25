// Reads etc/notifier.json. Only the selected provider's section is used, and
// its secret (the ntfy topic or Slack webhook URL) may come from the
// environment instead, so a committed config never has to carry it.
import { readFile } from "node:fs/promises";

export const DEFAULT_ENDPOINT = "https://server.zubspace.gg/v1/players";
export const DEFAULT_POLL_SECONDS = 5;
export const MIN_POLL_SECONDS = 1;
export const DEFAULT_NTFY_SERVER = "https://ntfy.sh";

export type Provider =
  | { kind: "ntfy"; server: URL; topic: string }
  | { kind: "slack"; webhookUrl: URL };

export interface Config {
  endpoint: URL;
  pollSeconds: number;
  ignoreNames: RegExp[];
  provider: Provider;
}

export type Env = Record<string, string | undefined>;

export async function loadConfig(path: string | URL, env: Env = process.env) {
  let raw: unknown;
  try {
    raw = JSON.parse(await readFile(path, "utf8"));
  } catch (error) {
    throw new Error(`cannot read config ${path}: ${(error as Error).message}`);
  }
  return parseConfig(raw, env);
}

/** Validates a parsed config; `warnings` describes values that were adjusted. */
export function parseConfig(raw: unknown, env: Env = {}): { config: Config; warnings: string[] } {
  const warnings: string[] = [];
  const top = object(raw, "config", ["endpoint", "poll_seconds", "ignore_names", "provider", "ntfy", "slack"]);

  const endpoint = httpUrl(top.endpoint ?? DEFAULT_ENDPOINT, "endpoint");

  const poll = top.poll_seconds ?? DEFAULT_POLL_SECONDS;
  if (typeof poll !== "number" || !Number.isFinite(poll)) throw new Error("poll_seconds must be a number");
  const pollSeconds = Math.max(poll, MIN_POLL_SECONDS);
  if (pollSeconds !== poll) {
    warnings.push(`poll_seconds ${poll} is below the ${MIN_POLL_SECONDS} second minimum; using ${pollSeconds}`);
  }

  const patterns = top.ignore_names ?? [];
  if (!Array.isArray(patterns) || !patterns.every((pattern) => typeof pattern === "string")) {
    throw new Error("ignore_names must be an array of regular expressions");
  }
  const ignoreNames = patterns.map((pattern) => {
    try {
      return new RegExp(pattern);
    } catch {
      throw new Error(`ignore_names: invalid regular expression ${JSON.stringify(pattern)}`);
    }
  });

  let provider: Provider;
  switch (top.provider) {
    case "ntfy": {
      const section = object(top.ntfy ?? {}, "ntfy", ["server", "topic"]);
      const topic = env.NOTIFIER_NTFY_TOPIC || section.topic;
      if (typeof topic !== "string" || !/^[-_A-Za-z0-9]{1,64}$/.test(topic)) {
        throw new Error("ntfy.topic (or NOTIFIER_NTFY_TOPIC) must be 1-64 letters, digits, - or _");
      }
      provider = { kind: "ntfy", server: httpUrl(section.server ?? DEFAULT_NTFY_SERVER, "ntfy.server"), topic };
      break;
    }
    case "slack": {
      const section = object(top.slack ?? {}, "slack", ["webhook_url"]);
      const webhookUrl = httpUrl(
        env.NOTIFIER_SLACK_WEBHOOK_URL || section.webhook_url,
        "slack.webhook_url (or NOTIFIER_SLACK_WEBHOOK_URL)",
      );
      provider = { kind: "slack", webhookUrl };
      break;
    }
    default:
      throw new Error('provider must be "ntfy" or "slack"');
  }

  return { config: { endpoint, pollSeconds, ignoreNames, provider }, warnings };
}

function object(value: unknown, name: string, keys: readonly string[]): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) throw new Error(`${name} must be an object`);
  for (const key of Object.keys(value)) {
    if (!keys.includes(key)) throw new Error(`${name}: unknown key "${key}"`);
  }
  return value as Record<string, unknown>;
}

function httpUrl(value: unknown, name: string): URL {
  const url = typeof value === "string" ? URL.parse(value) : null;
  if (!url || (url.protocol !== "http:" && url.protocol !== "https:")) throw new Error(`${name} must be an http(s) URL`);
  return url;
}
