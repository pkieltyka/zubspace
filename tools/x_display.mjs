import path from 'node:path';
import { spawnSync } from 'node:child_process';

// DISPLAY being set does not establish that a local X socket is reachable.
// Probe AF_UNIX with Node: bash /dev/tcp is a TCP connector, not a Unix socket.
export function usableXDisplay(value, { socketDirectory = '/tmp/.X11-unix', timeoutMs = 1500 } = {}) {
  const match = typeof value === 'string' && value.match(/^(?:unix\/?)?:(\d+)(?:\.\d+)?$/);
  if (!match) return false;
  const socket = path.join(socketDirectory, `X${match[1]}`);
  return spawnSync(process.execPath, ['--input-type=module', '-e', `
    import net from 'node:net';
    const socket = net.createConnection({ path: process.argv[1] });
    socket.setTimeout(Number(process.argv[2]));
    socket.once('connect', () => { socket.destroy(); process.exit(0); });
    socket.once('error', () => process.exit(1));
    socket.once('timeout', () => process.exit(1));
  `, socket, String(timeoutMs)], { timeout: timeoutMs + 500, stdio: 'ignore' }).status === 0;
}
