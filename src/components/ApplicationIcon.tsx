import { memo, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import * as api from "../api";
import type { ApplicationIconMap } from "../types";

const CACHE_LIMIT = 256;
const cache = new Map<string, { value: string | null; expiresAt: number }>();
const pending = new Map<string, Promise<string | null>>();
const queued = new Map<string, (value: string | null) => void>();
let flushScheduled = false;
const cachedIcons = (targets: string[]): ApplicationIconMap => Object.fromEntries(targets.flatMap(target => {
  const entry = cache.get(target);
  return entry && entry.expiresAt > Date.now() ? [[target, entry.value]] : [];
}));
const sameIcons = (left: ApplicationIconMap, right: ApplicationIconMap) => Object.keys(left).length === Object.keys(right).length && Object.keys(right).every(key => left[key] === right[key]);

async function flushIcons() {
  const entries = [...queued];
  queued.clear();
  flushScheduled = false;
  // The desktop command accepts at most 64 targets per batch. Coalescing
  // several pickers must not silently turn targets after the 64th into misses.
  for (let offset = 0; offset < entries.length; offset += 64) {
    const batch = entries.slice(offset, offset + 64);
    let values: ApplicationIconMap = {};
    try { values = await api.getApplicationIcons(batch.map(([target]) => target)); }
    catch { /* A failed batch keeps the text fallback and can be retried later. */ }
    for (const [target, resolve] of batch) {
      const value = values[target] ?? null;
      cache.delete(target);
      cache.set(target, { value, expiresAt: Date.now() + (value ? 10 * 60_000 : 30_000) });
      while (cache.size > CACHE_LIMIT) cache.delete(cache.keys().next().value!);
      pending.delete(target);
      resolve(value);
    }
  }
}

async function loadIcons(targets: string[]): Promise<ApplicationIconMap> {
  const result = cachedIcons(targets);
  const missing = targets.filter(target => !(target in result));
  for (const target of missing) {
    if (!pending.has(target)) pending.set(target, new Promise(resolve => queued.set(target, resolve)));
  }
  if (queued.size && !flushScheduled) {
    flushScheduled = true;
    queueMicrotask(() => { void flushIcons(); });
  }
  await Promise.all(missing.map(async target => { result[target] = await pending.get(target) ?? null; }));
  return result;
}

export function useApplicationIcons(targets: string[]): ApplicationIconMap {
  const signature = [...new Set(targets.map(target => target.trim()).filter(Boolean))].sort().join("\u0000");
  const [icons, setIcons] = useState<ApplicationIconMap>(() => cachedIcons(signature ? signature.split("\u0000") : []));
  useEffect(() => {
    let active = true;
    const requested = signature ? signature.split("\u0000") : [];
    const update = (next: ApplicationIconMap) => { if (active) setIcons(current => sameIcons(current, next) ? current : next); };
    update(cachedIcons(requested));
    void loadIcons(requested).then(update);
    const refresh = async (invalidate = false) => {
      if (!active) return;
      if (invalidate) {
        await Promise.all(requested.map(target => pending.get(target)));
        if (!active) return;
        for (const target of requested) cache.delete(target);
      }
      if (!document.hidden) void loadIcons(requested).then(update);
    };
    const onVisible = () => { void refresh(); };
    window.addEventListener("focus", onVisible);
    document.addEventListener("visibilitychange", onVisible);
    const subscription = api.isDesktop() ? listen("application-icons-changed", () => { void refresh(true); }).catch(() => () => {}) : null;
    return () => {
      active = false;
      window.removeEventListener("focus", onVisible);
      document.removeEventListener("visibilitychange", onVisible);
      void subscription?.then(unlisten => unlisten()).catch(() => {});
    };
  }, [signature]);
  return icons;
}

type ApplicationIconProps = { name: string; target?: string; icons: ApplicationIconMap; className?: string };
const sourceFor = ({ name, target, icons }: ApplicationIconProps) => icons[(target || name).trim()];
export const ApplicationIcon = memo(function ApplicationIcon({ name, target, icons, className = "" }: ApplicationIconProps) {
  const source = icons[(target || name).trim()];
  const [failedSource, setFailedSource] = useState<string | null>(null);
  const available = source && source !== failedSource;
  return <span className={`application-icon ${available ? "has-image" : "is-fallback"} ${className}`}>{available ? <img src={source} alt="" decoding="async" onError={() => setFailedSource(source)} /> : <span aria-hidden="true">{name.trim().slice(0, 1).toUpperCase() || "?"}</span>}</span>;
}, (previous, next) => previous.name === next.name && previous.target === next.target && previous.className === next.className && sourceFor(previous) === sourceFor(next));
