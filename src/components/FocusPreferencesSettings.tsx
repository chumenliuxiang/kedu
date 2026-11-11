import { useEffect, useState } from "react";
import * as api from "../api";
import type { FocusPreferences } from "../types";

type ToggleKey = "notificationsEnabled" | "soundEnabled" | "scheduleStartNotificationsEnabled" | "scheduleStartSoundEnabled" | "scheduleEndNotificationsEnabled" | "scheduleEndSoundEnabled";
const toggles: { key: ToggleKey; label: string }[] = [
  { key: "notificationsEnabled", label: "专注结束时系统通知" }, { key: "soundEnabled", label: "专注结束时提示音" },
  { key: "scheduleStartNotificationsEnabled", label: "日程开始时系统通知" }, { key: "scheduleStartSoundEnabled", label: "日程开始时提示音" },
  { key: "scheduleEndNotificationsEnabled", label: "日程结束时系统通知" }, { key: "scheduleEndSoundEnabled", label: "日程结束时提示音" },
];

export function FocusPreferencesSettings({ onSaved }: { onSaved: () => Promise<void> }) {
  const [value, setValue] = useState<FocusPreferences | null>(null);
  const [initial, setInitial] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  useEffect(() => { let active = true; void api.getFocusPreferences().then(preferences => { if (active) { setValue(preferences); setInitial(JSON.stringify(preferences)); } }).catch(reason => { if (active) setError(String(reason)); }); return () => { active = false; }; }, []);
  const update = (patch: Partial<FocusPreferences>) => { setValue(current => current && { ...current, ...patch }); setSaved(false); setError(""); };
  if (!value) return error ? <p className="fp-error st-row" role="alert">{error}</p> : <p className="st-row" role="status">正在读取设置…</p>;
  const dirty = JSON.stringify(value) !== initial;
  return <form className="focus-preferences" data-testid="focus-preferences" onSubmit={async event => {
    event.preventDefault(); if (busy) return; setBusy(true); setError(""); setSaved(false);
    try { const next = await api.saveFocusPreferences(value); setValue(next); setInitial(JSON.stringify(next)); await onSaved(); setSaved(true); }
    catch (reason) { setError(String(reason)); }
    finally { setBusy(false); }
  }}>
    <fieldset disabled={busy}><legend className="visually-hidden">专注设置</legend>
      <div className="st-row"><div className="st-row-copy"><span>窗口拦截方式</span><small>{value.windowGuardMode === "minimize" ? "最小化受限窗口并前置提醒；部分应用还原后可能缩在屏幕左上角。" : "前置刻度提醒，保留原窗口状态；约束力度较弱。"}</small></div><div className="st-row-control"><select aria-label="窗口拦截方式" value={value.windowGuardMode} onChange={event => update({ windowGuardMode: event.target.value as FocusPreferences["windowGuardMode"] })}><option value="reminder">前置提醒</option><option value="minimize">强制最小化</option></select></div></div>
      <div className="st-row"><div className="st-row-copy"><span>每轮累计暂停</span><small>普通模式下可分次使用；设为 0 关闭暂停。</small></div><div className="st-row-control"><span className="st-number"><input name="pauseMinutes" aria-label="每轮累计暂停（分钟）" type="number" min="0" max="60" step="1" required value={Number.isNaN(value.pauseMinutes) ? "" : value.pauseMinutes} onChange={event => update({ pauseMinutes: event.target.valueAsNumber })}/>分钟</span></div></div>
      <div className="st-row"><div className="st-row-copy"><span>完成后休息</span><small>设为 0 时直接完成，不进入休息。</small></div><div className="st-row-control"><span className="st-number"><input name="restMinutes" aria-label="完成后休息（分钟）" type="number" min="0" max="60" step="1" required value={Number.isNaN(value.restMinutes) ? "" : value.restMinutes} onChange={event => update({ restMinutes: event.target.valueAsNumber })}/>分钟</span></div></div>
      {toggles.map(item => <div className="st-row" key={item.key}><div className="st-row-copy"><span>{item.label}</span></div><div className="st-row-control"><label className="switch"><input name={item.key} type="checkbox" aria-label={item.label} checked={value[item.key]} onChange={event => update({ [item.key]: event.target.checked } as Partial<FocusPreferences>)}/><i/></label></div></div>)}
    </fieldset>
    {error && <p className="fp-error st-row" role="alert">{error}</p>}
    <div className="st-row st-actions"><span role="status" className="st-saved">{saved ? "已保存" : ""}</span><button type="submit" className="primary" disabled={busy || !dirty}>{busy ? "保存中…" : dirty ? "保存专注设置" : "已保存"}</button></div>
  </form>;
}
