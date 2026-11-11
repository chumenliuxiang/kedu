const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const { webcrypto } = require('node:crypto');

const extension = path.join(__dirname, '..', 'edge-extension');
const source = name => fs.readFileSync(path.join(extension, name), 'utf8');
const plain = value => JSON.parse(JSON.stringify(value));
function event() {
  const handlers = [];
  return { addListener: callback => handlers.push(callback), fire: (...args) => handlers.map(callback => callback(...args)), handlers };
}

function worker(store = {}) {
  let now = 100000;
  const calls = [];
  let fail = false;
  const current = { window: { id: 1, focused: true, type: 'normal' }, tab: { id: 11, windowId: 1, url: 'https://example.com/first?token=private', title: 'First page' } };
  const events = Object.fromEntries(['activated','updated','removed','focus','committed','history','fragment','alarm','installed','startup','message'].map(name => [name,event()]));
  const chrome = {
    storage: { session: { get: async key => ({ [key]: plain(store[key] ?? null) }), set: async data => Object.assign(store, plain(data)) } },
    tabs: { query: async () => current.tab ? [plain(current.tab)] : [], onActivated: events.activated, onUpdated: events.updated, onRemoved: events.removed },
    windows: { WINDOW_ID_NONE: -1, getLastFocused: async () => plain(current.window), onFocusChanged: events.focus },
    webNavigation: { onCommitted: events.committed, onHistoryStateUpdated: events.history, onReferenceFragmentUpdated: events.fragment },
    runtime: { id: 'kedu-test', getURL: value => `chrome-extension://kedu-test/${value}`, onMessage: events.message, onInstalled: events.installed, onStartup: events.startup },
    alarms: { get: async () => ({}), create: async () => {}, onAlarm: events.alarm },
  };
  class Clock extends Date { static now() { return now; } }
  const context = vm.createContext({ URL, chrome, crypto: webcrypto, Date: Clock, AbortSignal, console,
    fetch: async (url, options) => {
      if (fail) throw new Error('offline');
      calls.push({ url, body: options.body ? JSON.parse(options.body) : null });
      return { ok: true, json: async () => ({ accepted: true, bridgeOnline: true, extensionOnline: true, pageConfirmed: true }) };
    },
  });
  context.importScripts = name => vm.runInContext(source(name), context, { filename: name });
  vm.runInContext(source('service-worker.js'), context, { filename: 'service-worker.js' });
  return {
    store, calls, current, events, context,
    tick: (ms = 1000) => { now += ms; }, offline: value => { fail = value; },
    run: code => vm.runInContext(code, context),
    settle: async () => { for (let n=0;n<30;n++) await new Promise(resolve => setImmediate(resolve)); },
    message: (message, sender = { id:'kedu-test', url:'chrome-extension://kedu-test/popup.html' }) => new Promise(resolve => { if (!events.message.handlers[0](message, sender, resolve)) resolve(false); }),
  };
}

test('page identity retains only useful identity, with exact domain boundaries', () => {
  const h = worker();
  const cases = [
    ['https://www.Example.com/a/b?token=secret&utm_source=x#fragment', 'website', 'example.com', 'example.com/a/b'],
    ['https://www.bilibili.com/video/BV1abc/?p=2&spm_id_from=secret', 'website', 'bilibili.com', 'bilibili.com/video/BV1abc?p=2'],
    ['https://www.bilibili.com/video/BV1abc/?p=01', 'website', 'bilibili.com', 'bilibili.com/video/BV1abc'],
    ['https://www.bilibili.com/video/BV1abc/?p=02', 'website', 'bilibili.com', 'bilibili.com/video/BV1abc?p=2'],
    ['https://www.douyin.com/?modal_id=123456&enter_from=tracking', 'website', 'douyin.com', 'douyin.com/video/123456'],
    ['https://www.douyin.com/video/123456', 'website', 'douyin.com', 'douyin.com/video/123456'],
    ['https://evildouyin.com/?modal_id=123', 'website', 'evildouyin.com', 'evildouyin.com/'],
    ['https://youtube.com/watch?v=a_bc-123&list=private', 'website', 'youtube.com', 'youtube.com/watch?v=a_bc-123'],
    ['https://youtu.be/a_bc-123?si=secret', 'website', 'youtu.be', 'youtu.be/a_bc-123'],
    ['edge://settings/', 'internal', '', ''],
    ['file:///C:/private/report.pdf', 'local', '', ''],
    ['data:text/html,sensitive', 'unknown', '', ''],
    ['https://name:password@example.com/private', 'unknown', '', ''],
    ['edge://pdf/?src=https%3A%2F%2Fexample.com%2Ffile.pdf%3Ftoken%3Dsecret', 'website', 'example.com', 'example.com/file.pdf'],
  ];
  for (const [url, pageKind, domain, pageKey] of cases) {
    h.context.input = url;
    assert.deepEqual(plain(h.run('KeduPageIdentity.identify(input)')), { pageKind, domain, pageKey }, url);
  }
});

test('initial active observation has sanitized identity and no complete URL', async () => {
  const h = worker();
  await h.run('observe()');
  const packet = h.calls.at(-1).body;
  assert.equal(packet.pageKey, 'example.com/first');
  assert.equal(packet.domain, 'example.com');
  assert.equal(packet.title, 'First page');
  assert.equal(packet.focused, true);
  assert.equal(packet.sequence, 1);
  assert.equal(JSON.stringify(packet).includes('private'), false);
  assert.equal('url' in packet, false);
  assert.equal('documentId' in packet, false);
  assert.match(h.calls.at(-1).url, /\/v3\/recording\/observation$/);
});

test('switching and focus loss flush short visits without waiting for heartbeat', async () => {
  const h = worker();
  h.events.activated.fire({ tabId: 11 }); await h.settle();
  h.tick(300);
  h.current.tab = { id: 12, windowId: 1, url: 'https://other.test/second', title: 'Second page' };
  h.events.activated.fire({ tabId: 12 }); await h.settle();
  h.tick(200); h.current.window.focused = false;
  h.events.focus.fire(-1); await h.settle();
  assert.deepEqual(h.calls.map(call => [call.body.focused, call.body.pageKey, call.body.observedAt]), [[true,'example.com/first',100000],[true,'other.test/second',100300],[false,'',100500]]);
});

test('background tabs, child frames and stale page identities cannot replace current page', async () => {
  const h = worker();
  await h.run('observe()');
  const count = h.calls.length;
  await h.message({ type: 'kedu:page', pageKey: 'old.test/', title: 'Old' }, { tab: { id: 10, windowId: 1 }, frameId: 0 });
  await h.message({ type: 'kedu:page', pageKey: 'old.test/', title: 'Old' }, { tab: h.current.tab, frameId: 0 });
  assert.equal(await h.message({ type: 'kedu:page' }, { tab: h.current.tab, frameId: 1 }), false);
  assert.equal(h.calls.length, count);
});

test('late hidden event from old tab/document cannot clear a newly active page', async () => {
  const h = worker();
  await h.message({ type: 'kedu:page', pageKey: 'example.com/first', title: 'First' }, { tab: h.current.tab, frameId: 0, documentId: 'old' });
  h.tick();
  await h.message({ type: 'kedu:page', pageKey: 'example.com/first', title: 'Reloaded' }, { tab: h.current.tab, frameId: 0, documentId: 'new' });
  const count = h.calls.length;
  await h.message({ type: 'kedu:hidden', pageKey: 'example.com/first' }, { tab: h.current.tab, frameId: 0, documentId: 'old' });
  assert.equal(h.calls.length, count);
  await h.message({ type: 'kedu:hidden', pageKey: 'example.com/first' }, { tab: h.current.tab, frameId: 0, documentId: 'new' });
  assert.equal(h.calls.at(-1).body.focused, false);
});

test('session identity and monotonic sequence survive worker restart', async () => {
  const store = {};
  const first = worker(store); await first.run('observe()');
  const restarted = worker(store); restarted.tick(); await restarted.run('observe()');
  assert.equal(first.calls[0].body.clientId, restarted.calls[0].body.clientId);
  assert.equal(restarted.calls[0].body.sequence, 2);
  const browserRestart = worker(); await browserRestart.run('observe()');
  assert.notEqual(first.calls[0].body.clientId, browserRestart.calls[0].body.clientId);
});

test('queued old-tab hidden event cannot clear a newer activation waiting in the queue', async () => {
  const h = worker();
  await h.run('observe()');
  h.context.releaseCapture = null;
  const blocker = h.run('enqueue(new Promise(resolve => { globalThis.releaseCapture = resolve; }))');
  h.tick();
  const oldTab = { ...h.current.tab };
  h.current.tab = { id: 12, windowId: 1, url: 'https://new.test/active', title: 'New active' };
  const active = h.run('observe()');
  const hidden = h.message({ type:'kedu:hidden', pageKey:'example.com/first' }, { tab: oldTab, frameId:0, documentId:'old-document' });
  h.run('releaseCapture(null)');
  await blocker; await active; await hidden;
  assert.equal(h.calls.at(-1).body.focused, true);
  assert.equal(h.calls.at(-1).body.pageKey, 'new.test/active');
});

test('content heartbeat keeps media state without changing identity on title changes', async () => {
  const h = worker();
  await h.message({ type: 'kedu:page', pageKey: 'example.com/first', title: 'Renamed title', mediaActive: true }, { tab: h.current.tab, frameId: 0, documentId: 'doc' });
  assert.equal(h.calls.at(-1).body.mediaActive, true);
  assert.equal(h.calls.at(-1).body.title, 'Renamed title');
  h.tick(5000); await h.run('observe()');
  assert.equal(h.calls.at(-1).body.mediaActive, true);
  assert.equal(h.calls.at(-1).body.pageKey, 'example.com/first');
  h.tick(8000); await h.run('observe()');
  assert.equal(h.calls.at(-1).body.mediaActive, false);
});

test('navigation events distinguish internal/local pages without previous website data', async () => {
  const h = worker(); await h.run('observe()');
  h.current.tab.url = 'edge://downloads/'; h.tick(); h.events.committed.fire({ frameId:0, tabId:11 }); await h.settle();
  assert.equal(h.calls.at(-1).body.pageKind, 'internal');
  assert.equal(h.calls.at(-1).body.title, '');
  assert.equal(h.calls.at(-1).body.domain, '');
  h.current.tab.url = 'file:///C:/private/test.pdf'; h.tick(); h.events.updated.fire(11, { url: h.current.tab.url }); await h.settle();
  assert.equal(h.calls.at(-1).body.pageKind, 'local');
  assert.equal(h.calls.at(-1).body.pageKey, '');
});

test('offline bridge remains non-blocking and next observation reconnects', async () => {
  const h = worker(); h.offline(true);
  const result = await h.run('observe()');
  assert.equal(result.recording.bridgeOnline, false);
  assert.equal((await h.message({ type:'kedu:status' })).bridgeOnline, false);
  h.offline(false); h.tick(); await h.run('observe()');
  assert.equal(h.calls.at(-1).body.sequence, 2);
});

test('popup checks recording endpoint; extension contains no restriction behavior', async () => {
  const h = worker(); await h.message({ type:'kedu:status' });
  assert.match(h.calls.at(-1).url, /\/v3\/recording\/status$/);
  assert.equal(await h.message({ type:'kedu:status' }, { id:'kedu-test', url:'https://example.com/', tab:h.current.tab }), false);
  assert.equal(await h.message({ type:'kedu:status' }, { id:'other-extension', url:'chrome-extension://kedu-test/popup.html' }), false);
  assert.equal(await h.message({ type:'exception' }), false);
  assert.equal(await h.message({ type:'focusLeave' }), false);
  const scripts = ['content.js','service-worker.js'].map(source).join('\n');
  assert.equal(/kedu-restriction-root|renderOverlay|focus-leave|v1\/activity/.test(scripts), false);
  assert.equal(/setInterval/.test(source('service-worker.js')), false);
  const manifest = JSON.parse(source('manifest.json'));
  assert.equal(manifest.manifest_version, 3);
  for (const permission of ['tabs','storage','webNavigation','alarms']) assert.ok(manifest.permissions.includes(permission));
  assert.deepEqual(manifest.host_permissions, ['http://127.0.0.1:17361/*']);
});
