// Run with bun. Real terminal and host processes; private checkout, data and fixture provider.
import assert from 'node:assert/strict';
import { copyFileSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { CHOICE_GUARD_MS } from '../packages/corral-pi/src/ui/build/choice-dock.ts';
import { connect } from '../packages/corral-pi/test/support/bridge-client.ts';
import { scriptedFixtureInstall } from '../packages/corral-pi/test/support/fixture-install.ts';

const root = mkdtempSync(join(tmpdir(), 'corral-code-switch-'));
const data = join(root, 'data'), cwd = join(root, 'work'), id = 'B-first1';
let host;
try {
  mkdirSync(cwd);
  const assets = scriptedFixtureInstall(root, [{ text: 'Unique fixture response.' }]);
  let screen = '';
  host = Bun.spawn([process.execPath, join(assets, 'src/cli.ts'), 'code', '--model', 'openai-codex/gpt-6.2-sol', '--id', id, '--data-dir', data], {
    cwd, env: { TERM: 'xterm-256color', PATH: process.env.PATH, HOME: root, TMPDIR: root },
    terminal: { rows: 40, cols: 132, data: (_, bytes) => { screen = (screen + new TextDecoder().decode(bytes)).slice(-200000); } },
  });
  const wait = async (predicate, reason) => {
    const end = Date.now() + 12000;
    while (Date.now() < end && host.exitCode === null) {
      if (await predicate()) return;
      await Bun.sleep(30);
    }
    throw new Error(`Did not observe ${reason}. Last terminal output: ${Bun.stripANSI(screen).trim().slice(-1800)}`);
  };
  const record = key => JSON.parse(readFileSync(join(data, 'build', key, 'session.json'), 'utf8'));
  const ids = () => readdirSync(join(data, 'build'));
  const command = async text => {
    screen = '';
    if (text) {
      host.terminal.write(text);
      await wait(() => screen.includes(text), `editor to display ${text}`);
    } else {
      // The owner card deliberately ignores the Enter that opened it.
      await Bun.sleep(CHOICE_GUARD_MS + 1);
    }
    host.terminal.write('\r');
  };
  // A record is reserved before the terminal has an editor. Wait for that specific host's
  // attached bridge, not an earlier process's last paint on the same terminal.
  const ready = async key => {
    let client;
    try {
      const { socket, sessionFile } = record(key);
      if (!sessionFile) return false;
      client = await connect(socket);
      return (await client.request({ version: 2, id: 'ready', command: 'get_state', args: {} })).ok;
    } catch { return false; }
    finally { client?.close(); }
  };
  await wait(() => ready(id), 'initial editor');
  const original = record(id);
  await command('Remember the first conversation.');
  await wait(() => screen.includes('Unique fixture response.'), 'first reply');
  await command('/new');
  await wait(() => ids().length === 2 && ready(ids().find(key => key !== id)),  'new editor and persisted second Code record');
  const fresh = record(ids().find(key => key !== id));
  assert.notEqual(original.pid, fresh.pid);
  assert.notEqual(original.sessionFile, fresh.sessionFile);
  assert(!readFileSync(fresh.sessionFile, 'utf8').includes('Remember the first conversation.'));
  await command('/resume');
  await wait(() => screen.includes('Resume a stopped Code session') && screen.includes(id), 'resume picker');
  await command('');
  await wait(() => record(id).pid !== original.pid && screen.includes('Remember the first conversation.') && ready(id), 'restored conversation');
  assert.equal(record(id).sessionFile, original.sessionFile);
  assert.equal(ids().length, 2);
  await command('/fork');
  await wait(() => screen.includes('Fork conversation before this message'), 'fork picker');
  await command('');
  await wait(() => ids().length === 3 && screen.includes('Remember the first conversation.') && ready(ids().find(key => key !== id && key !== fresh.id)),  'new fork with selected draft');
  // Remove the fork draft, then import the original transcript rather than switching its host.
  host.terminal.write('\x15');
  copyFileSync(original.sessionFile, join(cwd, 'history.jsonl'));
  await command('/import history.jsonl');
  await wait(() => screen.includes('Import into a new Code session?'), 'import confirmation');
  await command('');
  await wait(() => ids().length === 4 && screen.includes('Unique fixture response.'), 'imported session');
  await command('/quit');
  assert.equal(await Promise.race([host.exited, Bun.sleep(5000).then(() => 'timeout')]), 0);
  console.log('PASS: terminal /new, /resume picker, /fork draft, /import and /quit.');
} finally {
  if (host && host.exitCode === null) { host.kill('SIGTERM'); await host.exited; }
  host?.terminal?.close();
  rmSync(root, { recursive: true, force: true });
}
