import { useEffect, useState, type RefObject } from "react";
import * as api from "../api";
import type { LimitRule, NewLimitRule, TargetCatalog } from "../types";
import { TargetPicker } from "./TargetPicker";

export function parseReminderMinutes(value: string, limit: number): number[] {
  if (!value.trim()) return [];
  const tokens = value.trim().split(/[,，;；\s]+/);
  if (tokens.length > 32 || tokens.some(token => !/^\d+$/.test(token) || Number(token) < 1 || Number(token) >= limit)) throw new Error("请填写小于每日额度的正整数，最多 32 项");
  return [...new Set(tokens.map(Number))].sort((a, b) => b - a);
}

export function RuleDialog({ dialogRef, editing, catalog, onSave }: { dialogRef: RefObject<HTMLDialogElement | null>; editing: LimitRule | null; catalog: TargetCatalog; onSave: (rule: NewLimitRule, reminders: number[]) => Promise<void> }) {
  const [kind, setKind] = useState<NewLimitRule["targetKind"]>("application"); const [target, setTarget] = useState(""); const [name, setName] = useState(""); const [group, setGroup] = useState(""); const [minutes, setMinutes] = useState(30);
  const [reminders, setReminders] = useState("5, 1"); const [savedReminders, setSavedReminders] = useState("5, 1"); const [customReminders, setCustomReminders] = useState(false); const [error, setError] = useState(""); const [busy, setBusy] = useState(false); const [loading, setLoading] = useState(false); const [loadFailed, setLoadFailed] = useState(false);
  const reset = () => { setKind(editing?.targetKind ?? "application"); setTarget(editing?.targetPattern ?? ""); setName(editing?.name ?? ""); setGroup(editing?.groupName ?? ""); setMinutes(editing?.dailyLimitMinutes ?? 30); setError(""); setCustomReminders(Boolean(editing)); setReminders(editing ? savedReminders : "5, 1"); };
  useEffect(() => {
    reset(); setLoadFailed(false); let cancelled = false;
    if (editing) { setLoading(true); api.getLimitReminders(editing.id).then(value => { if (!cancelled) { setReminders(value.join(", ")); setSavedReminders(value.join(", ")); } }).catch(reason => { if (!cancelled) { setLoadFailed(true); setError(String(reason)); } }).finally(() => { if (!cancelled) setLoading(false); }); } else setLoading(false);
    return () => { cancelled = true; };
  }, [editing]);
  const submit = async (event: React.SubmitEvent<HTMLFormElement>) => { event.preventDefault(); if (busy || loading || loadFailed) return; setError(""); try { const points = parseReminderMinutes(reminders, minutes); setBusy(true); await onSave({ name: name.trim(), targetKind: kind, targetPattern: target.trim(), groupName: group.trim(), dailyLimitMinutes: minutes }, points); } catch (reason) { setError(reason instanceof Error ? reason.message : String(reason)); } finally { setBusy(false); } };
  const changeMinutes = (value: number) => { setMinutes(value); if (!customReminders) setReminders([5, 1].filter(point => point < value).join(", ")); };
  return <dialog ref={dialogRef} className="modal target-modal lw-editor lw-rule-editor" aria-label={editing ? "编辑限制" : "新增每日限制"} onClose={reset}><form onSubmit={submit}>
    <header><h2>{editing ? "编辑限制" : "新增每日限制"}</h2><button type="button" className="icon-button" aria-label="关闭" onClick={() => dialogRef.current?.close()}>×</button></header>
    <fieldset className="lw-editor-section"><legend>限制目标</legend>
      <div className="lw-editor-target">
        <label>目标类型<select aria-label="目标类型" value={kind} onChange={event => { setKind(event.target.value as typeof kind); setTarget(""); setName(""); }}><option value="application">应用</option>{editing?.targetKind === "website" && <option value="website">网站</option>}<option value="group">限制组</option></select></label>
        <div className="form-field"><span>选择目标</span><TargetPicker kind={kind} value={target} catalog={catalog} onChange={(value, displayName) => { setTarget(value); setName(displayName); }}/></div>
      </div>
    </fieldset>
    <fieldset className="lw-editor-section"><legend>每天的额度</legend>
      <div className="lw-editor-grid">
        <div><label>每日额度（分钟）<input aria-label="每日额度（分钟）" type="number" min="1" max="1440" step="1" value={minutes} onChange={event => changeMinutes(Number(event.target.value))} required/></label><div className="lw-minute-presets" role="group" aria-label="快捷额度">{[15, 30, 60].map(value => <button key={value} type="button" aria-pressed={minutes === value} onClick={() => changeMinutes(value)}>{value} 分钟</button>)}</div></div>
        <div><label>提前提醒（剩余分钟）<input aria-label="提前提醒（剩余分钟）" value={reminders} disabled={loading} aria-describedby="reminder-help" aria-invalid={Boolean(error)} onChange={event => { setCustomReminders(true); setReminders(event.target.value); }} placeholder="例如 10, 5, 1"/></label><p id="reminder-help" className="reminder-help">多个时间用逗号分隔，留空关闭提醒</p></div>
      </div>
    </fieldset>
    <fieldset className="lw-editor-section"><legend>名称与分组</legend>
      <div className="lw-editor-grid">
        <label>名称<input aria-label="名称" value={name} onChange={event => setName(event.target.value)} required/></label>
        <label><span>所属限制组<span className="lw-editor-optional">选填</span></span><input aria-label="所属限制组" list="known-limit-groups" value={group} onChange={event => setGroup(event.target.value)} placeholder="多个组用分号分隔"/><datalist id="known-limit-groups">{catalog.groups.map(item => <option key={item} value={item}/>)}</datalist></label>
      </div>
    </fieldset>
    {error && <p role="alert" className="form-error">{error}</p>}
    <footer><button type="button" className="secondary" onClick={() => dialogRef.current?.close()}>取消</button><button type="submit" className="primary" disabled={!target.trim() || !name.trim() || busy || loading || loadFailed}>{busy ? "保存中…" : "保存规则"}</button></footer>
  </form></dialog>;
}
