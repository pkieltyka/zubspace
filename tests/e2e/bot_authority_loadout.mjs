import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
// Maximum entry is an intentional project override, including proximity.
// Keep reference direct-bomb coverage as a separately declared fixture.
export function botAuthorityLoadout(env, arena) {
  if (!["chaos-zone", "extreme"].includes(arena)) throw new Error("unknown bot-authority arena");
  const profile = env.ZUBSPACE_E2E_SPAWN_LOADOUT || "maximum";
  if (!["maximum", "reference"].includes(profile)) throw new Error("unknown bot-authority spawn loadout");
  if (profile === "reference" && Number(env.ZUBSPACE_E2E_JOIN_MIN_UPTIME_SECONDS || 0) > 0) {
    throw new Error("aged-server authority requires production maximum entry");
  }
  const maxedSpawnLoadout = profile === "maximum";
  return { profile, maxedSpawnLoadout,
    bombWeaponType: maxedSpawnLoadout || arena === "chaos-zone" ? 4 : 3 };
}

export function botBombSafetyProfile(fixture, arenaConfig) {
  let section = '', safety = null, proximity = null, explosion = null;
  for (const raw of arenaConfig.split(/\r?\n/)) {
    const line = raw.split(/[;#]/, 1)[0].trim();
    const header = line.match(/^\[([^\]]+)\]$/);
    if (header) section = header[1];
    else if (section === 'Bomb') {
      const match = line.match(/^(BombSafety|ProximityDistance|BombExplodePixels)=(.*)$/);
      if (!match) continue;
      if (!/^\d+$/.test(match[2].trim())) throw new Error(`invalid authored ${match[1]}`);
      if (match[1] === 'BombSafety') {
        if (safety !== null) throw new Error('ambiguous authored BombSafety');
        safety = Number(match[2].trim());
      } else if (match[1] === 'ProximityDistance') {
        if (proximity !== null) throw new Error('ambiguous authored ProximityDistance');
        proximity = Number(match[2].trim());
      } else {
        if (explosion !== null) throw new Error('ambiguous authored BombExplodePixels');
        explosion = Number(match[2].trim());
      }
    }
  }
  if (safety === null || proximity === null || explosion === null) throw new Error('missing authored bomb safety settings');
  return {enabled: fixture.bombWeaponType === 4 && safety !== 0, proximityDistance: proximity,
    explosionPixels: explosion};
}

export function botBombSafetyFloor(profile, bombLevel) {
  if (!Number.isInteger(bombLevel) || bombLevel < 0 || bombLevel > 4)
    throw new Error('missing or invalid live bomb level for safety staging');
  if (!profile.enabled || bombLevel === 0) return 0;
  // Fire-only tile radius, not the larger contact sensor. Four pixels are
  // ordinary setup headroom; no gameplay safety rule is bypassed.
  return (profile.proximityDistance + bombLevel - 1) * 16 + 4;
}

// These drivers select Warbird for the Extreme bomb legs. Read its actual
// authored loadout instead of treating maximum-entry proximity as direct fire.
export function stationaryWarbirdBombLaneMinimum({
  fixture, arenaConfig, expectedExitSpeed, rotationPerSecond,
}) {
  if (!Number.isFinite(expectedExitSpeed) || expectedExitSpeed < 0 ||
      !Number.isFinite(rotationPerSecond) || rotationPerSecond <= 0) {
    throw new Error("invalid stationary bomb crossing geometry");
  }
  const setting = (section, key) => {
    let current = "";
    const values = [];
    for (const raw of arenaConfig.split(/\r?\n/)) {
      const line = raw.split(/[;#]/, 1)[0].trim();
      const header = line.match(/^\[([^\]]+)\]$/);
      if (header) current = header[1];
      else if (current === section && line.startsWith(`${key}=`)) {
        values.push(line.slice(key.length + 1).trim());
      }
    }
    if (values.length !== 1 || !/^\d+$/.test(values[0])) {
      throw new Error(`missing or ambiguous authored ${section}:${key}`);
    }
    return Number(values[0]);
  };
  let safetyRadius = 0;
  if (fixture.bombWeaponType === 4 && setting("Bomb", "BombSafety") !== 0) {
    const level = setting("Warbird", fixture.maxedSpawnLoadout ? "MaxBombs" : "InitialBombs");
    if (level < 1 || level > 4) throw new Error("invalid authored Warbird bomb level");
    // Match the corrected shared fire-only radius (the reference game's tile units).
    // The expanded 18px contact sensor remains a distinct collision rule.
    safetyRadius = (setting("Bomb", "ProximityDistance") + level - 1) * 16;
  }
  const alignedTurnSeconds = Math.acos(0.9) / (Math.PI * 2) / rotationPerSecond;
  // Preserve the original turn, legal-crossing and sampling margins. A
  // proximity shot must also remain outside BombSafety when Z is pressed;
  // otherwise held Z can fire only after the pilot has passed the target.
  return Math.ceil(expectedExitSpeed * alignedTurnSeconds) +
    12 + Math.max(18, safetyRadius) + 16;
}

export async function assertBotAuthorityLoadout(serverPort, fixture, initialZone) {
  const response = await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`, {
    signal: AbortSignal.timeout(10_000),
  });
  if (!response.ok) throw new Error(`bot-authority fixture status HTTP ${response.status}`);
  const status = await response.json();
  if (status.maxed_spawn_loadout !== fixture.maxedSpawnLoadout || status.zone !== initialZone) {
    throw new Error(`bot-authority loadout/zone mismatch: ${JSON.stringify({ fixture, initialZone,
      observedMaxedSpawnLoadout: status.maxed_spawn_loadout, observedZone: status.zone })}`);
  }
  return { ...fixture, observedMaxedSpawnLoadout: status.maxed_spawn_loadout };
}
