import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import net from 'node:net';
import test from 'node:test';
import { createDesktopXDisplay } from '../tools/e2e_desktop_display.mjs';

async function fixture(name, accepted, run) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'zubspace-desktop-display-'));
  const server = net.createServer(socket => {
    socket.once('data', () => {
      const header = Buffer.alloc(8); header[0] = accepted ? 1 : 0; header.writeUInt16LE(11, 2);
      socket.end(header);
    });
    socket.on('error', () => {});
  });
  await new Promise(resolve => server.listen(path.join(dir, name), resolve));
  try { await run(dir, server); }
  finally { await new Promise(resolve => server.close(resolve)); fs.rmSync(dir, { recursive: true, force: true }); }
}

test('an ordinary display is reused without owning or deleting its server socket', async () => {
  await fixture('X7', true, async dir => {
    const display = await createDesktopXDisplay({ display: ':7', socketDirectory: dir });
    assert.equal(display.display, ':7');
    assert.equal(display.evidence.serverProcessOwned, false);
    display.close();
    assert.ok(fs.lstatSync(path.join(dir, 'X7')).isSocket());
  });
});

test('alternate Xwayland socket gets only a task-owned alias with idempotent cleanup', async () => {
  await fixture('X7_', true, async dir => {
    fs.writeFileSync(path.join(dir, 'X180'), 'reserved');
    const display = await createDesktopXDisplay({ display: ':7', socketDirectory: dir });
    const alias = display.evidence.aliasSocket;
    assert.equal(fs.readlinkSync(alias), path.join(dir, 'X7_'));
    assert.equal(fs.existsSync(path.join(dir, 'X7')), false);
    display.close(); display.close();
    assert.equal(fs.existsSync(alias), false);
    assert.equal(fs.readFileSync(path.join(dir, 'X180'), 'utf8'), 'reserved');
    assert.ok(fs.lstatSync(path.join(dir, 'X7_')).isSocket());
  });
});

test('cleanup leaves a replaced alias alone', async () => {
  await fixture('X7_', true, async dir => {
    const display = await createDesktopXDisplay({ display: ':7', socketDirectory: dir });
    const alias = display.evidence.aliasSocket;
    fs.unlinkSync(alias); fs.writeFileSync(alias, 'replacement');
    display.close();
    assert.equal(fs.readFileSync(alias, 'utf8'), 'replacement');
  });
});

test('missing or authentication-requiring alternate displays fail closed', async () => {
  await fixture('X7_', false, async dir => {
    await assert.rejects(createDesktopXDisplay({ display: ':7', socketDirectory: dir }), /reachable desktop/);
    assert.deepEqual(fs.readdirSync(dir), ['X7_']);
    await assert.rejects(createDesktopXDisplay({ display: ':8', socketDirectory: dir }), /reachable desktop/);
    await assert.rejects(createDesktopXDisplay({ display: 'remote:7', socketDirectory: dir }), /local desktop/);
  });
});
