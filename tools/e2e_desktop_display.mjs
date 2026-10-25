import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import { usableXDisplay } from './x_display.mjs';

async function acceptsLocalSetup(socketPath, timeoutMs = 1500) {
  return new Promise(resolve => {
    const socket = net.createConnection({ path: socketPath });
    let header = Buffer.alloc(0), done = false;
    const finish = accepted => {
      if (done) return;
      done = true;
      socket.destroy();
      resolve(accepted);
    };
    socket.setTimeout(timeoutMs);
    socket.once('connect', () => {
      const setup = Buffer.alloc(12);
      setup[0] = 108; // Little endian, X11 protocol 11, no authentication bytes.
      setup.writeUInt16LE(11, 2);
      socket.write(setup);
    });
    socket.on('data', chunk => {
      header = Buffer.concat([header, chunk]).subarray(0, 8);
      if (header.length === 8) finish(header[0] === 1 && header.readUInt16LE(2) === 11);
    });
    socket.once('error', () => finish(false));
    socket.once('end', () => finish(false));
    socket.once('timeout', () => finish(false));
  });
}

async function abstractSocketOccupied(socketPath) {
  if (process.platform !== 'linux') return false;
  return new Promise(resolve => {
    const socket = net.createConnection({ path: `\0${socketPath}` });
    let done = false;
    const finish = occupied => { if (done) return; done = true; socket.destroy(); resolve(occupied); };
    socket.setTimeout(150);
    socket.once('connect', () => finish(true));
    socket.once('error', () => finish(false));
    socket.once('timeout', () => finish(true)); // An ambiguous live endpoint is reserved.
  });
}

// Reuse a desktop display without owning or stopping its X server. Some
// Xwayland sessions retain a live X<n>_ socket after X<n> is unavailable. A
// task-owned symlink can address that same server without replacing X<n>.
// This fallback requires a same-user socket accepting local unauthenticated
// setup; it never reads, copies or changes desktop authentication cookies.
export async function createDesktopXDisplay({ display = process.env.DISPLAY,
  socketDirectory = '/tmp/.X11-unix' } = {}) {
  const match = typeof display === 'string' && display.match(/^(?:unix\/?)?:(\d+)(?:\.\d+)?$/);
  if (!match) throw new Error('Native GPU QA requires a local desktop DISPLAY');
  const normal = path.join(socketDirectory, `X${match[1]}`);
  if (usableXDisplay(display, { socketDirectory })) {
    return { display, evidence: { kind: 'desktop', serverProcessOwned: false }, close() {} };
  }
  const alternate = `${normal}_`;
  const stat = fs.lstatSync(alternate, { throwIfNoEntry: false });
  if (!stat?.isSocket() || stat.uid !== process.getuid?.() || !await acceptsLocalSetup(alternate)) {
    throw new Error('Native GPU QA requires a reachable desktop X server; no usable local display was found');
  }
  for (let number = 180; number < 1000; number++) {
    const alias = path.join(socketDirectory, `X${number}`);
    if (fs.existsSync(path.join(path.dirname(socketDirectory), `.X${number}-lock`)) ||
        await abstractSocketOccupied(alias)) continue;
    try { fs.symlinkSync(alternate, alias); }
    catch (error) { if (error.code === 'EEXIST') continue; throw error; }
    const inode = fs.lstatSync(alias).ino;
    const close = () => {
      process.off('exit', close);
      const current = fs.lstatSync(alias, { throwIfNoEntry: false });
      if (current?.isSymbolicLink() && current.ino === inode && fs.readlinkSync(alias) === alternate) {
        fs.unlinkSync(alias);
      }
    };
    process.once('exit', close);
    return { display: `:${number}`, close,
      evidence: { kind: 'desktop-unix-alias', requestedDisplay: display,
        sourceSocket: alternate, aliasSocket: alias, serverProcessOwned: false } };
  }
  throw new Error('No free task-owned desktop display alias');
}
