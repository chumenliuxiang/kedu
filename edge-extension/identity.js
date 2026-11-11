/* Shared, pure URL classification. Raw URLs never leave this function. */
globalThis.KeduPageIdentity = (() => {
  const empty = pageKind => ({ pageKind, domain: "", pageKey: "" });
  const belongsTo = (host, domain) => host === domain || host.endsWith(`.${domain}`);
  const identifier = value => value && /^[A-Za-z0-9_-]{1,160}$/.test(value) ? value : "";

  function identify(raw, depth = 0) {
    let url;
    try { url = new URL(raw); } catch { return empty("unknown"); }
    if (url.protocol === "file:") return empty("local");
    // Some PDF viewers expose their source URL, others remain unconfirmed.
    const pdfViewer = (["edge:", "chrome:"].includes(url.protocol) && url.hostname === "pdf") ||
      (url.protocol === "chrome-extension:" && url.hostname === "mhjfbmdgcfjbbpaeojofohoefgiehjai");
    if (pdfViewer && depth < 1) {
      const source = url.searchParams.get("src") || url.searchParams.get("file");
      if (source) return identify(source, depth + 1);
    }
    if (["edge:", "chrome:", "about:", "chrome-extension:"].includes(url.protocol)) return empty("internal");
    if (!["http:", "https:"].includes(url.protocol) || url.username || url.password) return empty("unknown");
    const domain = url.hostname.toLowerCase().replace(/^www\./, "").replace(/\.$/, "");
    if (!domain) return empty("unknown");
    const path = url.pathname.length <= 2048 ? url.pathname : "/";
    let pageKey = domain + path;
    if (belongsTo(domain, "douyin.com")) {
      const videoId = identifier(url.searchParams.get("modal_id")) || /^\/video\/(\d+)\/?$/.exec(path)?.[1];
      if (videoId) pageKey = `${domain}/video/${videoId}`;
    } else if (belongsTo(domain, "bilibili.com") && path.startsWith("/video/")) {
      pageKey = domain + path.replace(/\/$/, "");
      const part = url.searchParams.get("p");
      const partNumber = part && /^\d+$/.test(part) ? Number(part) : 0;
      if (Number.isInteger(partNumber) && partNumber > 1 && partNumber <= 4294967295) pageKey += `?p=${partNumber}`;
    } else if (belongsTo(domain, "youtube.com") && path === "/watch") {
      const videoId = identifier(url.searchParams.get("v"));
      if (videoId) pageKey += `?v=${videoId}`;
    }
    return { pageKind: "website", domain, pageKey };
  }
  return { identify };
})();
