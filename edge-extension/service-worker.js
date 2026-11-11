importScripts("identity.js");

const BRIDGE = "http://127.0.0.1:17361/v3/recording";
const STATE_KEY = "keduRecording";
const ALARM = "kedu-recording-check";

async function request(path, body) {
  const response = await fetch(`${BRIDGE}/${path}`, {
    method: body ? "POST" : "GET",
    headers: body ? { "Content-Type": "application/json" } : undefined,
    body: body ? JSON.stringify(body) : undefined,
    cache: "no-store",
    signal: AbortSignal.timeout(1500),
  });
  if (!response.ok) throw new Error(`bridge ${response.status}`);
  return response.json();
}

function inactive(observedAt = Date.now()) {
  return { observedAt, focused: false, pageKind: "unknown", domain: "", title: "", pageKey: "", mediaActive: false };
}

async function state() {
  const saved = (await chrome.storage.session.get(STATE_KEY))[STATE_KEY];
  return saved || { clientId: crypto.randomUUID(), sequence: 0 };
}

async function captureActive(hint = null, observedAt = Date.now()) {
  try {
    const window = await chrome.windows.getLastFocused();
    if (!window.focused || window.type !== "normal") return hint ? null : inactive(observedAt);
    const [tab] = await chrome.tabs.query({ active: true, windowId: window.id });
    if (!tab || (hint && tab.id !== hint.tabId)) return null;
    const identity = KeduPageIdentity.identify(tab.url || "");
    // Never reuse a background document's media or identity after a navigation.
    if (hint?.pageKey !== undefined && identity.pageKey !== hint.pageKey) return null;
    const previous = (await state()).lastObservation;
    const samePage = previous?.tabId === tab.id && previous?.pageKey === identity.pageKey;
    const recentMedia = samePage && observedAt - previous.observedAt <= 7000 && previous.mediaActive;
    return {
      observedAt, focused: true, windowId: window.id, tabId: tab.id, ...identity,
      title: identity.pageKind === "website" ? String(hint?.title ?? tab.title ?? "").slice(0, 500) : "",
      mediaActive: identity.pageKind === "website" && Boolean(hint?.mediaActive ?? recentMedia),
      documentId: hint?.documentId,
    };
  } catch {
    return hint ? null : inactive(observedAt);
  }
}

// This queue only serializes in-flight work. Identity, sequence and last observation
// survive service-worker suspension in storage.session; no keepalive loop is used.
const pending = [];
let draining = false;
function enqueue(capture) {
  return new Promise(resolve => {
    pending.push({ capture, resolve });
    void drain();
  });
}

async function drain() {
  if (draining) return;
  draining = true;
  try {
    while (pending.length) {
      const job = pending.shift();
      try {
        const observation = await job.capture;
        if (!observation || Date.now() - observation.observedAt > 12000) {
          job.resolve({ accepted: false });
          continue;
        }
        const saved = await state();
        // A delayed API callback must not move the observation clock backwards.
        if (observation.observedAt < (saved.lastObservation?.observedAt ?? 0)) {
          job.resolve({ accepted: false });
          continue;
        }
        const { documentId, closing, ...payload } = observation;
        if (closing && (saved.lastObservation?.tabId !== closing.tabId ||
          saved.lastObservation?.pageKey !== closing.pageKey ||
          (closing.documentId && saved.lastDocumentId && closing.documentId !== saved.lastDocumentId))) {
          job.resolve({ accepted: false });
          continue;
        }
        saved.sequence += 1;
        const previous = saved.lastObservation;
        saved.lastDocumentId = documentId ?? (
          previous?.tabId === payload.tabId && previous?.pageKey === payload.pageKey ? saved.lastDocumentId : null
        );
        saved.lastObservation = payload;
        await chrome.storage.session.set({ [STATE_KEY]: saved });
        job.resolve(await request("observation", { clientId: saved.clientId, sequence: saved.sequence, ...payload }));
      } catch {
        job.resolve({ accepted: false, recording: { bridgeOnline: false, state: "disconnected" } });
      }
    }
  } finally {
    draining = false;
  }
}

function observe(hint = null) {
  return enqueue(captureActive(hint));
}

function hiddenObservation(message, sender) {
  return {
    ...inactive(), tabId: sender.tab.id, windowId: sender.tab.windowId,
    closing: { tabId: sender.tab.id, pageKey: message.pageKey, documentId: sender.documentId },
  };
}

async function ensureAlarm() {
  try {
    if (!(await chrome.alarms.get(ALARM))) await chrome.alarms.create(ALARM, { periodInMinutes: 0.5 });
  } catch { /* Browser shutdown can invalidate extension APIs. */ }
}

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (message?.type === "kedu:page" && sender.tab && sender.frameId === 0) {
    void (async () => {
      sendResponse(await observe({
        tabId: sender.tab.id, documentId: sender.documentId,
        pageKey: String(message.pageKey || ""), title: String(message.title || ""),
        mediaActive: Boolean(message.mediaActive),
      }));
    })();
    return true;
  }
  if (message?.type === "kedu:hidden" && sender.tab && sender.frameId === 0) {
    void (async () => { sendResponse(await enqueue(hiddenObservation(message, sender))); })();
    return true;
  }
  if (message?.type === "kedu:status" && sender.id === chrome.runtime.id &&
    typeof sender.url === "string" && sender.url.startsWith(chrome.runtime.getURL(""))) {
    void (async () => {
      try { sendResponse(await request("status")); }
      catch { sendResponse({ bridgeOnline: false, state: "disconnected" }); }
    })();
    return true;
  }
  return false;
});

chrome.tabs.onActivated.addListener(({ tabId }) => { void observe({ tabId }); });
chrome.tabs.onUpdated.addListener((tabId, changes) => {
  if (changes.url !== undefined || changes.title !== undefined || changes.status === "complete") void observe({ tabId });
});
chrome.tabs.onRemoved.addListener(() => { void observe(); });
chrome.windows.onFocusChanged.addListener(windowId => {
  if (windowId === chrome.windows.WINDOW_ID_NONE) void enqueue(Promise.resolve(inactive()));
  else void observe();
});
function navigation(details) {
  if (details.frameId === 0) void observe({ tabId: details.tabId });
}
chrome.webNavigation.onCommitted.addListener(navigation);
chrome.webNavigation.onHistoryStateUpdated.addListener(navigation);
chrome.webNavigation.onReferenceFragmentUpdated.addListener(navigation);
chrome.alarms.onAlarm.addListener(alarm => { if (alarm.name === ALARM) void observe(); });
chrome.runtime.onInstalled.addListener(() => { void ensureAlarm(); void observe(); });
chrome.runtime.onStartup.addListener(() => { void ensureAlarm(); void observe(); });
void ensureAlarm();
