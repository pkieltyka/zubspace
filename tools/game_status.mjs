// A QA-only view assembled from the public Part A API. This is never a server
// response contract. Keep process counters and zone diagnostics available to
// existing evidence readers without adding legacy fields to the public API.
const safeZoneId = /^[a-z0-9](?:[a-z0-9-]{0,13}[a-z0-9])?$/;
const count = value => Number.isSafeInteger(value) && value >= 0;

export async function fetchGameStatusResponse(input, init = {}) {
  const url = new URL(input);
  if (url.pathname !== "/v1/status" || url.search || url.hash || url.username || url.password) {
    throw new Error("QA status reader requires an exact process status endpoint");
  }
  const options = { ...init, cache: "no-store", signal: init.signal ?? AbortSignal.timeout(5000) };
  const endpoint = pathname => { const target = new URL(url); target.pathname = pathname; return target; };
  // These endpoints are independent observations, not an atomic cross-request
  // snapshot. Never require their totals to match during a join/departure.
  const [processResponse, catalogResponse, rosterResponse] = await Promise.all([
    fetch(url, options), fetch(endpoint("/v1/zones"), options), fetch(endpoint("/v1/players"), options),
  ]);
  if (!processResponse.ok) return processResponse;
  if (!catalogResponse.ok || !rosterResponse.ok) {
    throw new Error(`QA status discovery/roster HTTP ${catalogResponse.status}/${rosterResponse.status}`);
  }
  const [processStatus, catalog, roster] = await Promise.all([
    processResponse.json(), catalogResponse.json(), rosterResponse.json(),
  ]);
  if (processStatus.ok !== true || processStatus.service !== "zubspace-server" ||
      !count(processStatus.total_players) || processStatus.zone !== undefined ||
      !count(catalog.total_players) || !Array.isArray(catalog.zones) ||
      !count(roster.total_players) || !Array.isArray(roster.players) || roster.total_players !== roster.players.length) {
    throw new Error("QA status requires the Part A process, catalog and roster schemas");
  }
  const defaults = catalog.zones.filter(zone => zone.default === true && zone.status === "open");
  if (defaults.length !== 1) throw new Error("QA status requires one active default zone before Part B");
  const summary = defaults[0];
  if (typeof summary.id !== "string" || !safeZoneId.test(summary.id) || !count(summary.total_players) ||
      typeof summary.free_for_all !== "boolean" || typeof summary.maxed_spawn_loadout !== "boolean") {
    throw new Error("QA status has an invalid zone summary");
  }
  const ids = new Set();
  for (const player of roster.players) {
    if (!count(player.id) || ids.has(player.id) || typeof player.name !== "string" ||
        typeof player.zone !== "string" || !safeZoneId.test(player.zone) || typeof player.bot !== "boolean") {
      throw new Error("QA status has an invalid public player");
    }
    ids.add(player.id);
  }
  const zonePath = `/v1/zones/${summary.id}/status`;
  const zoneResponse = await fetch(endpoint(zonePath), options);
  if (!zoneResponse.ok) throw new Error(`QA zone status HTTP ${zoneResponse.status}`);
  const simulation = await zoneResponse.json();
  if (simulation.ok !== true || simulation.zone !== summary.id ||
      simulation.status !== "open" || !count(simulation.total) || !count(simulation.snapshot_acked_clients)) {
    throw new Error("QA status has an invalid open-zone simulation payload");
  }
  const view = {
    ...processStatus, ...simulation,
    // Total remains the process total; scoped membership has its own name.
    total: processStatus.total_players,
    zoneTotal: simulation.total,
    joinedPlayers: roster.players.filter(player => !player.bot).length,
    botPlayers: roster.players.filter(player => player.bot).length,
    free_for_all: summary.free_for_all,
    maxed_spawn_loadout: summary.maxed_spawn_loadout,
    qaStatusSource: { process: "/v1/status", catalog: "/v1/zones", roster: "/v1/players", simulation: zonePath },
  };
  return Response.json(view);
}
