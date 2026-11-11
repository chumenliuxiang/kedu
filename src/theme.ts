import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

export type ColorMode = "light" | "dark";
export type ThemePreference = ColorMode | "system";
export const accents = [
  { id: "blue", name: "蓝色", light: "#007aff", dark: "#0a84ff" },
  { id: "indigo", name: "靛蓝", light: "#5856d6", dark: "#6e6cf0" },
  { id: "purple", name: "紫色", light: "#a550d6", dark: "#bf5af2" },
  { id: "pink", name: "粉色", light: "#e8366a", dark: "#ff4f7b" },
  { id: "red", name: "红色", light: "#e5372d", dark: "#ff5246" },
  { id: "orange", name: "橙色", light: "#e07f00", dark: "#ff9f0a" },
  { id: "green", name: "绿色", light: "#2a9d48", dark: "#32d158" },
  { id: "teal", name: "青色", light: "#1a9bb4", dark: "#40c8e0" },
  { id: "graphite", name: "石墨", light: "#6e6e73", dark: "#98989d" },
] as const;
export type AccentId = typeof accents[number]["id"];
export const modes: { id: ThemePreference; name: string }[] = [
  { id: "light", name: "浅色" }, { id: "dark", name: "深色" }, { id: "system", name: "自动" },
];
const background: Record<ColorMode, string> = { light: "#f5f5f7", dark: "#161618" };

const desktop = () => "__TAURI_INTERNALS__" in window;
const darkPalettes = new Set(["ink", "midnight", "plum", "dark"]);
/** Earlier releases stored named palettes; keep the user's light/dark choice. */
function normalizeTheme(value: unknown): ThemePreference {
  if (value === "system") return "system";
  if (typeof value !== "string") return "light";
  return darkPalettes.has(value) ? "dark" : "light";
}
const validAccent = (value: unknown): value is AccentId => accents.some(item => item.id === value);
function stored<T>(key: string, parse: (value: string | null) => T): T {
  try { return parse(localStorage.getItem(key)); } catch { return parse(null); }
}
const system = matchMedia("(prefers-color-scheme: dark)");
const resolve = (choice: ThemePreference): ColorMode => choice === "system" ? (system.matches ? "dark" : "light") : choice;
const initialPreference = stored("kedu.theme", value => value === null ? "light" : normalizeTheme(value));
const initialAccent = stored("kedu.accent", value => validAccent(value) ? value : "blue");

type ThemeState = { preference: ThemePreference; resolved: ColorMode; accent: AccentId; saving: boolean; error: string };
let state: ThemeState = { preference: initialPreference, resolved: resolve(initialPreference), accent: initialAccent, saving: false, error: "" };
let persisted = { preference: state.preference, accent: state.accent };
let revision = 0;
let initialization: Promise<void> | undefined;
let saves = Promise.resolve();
const listeners = new Set<() => void>();
let nativeMode = "";

function apply() {
  const root = document.documentElement;
  root.dataset.theme = state.resolved;
  root.dataset.colorMode = state.resolved;
  root.dataset.accent = state.accent;
  root.style.colorScheme = state.resolved;
  root.style.backgroundColor = background[state.resolved];
  const nextNativeMode = state.preference === "system" ? "system" : state.resolved;
  if (desktop() && nativeMode !== nextNativeMode) {
    nativeMode = nextNativeMode;
    void getCurrentWindow().setTheme(nextNativeMode === "system" ? null : state.resolved).catch(reason => { nativeMode = ""; console.warn("Native window theme unavailable", reason); });
  }
  document.querySelector('meta[name="color-scheme"]')?.setAttribute("content", state.resolved);
  document.querySelector('meta[name="theme-color"]')?.setAttribute("content", background[state.resolved]);
  try { localStorage.setItem("kedu.theme", state.preference); localStorage.setItem("kedu.accent", state.accent); } catch { /* Desktop persistence remains available. */ }
  window.dispatchEvent(new Event("kedu-theme-changed"));
  listeners.forEach(listener => listener());
}

export function initializeTheme(): Promise<void> {
  if (initialization) return initialization;
  apply();
  system.addEventListener("change", () => {
    if (state.preference === "system") { state = { ...state, resolved: resolve("system") }; apply(); }
  });
  window.addEventListener("storage", event => {
    if (desktop() || (event.key !== "kedu.theme" && event.key !== "kedu.accent")) return;
    const preference = stored("kedu.theme", normalizeTheme);
    const accent = stored("kedu.accent", value => validAccent(value) ? value : "blue");
    persisted = { preference, accent }; state = { ...state, preference, accent, resolved: resolve(preference) }; apply();
  });
  initialization = refreshTheme();
  return initialization;
}

export async function refreshTheme() {
  const started = revision;
  await saves;
  if (!desktop()) return;
  try {
    const [theme, accent] = await Promise.all([invoke<string>("get_theme"), invoke<string>("get_accent").catch(() => persisted.accent)]);
    if (started !== revision) return;
    persisted = { preference: normalizeTheme(theme), accent: validAccent(accent) ? accent : "blue" };
    state = { ...state, ...persisted, resolved: resolve(persisted.preference), error: "" }; apply();
  } catch (reason) {
    if (started === revision) { state = { ...state, error: `无法读取外观设置：${String(reason)}` }; listeners.forEach(listener => listener()); }
  }
}

function save(next: Partial<Pick<ThemeState, "preference" | "accent">>) {
  const request = ++revision;
  state = { ...state, ...next, resolved: resolve(next.preference ?? state.preference), saving: true, error: "" }; apply();
  // Serialize saves so rapid choices cannot persist out of order.
  saves = saves.then(async () => {
    try {
      if (desktop() && next.preference) await invoke("set_theme", { theme: next.preference });
      if (desktop() && next.accent) await invoke("set_accent", { accent: next.accent });
      persisted = { ...persisted, ...next };
      if (request === revision) { state = { ...state, saving: false }; apply(); }
    } catch (reason) {
      if (request === revision) {
        state = { ...state, ...persisted, resolved: resolve(persisted.preference), saving: false, error: `外观未保存：${String(reason)}` }; apply();
      }
    }
  });
}
export function chooseTheme(preference: ThemePreference) { if (modes.some(mode => mode.id === preference)) save({ preference }); }
export function chooseAccent(accent: AccentId) { if (validAccent(accent)) save({ accent }); }
export function toggleColorMode() { chooseTheme(state.resolved === "dark" ? "light" : "dark"); }

export function useTheme() {
  return useSyncExternalStore(callback => { listeners.add(callback); return () => { listeners.delete(callback); }; }, () => state);
}

/** Validated categorical order (light, dark). Colour follows the entity, never its rank. */
export const seriesPalette = [
  ["#2a78d6", "#3987e5"], ["#eb6834", "#d95926"], ["#1baf7a", "#199e70"], ["#eda100", "#c98500"],
  ["#e87ba4", "#d55181"], ["#008300", "#008300"], ["#6250d6", "#9085e9"], ["#e34948", "#e66767"],
] as const;
const darkSteps = new Map<string, string>(seriesPalette.map(([light, dark]) => [light, dark]));
/** Identity colours are chosen for light surfaces; use the dark step or lift custom ones. */
export function chartColor(color: string, mode: ColorMode): string {
  if (mode !== "dark" || !/^#[\da-f]{6}$/i.test(color)) return color;
  const step = darkSteps.get(color.toLowerCase());
  if (step) return step;
  const channels = [1, 3, 5].map(offset => Math.round(parseInt(color.slice(offset, offset + 2), 16) * .72 + 255 * .28));
  return `rgb(${channels.join(", ")})`;
}
