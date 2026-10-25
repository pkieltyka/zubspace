import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import net from 'node:net';
import test from 'node:test';
import { usableXDisplay } from '../tools/x_display.mjs';

test('a real local Unix display listener is reachable; missing/stale paths are not', async () => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'zubspace-x-display-'));
  const server = net.createServer(socket => socket.destroy());
  try {
    await new Promise(resolve => server.listen(path.join(dir, 'X7'), resolve));
    assert.equal(usableXDisplay(':7', { socketDirectory: dir }), true);
    assert.equal(usableXDisplay('unix/:7.0', { socketDirectory: dir }), true);
    assert.equal(usableXDisplay(':8', { socketDirectory: dir }), false);
    await fs.writeFile(path.join(dir, 'X9'), 'not a socket');
    assert.equal(usableXDisplay(':9', { socketDirectory: dir }), false);
    for (const display of ['', undefined, 'remote:7', ':7;false']) {
      assert.equal(usableXDisplay(display, { socketDirectory: dir }), false);
    }
  } finally {
    await new Promise(resolve => server.close(resolve));
    await fs.rm(dir, { recursive: true, force: true });
  }
});
