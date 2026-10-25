// The two notification targets. A message is posted once, retried only for
// network errors, 429 and 5xx responses.
import { setTimeout as sleep } from "node:timers/promises";
import type { Provider } from "./config.ts";

export type Send = (text: string) => Promise<void>;

export interface SenderOptions {
  fetch?: typeof fetch;
  retryDelayMs?: number;
}

const ATTEMPTS = 3;
const TIMEOUT_MS = 10_000;

export function createSender(provider: Provider, options: SenderOptions = {}): Send {
  const { fetch: fetchImpl = fetch, retryDelayMs = 1_000 } = options;

  async function post(url: URL, body: string, contentType: string): Promise<void> {
    for (let attempt = 1; ; attempt++) {
      const failure = await postOnce(fetchImpl, url, body, contentType);
      if (!failure) return;
      if (!failure.retry || attempt === ATTEMPTS) throw failure.error;
      await sleep(retryDelayMs * attempt);
    }
  }

  if (provider.kind === "ntfy") {
    const base = provider.server.href.endsWith("/") ? provider.server.href : `${provider.server.href}/`;
    const url = new URL(provider.topic, base);
    return (text) => post(url, text, "text/plain; charset=utf-8");
  }
  return (text) => post(provider.webhookUrl, JSON.stringify(slackPayload(text)), "application/json");
}

/** Slack renders mrkdwn by default, and player names may contain `*`. */
export function slackPayload(text: string) {
  return { text: text.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;"), mrkdwn: false };
}

async function postOnce(
  fetchImpl: typeof fetch,
  url: URL,
  body: string,
  contentType: string,
): Promise<{ error: Error; retry: boolean } | undefined> {
  let response: Response;
  try {
    response = await fetchImpl(url, {
      method: "POST",
      headers: { "content-type": contentType },
      body,
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
  } catch (error) {
    return { error: error as Error, retry: true };
  }
  const detail = await response.text().catch(() => "");
  if (response.ok) return undefined;
  return {
    error: new Error(`${url.origin} answered HTTP ${response.status} ${detail.slice(0, 200)}`.trim()),
    retry: response.status === 429 || response.status >= 500,
  };
}
