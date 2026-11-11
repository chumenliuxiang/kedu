(() => {
  let timer;
  let lastKey = "";
  let lastSentAt = 0;
  let invalidated = false;

  async function send(hidden = false, force = false) {
    if (invalidated) return;
    const page = KeduPageIdentity.identify(location.href);
    if (page.pageKind !== "website") return;
    if (!hidden && document.visibilityState !== "visible") return;
    const now = Date.now();
    // Media events sometimes arrive in bursts. Navigation and hiding always flush.
    if (!hidden && !force && page.pageKey === lastKey && now - lastSentAt < 250) return;
    lastKey = page.pageKey;
    lastSentAt = now;
    try {
      if (hidden) {
        await chrome.runtime.sendMessage({ type: "kedu:hidden", pageKey: lastKey });
      } else {
        const mediaActive = [...document.querySelectorAll("video,audio")].some(media => !media.paused && !media.ended && media.readyState >= 2);
        await chrome.runtime.sendMessage({ type: "kedu:page", pageKey: page.pageKey, title: document.title.slice(0, 500), mediaActive });
      }
    } catch {
      // Reloading an unpacked extension invalidates old content contexts. A page
      // refresh installs the new script; never inject connection UI into websites.
      if (!chrome.runtime?.id) { invalidated = true; clearInterval(timer); }
    }
  }

  function visibility() {
    clearInterval(timer);
    if (document.visibilityState === "visible") {
      void send(false, true);
      timer = setInterval(() => { void send(); }, 5000);
    } else void send(true, true);
  }

  document.addEventListener("visibilitychange", visibility);
  window.addEventListener("pagehide", () => { clearInterval(timer); void send(true, true); });
  window.addEventListener("pageshow", visibility);
  window.addEventListener("focus", () => { void send(false, true); });
  for (const type of ["play", "pause", "ended", "loadeddata"]) document.addEventListener(type, () => { void send(); }, true);
  const title = document.querySelector("title");
  if (title) new MutationObserver(() => { void send(); }).observe(title, { childList: true, subtree: true, characterData: true });
  visibility();
})();
