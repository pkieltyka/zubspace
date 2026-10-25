import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";

export function sourceRevision(repoRoot, env = process.env) {
  const supplied = env.ZUBSPACE_BUILD_COMMIT || null;
  const valid = value => /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/.test(value);
  if (supplied && !valid(supplied)) throw new Error("ZUBSPACE_BUILD_COMMIT must be a full lowercase hexadecimal Git commit");
  let checkout = null;
  try {
    checkout = execFileSync("git", ["rev-parse", "--verify", "HEAD"],
      { cwd: repoRoot, env, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
  } catch { /* Source archives and container contexts may not include Git metadata. */ }
  if (checkout && !valid(checkout)) throw new Error("Git returned an invalid source revision");
  if (checkout && supplied && checkout.toLowerCase() !== supplied.toLowerCase())
    throw new Error("ZUBSPACE_BUILD_COMMIT does not match the checked-out source");
  const revision = supplied || checkout;
  if (!revision && env.ZUBSPACE_REQUIRE_BUILD_REVISION === "1")
    throw new Error("Container builds require ZUBSPACE_BUILD_COMMIT; use make docker-build or supply the full source revision");
  return revision?.toLowerCase() || "nogit";
}

// Stamp the HTML before serving/packaging so the footer needs no extra request.
export function clientBuildInfo(repoRoot, env = process.env) {
  const cargo = fs.readFileSync(path.join(repoRoot, "Cargo.toml"), "utf8");
  const appVersion = cargo.match(/^version\s*=\s*"([0-9A-Za-z.+-]+)"/m)?.[1];
  if (!appVersion) throw new Error("Cargo package version is missing");
  return { appVersion, commit: sourceRevision(repoRoot, env).slice(0, 8) };
}

export function stampClientBuildInfo(html, { appVersion, commit }) {
  return html.replace('id="menu-build">Development build', `id="menu-build">v${appVersion} · ${commit}`);
}
