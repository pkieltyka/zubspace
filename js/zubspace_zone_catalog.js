(function installZubspaceZoneCatalog(global) {
  "use strict";

  const maximumZones = 64;
  const maximumZoneIdBytes = 15;
  const maximumDisplayNameBytes = 32;
  const maximumMapFilenameBytes = 15;
  // 64 maximum-size descriptors, including live fields and additive metadata.
  // The independent stream cap still rejects an oversized or endless response.
  const maximumEncodedBytes = 64 * 1024;

  const maximumStreamChunks = maximumEncodedBytes + 1;
  const safeZoneId = /^[a-z0-9](?:[a-z0-9-]{0,13}[a-z0-9])?$/;
  const safeMapFilename = /^[A-Za-z0-9_#@.-]+\.[Ll][Vv][Ll]$/;
  const embeddedZones = Object.freeze([
    Object.freeze({
      id: "chaos-zone",
      display_name: "Chaos Zone",
      map: "bdhadar.lvl",
    }),
    Object.freeze({
      id: "extreme",
      display_name: "Extreme",
      map: "doomsy8.lvl",
    }),
  ]);

  function object(value) {
    return value !== null && typeof value === "object" && !Array.isArray(value);
  }

  function count(value, maximum = maximumZones * 500) {
    return Number.isSafeInteger(value) && value >= 0 && value <= maximum;
  }

  function asciiByteLength(value) {
    if (typeof value !== "string" || !/^[\x20-\x7e]+$/.test(value)) {
      return null;
    }
    return value.length;
  }

  function normalizedIdentity(value) {
    return value.toLowerCase().replace(/[^a-z0-9]/g, "");
  }

  function validateCatalog(parsed) {
    if (!object(parsed) || !count(parsed.total_players)) {
      throw new TypeError("zone catalog envelope has an invalid schema");
    }
    if (!Array.isArray(parsed.zones) || parsed.zones.length > maximumZones) {
      throw new RangeError(`zone catalog may contain at most ${maximumZones} zones`);
    }

    const identities = new Set();
    const zones = parsed.zones.map((zone) => {
      if (!object(zone) || typeof zone.status !== "string" ||
          !/^[a-z][a-z0-9_-]{0,31}$/.test(zone.status) ||
          typeof zone.default !== "boolean" || typeof zone.listed !== "boolean" ||
          typeof zone.free_for_all !== "boolean" || typeof zone.maxed_spawn_loadout !== "boolean" ||
          !count(zone.max_players, 500) || zone.max_players === 0 ||
          !count(zone.total_players, zone.max_players) || (zone.status === "standby" && zone.total_players !== 0)) {
        throw new TypeError("zone descriptor has an invalid schema");
      }
      const idBytes = asciiByteLength(zone.id);
      const displayBytes = asciiByteLength(zone.display_name);
      const mapBytes = asciiByteLength(zone.map);
      if (
        idBytes === null ||
        idBytes > maximumZoneIdBytes ||
        !safeZoneId.test(zone.id)
      ) {
        throw new TypeError("zone descriptor has an invalid id");
      }
      if (displayBytes === null || displayBytes > maximumDisplayNameBytes) {
        throw new TypeError("zone descriptor has an invalid display_name");
      }
      if (
        mapBytes === null ||
        mapBytes > maximumMapFilenameBytes ||
        zone.map.includes("..") ||
        !safeMapFilename.test(zone.map)
      ) {
        throw new TypeError("zone descriptor has an unsafe map asset name");
      }

      const localIdentities = new Set([
        normalizedIdentity(zone.id),
        normalizedIdentity(zone.display_name),
      ]);
      for (const identity of localIdentities) {
        if (!identity || identities.has(identity)) {
          throw new TypeError("zone catalog contains a duplicate normalized identity");
        }
        identities.add(identity);
      }
      return Object.freeze({
        id: zone.id,
        display_name: zone.display_name,
        map: zone.map,
        status: zone.status,
        default: zone.default,
        listed: zone.listed,
        total_players: zone.total_players,
        max_players: zone.max_players,
        free_for_all: zone.free_for_all,
        maxed_spawn_loadout: zone.maxed_spawn_loadout,
      });
    });
    if (zones.reduce((sum, zone) => sum + zone.total_players, 0) !== parsed.total_players) {
      throw new TypeError("zone catalog total disagrees with its zone counts");
    }
    return Object.freeze(zones);
  }

  function resolveArenaSelection(
    zones,
    { queryArena = "", storedArena = "", fallbackArena = "chaos-zone" } = {},
  ) {
    if (!Array.isArray(zones) || zones.length === 0) {
      throw new TypeError("arena selection requires a nonempty validated catalog");
    }
    const ids = new Set();
    const aliases = new Map();
    for (const zone of zones) {
      if (!zone || typeof zone.id !== "string" || typeof zone.display_name !== "string") {
        throw new TypeError("arena selection requires validated zone descriptors");
      }
      ids.add(zone.id);
      aliases.set(normalizedIdentity(zone.id), zone.id);
      aliases.set(normalizedIdentity(zone.display_name), zone.id);
    }
    const catalogIdentity = (value) => {
      if (typeof value !== "string") return null;
      const identity = normalizedIdentity(value.trim());
      return identity ? aliases.get(identity) || null : null;
    };

    // A URL arena is an initial hint only. Once this returns, the selected
    // catalog id in window.__zubspaceArena is the sole live connection value.
    return (
      catalogIdentity(queryArena) ||
      (ids.has(storedArena) ? storedArena : null) ||
      catalogIdentity(fallbackArena) ||
      zones[0].id
    );
  }

  async function cancelQuietly(reader, reason) {
    try {
      await reader.cancel(reason);
    } catch {
      // Preserve the primary size, stream, UTF-8, or schema failure.
    }
  }

  function advertisedContentLength(response) {
    const raw = response.headers?.get?.("content-length");
    if (raw === null || raw === undefined) {
      return null;
    }
    if (typeof raw !== "string" || !/^[0-9]+$/.test(raw)) {
      throw new TypeError("zone catalog has a malformed or duplicate Content-Length");
    }
    const parsed = Number(raw);
    if (!Number.isSafeInteger(parsed)) {
      throw new RangeError("zone catalog Content-Length is outside the safe range");
    }
    return parsed;
  }

  async function readResponse(response) {
    const reader = response?.body?.getReader?.();
    if (!reader) {
      throw new TypeError("zone catalog response is not a readable byte stream");
    }
    let complete = false;
    try {
      if (response.ok !== true) {
        throw new Error("zone catalog request failed");
      }
      const contentLength = advertisedContentLength(response);
      if (contentLength !== null && contentLength > maximumEncodedBytes) {
        throw new RangeError(
          `zone catalog Content-Length exceeds the ${maximumEncodedBytes}-byte encoded limit`,
        );
      }
      const encoded = new Uint8Array(maximumEncodedBytes);
      let length = 0;
      let chunks = 0;
      for (;;) {
        const result = await reader.read();
        if (!result || typeof result.done !== "boolean") {
          throw new TypeError("zone catalog stream returned an invalid read result");
        }
        if (result.done) {
          complete = true;
          break;
        }
        if (!(result.value instanceof Uint8Array)) {
          throw new TypeError("zone catalog stream returned a non-byte chunk");
        }
        chunks += 1;
        if (chunks > maximumStreamChunks) {
          throw new RangeError("zone catalog stream contains too many chunks");
        }
        const nextLength = length + result.value.byteLength;
        if (!Number.isSafeInteger(nextLength) || nextLength > maximumEncodedBytes) {
          throw new RangeError(
            `zone catalog exceeds the ${maximumEncodedBytes}-byte encoded limit`,
          );
        }
        encoded.set(result.value, length);
        length = nextLength;
      }
      const text = new TextDecoder("utf-8", { fatal: true }).decode(
        encoded.subarray(0, length),
      );
      return validateCatalog(JSON.parse(text));
    } finally {
      if (!complete) {
        await cancelQuietly(reader, "invalid zone catalog response");
      } else {
        reader.releaseLock?.();
      }
    }
  }

  async function resolveResponse(response) {
    try {
      const zones = await readResponse(response);
      return Object.freeze({ source: "server", total: zones.reduce((sum, zone) => sum + zone.total_players, 0), zones });
    } catch {
      return Object.freeze({ source: "embedded", total: null, zones: embeddedZones });
    }
  }

  global.zubspaceZoneCatalogLimits = Object.freeze({
    maximumZones,
    maximumZoneIdBytes,
    maximumDisplayNameBytes,
    maximumMapFilenameBytes,
    maximumEncodedBytes,
    maximumStreamChunks,
  });
  global.zubspaceEmbeddedZoneCatalog = embeddedZones;
  global.zubspaceValidateZoneCatalog = validateCatalog;
  global.zubspaceResolveArenaSelection = resolveArenaSelection;
  global.zubspaceReadZoneCatalogResponse = readResponse;
  global.zubspaceResolveZoneCatalogResponse = resolveResponse;
})(typeof window === "undefined" ? globalThis : window);
