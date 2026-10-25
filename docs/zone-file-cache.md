# Zone file cache

A zone's map and LVZ archives can add up to several megabytes, and the server
normally sends them over the reliable DataChannel on every join. The browser
avoids most of that work in two ways:

- **Warmup over HTTP.** While the player is still in the menu, the page
  fetches the selected zone's files from the static bundle.
- **A persistent cache.** Files that passed verification on an earlier join
  are kept in IndexedDB.

Neither source is trusted on its own. After login the server advertises every
file of the zone with its CRC and SHA-256, and the client uses local bytes only
when both match and the bytes pass the same validation as a download. It then
tells the server which files it already holds, and the server sends only the
rest.

## Warmup

`js/zubspace_zone_loading.js` reads the generated asset manifest
(`zone-assets.json` in a release bundle), which lists each zone's files and
their SHA-256 digests. For the selected zone only, it queues every file through
one pool of six concurrent requests, map and settings first, and keeps the
bytes in memory. Each file is limited to 1,048,559 bytes and the zone to
32 MiB. On desktop it also fetches the WASM module so compilation can start as
soon as the player presses Play.

If the cache already holds a file with the manifest's digest, the warmup uses
it instead of fetching. The WASM runtime's own file loads for that zone share
the same in-flight requests. Any warmup failure is harmless: the client falls
back to the normal transfer from the server.

## Persistent cache

`js/zubspace_zone_cache.js` keeps an IndexedDB database
`zubspace.zone-files.v1` with two object stores, `metadata` and `data`
(the bytes as an `ArrayBuffer`), always written in one transaction.

- **Identity.** An entry's key combines the offer endpoint (the server the
  page connects to) with `v2.<zone id>.<sha256>.<file name>`. A file is reused
  only for the same server, zone, name and content.
- **Scope.** Maps and LVZ archives only; settings files are ordinary HTTP
  resources.
- **Limits.** At most 64 MiB and 512 entries in total, 1,048,559 bytes per
  entry, and 30 days since last use. When a write would exceed a limit, the
  least recently used entries are evicted, across all endpoints. A quota error
  evicts the oldest entry and retries once.
- **Loading.** On page load the entries for the current endpoint are read
  into memory, discarding any that are malformed, expired or over budget, so
  the WASM client can read them synchronously.
- **Failure.** Missing IndexedDB, a blocked upgrade, corruption, quota
  exhaustion, or a database open over one second or a transaction over two
  seconds all leave the cache unavailable or the entry unwritten; the game
  still loads normally. No transaction is held open across a Promise or a WASM
  frame.

The WASM side reaches the cache through the imports `zub_map_cache_len`,
`zub_map_cache_read`, `zub_map_cache_write` and `zub_map_cache_remove`
(`src/transport.rs`); despite the names they handle LVZ files as well. A read
first looks in the cache, then in the bytes the warmup fetched for that zone.

## Join sequence

1. The server advertises the zone's files. For each one the client builds the
   cache identity from the advertised zone, digest and file name, and looks
   for local bytes.
2. Local bytes must match the advertised CRC and SHA-256, and LVZ archives
   must pass the same structure, decoded-size, render-budget and full image
   checks as a download. A mismatch removes the cache entry and the file is
   requested instead.
3. For a verified map, the client sends the cached-map check (`0x7c` version
   1). For a verified LVZ, it sends a held-file claim (`0x7c` version 2):

   | Bytes | Field |
   | --- | --- |
   | 0 | `0x7c` |
   | 1 | Version `2` |
   | 2–17 | Zone id, NUL-padded |
   | 18–49 | SHA-256 of the active map |
   | 50–51 | File index, little-endian u16 |
   | 52–83 | SHA-256 of the file |

4. Missing files are requested and transferred as usual.
5. Once every file is resolved and the map is installed, the client writes the
   newly verified files to the cache. If validation fails before that point,
   nothing from this join is written.

Held-file claims for LVZ archives are used only with servers that support
them: the server appends the six bytes `ZCAP 01 01` to its login response, and
without that trailer the client requests every LVZ file. Older clients ignore
the trailer.

The server accepts a claim only from the current joined session, for the
current zone and map, with a matching file digest. Repeated claims are
harmless, and a claim cannot cancel a transfer already in progress. A held map
still goes through the normal map security check; claiming a digest does not
prove the client has the file.

## Server-side counting

| Counter | Meaning |
| --- | --- |
| `zubspace_zone_files_held_total` | Files claimed as held, once per file per join |
| `zubspace_zone_bytes_held_total` | Bytes those claims saved |
| `zubspace_zone_files_sent_total` | Completed file transfers, including repeats |
| `zubspace_zone_bytes_sent_total` | Bytes in completed transfers, including repeats |

Bytes are the compressed map and the raw LVZ archives, without framing; failed
or partial transfers are not counted. Each join also logs
`server zone_file_manifest_resolved` with the held and sent file counts and
bytes, the number of completed transfers and the elapsed time. That marks the
point where every file was resolved, not the end of the security check or
readiness to play. See [observability](observability.md).

## Tests

`make test-browser-zone-cache` exercises real IndexedDB storage, quota
recovery and eviction in a headless browser, without a server.
`make test-e2e-zone-cache-join` joins the real server and checks HTTP reuse,
a reload and a browser restart with a clean process. Tests that need a cold
transfer set `window.__zubspaceZoneCacheReadDisabledForTest = true`, which
makes every cache read a miss.
