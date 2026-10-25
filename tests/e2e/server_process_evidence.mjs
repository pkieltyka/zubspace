import fs from "node:fs";
import { execFileSync } from "node:child_process";

export function assertServerProcessEvidence(evidence) {
  if (!evidence || evidence.available !== true ||
      evidence.scope !== "server-process-lifetime-including-startup" || evidence.error != null) {
    throw new Error("missing or unavailable server resource evidence");
  }
  for (const key of ["pid", "ticksPerSecond", "rssKiB", "highWaterRssKiB", "threads"]) {
    if (!Number.isSafeInteger(evidence[key]) || evidence[key] <= 0) throw new Error(`invalid resource ${key}`);
  }
  if (!Number.isSafeInteger(evidence.startTicks) || evidence.startTicks < 0 ||
      !Number.isFinite(evidence.lifetimeSeconds) || evidence.lifetimeSeconds <= 0 ||
      !Number.isFinite(evidence.cpuSeconds) || evidence.cpuSeconds < 0 ||
      !Number.isFinite(evidence.cpuPercentOfOneCore) || evidence.cpuPercentOfOneCore < 0 ||
      evidence.highWaterRssKiB < evidence.rssKiB) throw new Error("invalid resource population");
  const expected = evidence.cpuSeconds / evidence.lifetimeSeconds * 100;
  if (Math.abs(expected - evidence.cpuPercentOfOneCore) > Number.EPSILON * 8 * Math.max(1, expected)) {
    throw new Error("resource CPU percentage disagrees with measured lifetime");
  }
  return evidence;
}

// Linux process-lifetime cost, including startup. Not a steady-state workload
// window, peak CPU sample, or evidence of a particular number of moving peers.
export function parseServerProcessEvidence({ pid, stat, status, uptime, ticksPerSecond }) {
  if (!Number.isSafeInteger(pid) || pid <= 0 || !Number.isSafeInteger(ticksPerSecond) || ticksPerSecond <= 0) {
    throw new Error("invalid process identity or clock frequency");
  }
  // comm (field 2) may contain whitespace and parentheses. Numeric fields
  // begin only after the final closing parenthesis, not at whitespace index 2.
  const close = stat.lastIndexOf(")");
  if (!stat.startsWith(`${pid} (`) || close < 0) throw new Error("malformed process stat identity");
  const fields = stat.slice(close + 1).trim().split(/\s+/);
  const integer = (value, label) => {
    if (!/^\d+$/.test(value ?? "") || !Number.isSafeInteger(Number(value))) throw new Error(`invalid ${label}`);
    return Number(value);
  };
  const userTicks = integer(fields[11], "user ticks");
  const systemTicks = integer(fields[12], "system ticks");
  const startTicks = integer(fields[19], "start ticks");
  const uptimeSeconds = Number(uptime.trim().split(/\s+/)[0]);
  const lifetimeSeconds = uptimeSeconds - startTicks / ticksPerSecond;
  if (!Number.isFinite(lifetimeSeconds) || lifetimeSeconds <= 0) throw new Error("invalid process lifetime");
  const statusValue = (name, unit = "") => integer(
    status.match(new RegExp(`^${name}:\\s+(\\d+)${unit}\\s*$`, "m"))?.[1], name,
  );
  const rssKiB = statusValue("VmRSS", "\\s+kB");
  const highWaterRssKiB = statusValue("VmHWM", "\\s+kB");
  const threads = statusValue("Threads");
  if (rssKiB <= 0 || highWaterRssKiB < rssKiB || threads <= 0) throw new Error("invalid resident memory or thread count");
  return assertServerProcessEvidence({
    available: true, scope: "server-process-lifetime-including-startup",
    pid, startTicks, ticksPerSecond, lifetimeSeconds,
    cpuSeconds: (userTicks + systemTicks) / ticksPerSecond,
    cpuPercentOfOneCore: ((userTicks + systemTicks) / ticksPerSecond / lifetimeSeconds) * 100,
    rssKiB, highWaterRssKiB, threads,
  });
}

export function captureServerProcessEvidence(pid) {
  try {
    if (!Number.isSafeInteger(pid) || pid <= 0) throw new Error("server PID unavailable");
    const ticksPerSecond = Number(execFileSync("getconf", ["CLK_TCK"], { encoding: "utf8", timeout: 1000 }).trim());
    const stat = fs.readFileSync(`/proc/${pid}/stat`, "utf8");
    const status = fs.readFileSync(`/proc/${pid}/status`, "utf8");
    const uptime = fs.readFileSync("/proc/uptime", "utf8");
    return parseServerProcessEvidence({ pid, stat, status, uptime, ticksPerSecond });
  } catch (error) {
    // Preserve gameplay failure artifacts even when the server has already
    // exited. Missing measurements must never turn into fabricated zero cost.
    return { available: false, scope: "server-process-lifetime-including-startup", error: error.message };
  }
}

// ChildProcess retains these facts after /proc disappears. Keep the intended
// loopback listener and exit status without copying arbitrary CLI arguments or
// environment variables into artifacts.
export function captureServerLaunchEvidence(child) {
  return {
    pid: child?.pid ?? null,
    listenAddress: child?.spawnargs?.find(arg => /^127\.0\.0\.1:\d+$/.test(arg)) ?? null,
    exitCode: child?.exitCode ?? null,
    signalCode: child?.signalCode ?? null,
  };
}
