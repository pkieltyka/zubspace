// Local test listener overrides; the shipped Compose topology stays unchanged.
// Read only the explicit integer in our checked-in [server] table. Fail closed
// on missing/duplicate/unsupported values instead of guessing a capacity.
export function composeServerMaxPlayers(config) {
  let inServer = false, tables = 0;
  const values = [];
  for (const line of config.split(/\r?\n/)) {
    const table = /^\s*\[([^\]]+)\]\s*(?:#.*)?$/.exec(line);
    if (table) {
      inServer = table[1].trim() === "server";
      if (inServer) tables += 1;
    } else if (inServer && /^\s*max_players\s*=/.test(line)) {
      const scalar = /^\s*max_players\s*=\s*([0-9](?:_?[0-9])*)\s*(?:#.*)?$/.exec(line);
      if (!scalar) throw new Error("[server].max_players must be an explicit integer");
      values.push(Number(scalar[1].replaceAll("_", "")));
    }
  }
  if (tables !== 1 || values.length !== 1)
    throw new Error("Compose requires exactly one [server].max_players setting");
  validateMaxPlayers(values[0]);
  return values[0];
}

function validateMaxPlayers(maxPlayers) {
  if (!Number.isSafeInteger(maxPlayers) || maxPlayers < 1 || maxPlayers > 768)
    throw new Error("max_players must be an integer from 1 to 768");
}

export function composeRtcTopology(env = process.env, { maxPlayers = 500 } = {}) {
  validateMaxPlayers(maxPlayers);
  const names = ["ZUBSPACE_COMPOSE_CLIENT_PORT", "ZUBSPACE_COMPOSE_SERVER_PORT", "ZUBSPACE_COMPOSE_UDP_PORT_RANGE"];
  const supplied = names.filter(name => env[name] !== undefined);
  const isolated = supplied.length > 0;
  if (isolated && supplied.length !== names.length) throw new Error(`isolated Compose ports require all of ${names.join(", ")}`);
  const port = (value, name) => {
    if (!/^[1-9][0-9]{0,4}$/.test(value) || Number(value) > 65535)
      throw new Error(`${name} must be a port from 1 to 65535`);
    return Number(value);
  };
  const clientPort = isolated ? port(env[names[0]], names[0]) : 7000;
  const serverPort = isolated ? port(env[names[1]], names[1]) : 9001;
  if (clientPort === serverPort) throw new Error("Compose client and server TCP ports must differ");
  const range = /^([1-9][0-9]{0,4})-([1-9][0-9]{0,4})$/.exec(isolated ? env[names[2]] : "9002-9501");
  if (!range) throw new Error(`${names[2]} must be an inclusive UDP port range`);
  const first = port(range[1], names[2]), last = port(range[2], names[2]);
  if (last < first || last - first + 1 < maxPlayers)
    throw new Error(`${names[2]} must provide at least ${maxPlayers} UDP ports for max_players=${maxPlayers}`);
  const udpPortRange = `${first}-${last}`;
  if (!isolated) return { clientPort, serverPort, udpPortRange, override: null };
  return { clientPort, serverPort, udpPortRange, override: { services: {
    "zubspace-server": {
      environment: { ZUBSPACE_SERVER_BIND: `127.0.0.1:${serverPort}`, ZUBSPACE_SERVER_UDP_PORT_RANGE: udpPortRange },
      // A scalar replaces the original command on providers that concatenate
      // list-valued overrides. All interpolated ports are validated integers.
      healthcheck: { test: `curl --fail --silent --show-error http://127.0.0.1:${serverPort}/v1/status` },
    },
    "zubspace-client": { environment: {
      ZUBSPACE_SERVER_UPSTREAM: `127.0.0.1:${serverPort}`, ZUBSPACE_CLIENT_LISTEN_PORT: String(clientPort),
    } },
  } } };
}
