import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import * as api from "./api";
import type { FocusAppearance, FocusState, FocusTask, FocusTaskInput, FocusWhitelistInput, FocusWorkspaceData, ScheduleImport, ScheduleImportItem, ScheduleItem, TargetCatalog } from "./types";
import { ApplicationIcon, useApplicationIcons } from "./components/ApplicationIcon";
import { TargetPicker } from "./components/TargetPicker";
import { usePolledQuery } from "./hooks/usePolledQuery";
import { FocusStatistics } from "./FocusStatistics";
import { createScheduleExample, ScheduleImportGuide } from "./ScheduleImportGuide";
import { Icon } from "./components/Icon";
import { PageHeader } from "./components/ui";
import "./FocusWorkspace.css";

const dateKey = () => { const d = new Date(); return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`; };
export const emptyFocusWorkspace = (): FocusWorkspaceData => ({ tasks: [], session: null, whitelist: [], appearance: { style: "normal", size: "normal", autoImmerse: false }, preferences: { ...api.defaultFocusPreferences } });
const secondsText = (milliseconds: number, remaining = true) => { const seconds = Math.max(0, remaining ? Math.ceil(milliseconds / 1000) : Math.floor(milliseconds / 1000)); const hours = Math.floor(seconds / 3600); return `${hours ? `${String(hours).padStart(2, "0")}:` : ""}${String(Math.floor(seconds / 60) % 60).padStart(2, "0")}:${String(seconds % 60).padStart(2, "0")}`; };
const statusText = (session: FocusState) => session.status === "away" ? "暂停中" : session.status === "resting" ? "休息中" : session.status === "completed" ? "已完成" : "进行中";
const scheduleEnd = (item: ScheduleItem) => { const [hours, minutes] = item.start.split(":").map(Number); const total = hours * 60 + minutes + item.durationMinutes; return `${total >= 1440 ? "次日 " : ""}${String(Math.floor(total / 60) % 24).padStart(2, "0")}:${String(total % 60).padStart(2, "0")}`; };

function Modal({ title, close, children, wide = false, closeBeforeRestore = false }: { title: string; close: () => void; children: React.ReactNode; wide?: boolean; closeBeforeRestore?: boolean }) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => { const before = document.activeElement; const dialog = ref.current; dialog?.showModal(); return () => { if (closeBeforeRestore && dialog?.open) dialog.close(); if (before instanceof HTMLElement && before.isConnected) before.focus(); }; }, []);
  return <dialog ref={ref} className={`modal fw-modal ${wide ? "fw-modal-wide" : ""}`} onCancel={event => { event.preventDefault(); close(); }} aria-label={title}>
    <header><h2>{title}</h2><button type="button" className="icon-button" aria-label="关闭" onClick={close}>×</button></header>{children}
  </dialog>;
}

function Tabs<T extends string>({ value, options, onChange, label }: { value: T; options: { value: T; label: string }[]; onChange: (value: T) => void; label: string }) {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  return <div className="fw-tabs" role="tablist" aria-label={label}>{options.map((item, index) => <button ref={node => { refs.current[index] = node; }} key={item.value} type="button" role="tab" aria-selected={value === item.value} tabIndex={value === item.value ? 0 : -1} onClick={() => onChange(item.value)} onKeyDown={event => { if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return; event.preventDefault(); const next = event.key === "Home" ? 0 : event.key === "End" ? options.length - 1 : (index + (event.key === "ArrowRight" ? 1 : -1) + options.length) % options.length; onChange(options[next].value); refs.current[next]?.focus(); }}>{item.label}</button>)}</div>;
}

/** Paints only the clock once per visible second. Backend snapshots own all phase transitions. */
function useVisibleClock() {
  const [now, setNow] = useState(Date.now);
  useEffect(() => { let timer: number | undefined; const run = () => { clearInterval(timer); if (!document.hidden) { setNow(Date.now()); timer = window.setInterval(() => setNow(Date.now()), 1000); } }; run(); document.addEventListener("visibilitychange", run); return () => { clearInterval(timer); document.removeEventListener("visibilitychange", run); }; }, []);
  return now;
}

export function focusDisplay(session: FocusState, now: number) {
  const delta = Math.max(0, now - session.observedAt);
  const learning = session.status === "focusing";
  const remaining = session.remainingMilliseconds === null ? null : Math.max(0, session.remainingMilliseconds - (learning ? delta : 0));
  const elapsed = session.elapsedMilliseconds + (learning ? Math.min(delta, session.remainingMilliseconds ?? delta) : 0);
  const leave = Math.max(0, session.leaveRemainingMilliseconds - (session.status === "away" ? delta : 0));
  // A phase snapshot can arrive before the visible clock's next tick. Never
  // subtract that stale clock value and briefly add a second to a new break.
  const observedNow = Math.max(now, session.observedAt);
  const rest = Math.max(0, Math.min(session.restDurationSeconds * 1000, (session.breakEndsAt ?? observedNow) - observedNow));
  return { remaining, elapsed, leave, rest };
}

function ClockDigits({ value, style }: { value: string; style: FocusAppearance["style"] }) {
  const segments: Record<string, string> = { "0": "ab cdef".replace(" ", ""), "1": "bc", "2": "abged", "3": "abgcd", "4": "fgbc", "5": "afgcd", "6": "afgecd", "7": "abc", "8": "abcdefg", "9": "abfgcd" };
  const paths: Record<string, string> = { a: "M4 1H16L18 3 16 5H4L2 3Z", b: "M17 5L19 3 21 5V16L19 18 17 16Z", c: "M17 20L19 18 21 20V31L19 33 17 31Z", d: "M4 31H16L18 33 16 35H4L2 33Z", e: "M0 20L2 18 4 20V31L2 33 0 31Z", f: "M0 5L2 3 4 5V16L2 18 0 16Z", g: "M4 16H16L18 18 16 20H4L2 18Z" };
  return <div className={`fw-digits fw-digits-${style}`} role="img" aria-label={value}>{style === "flip" ? value.split(":").map((part, index) => <span className="fw-flip-part" aria-hidden="true" key={index}>{part}</span>) : style === "digital" ? value.split("").map((character, index) => character === ":" ? <svg className="fw-digit-colon" viewBox="0 0 8 36" aria-hidden="true" key={index}><circle cx="4" cy="11" r="1.6"/><circle cx="4" cy="25" r="1.6"/></svg> : <svg viewBox="0 0 22 36" aria-hidden="true" key={index}>{Object.entries(paths).map(([segment, d]) => <path key={segment} d={d} opacity={segments[character]?.includes(segment) ? 1 : .06}/>)}</svg>) : <span aria-hidden="true">{value}</span>}</div>;
}

function TimerFace({ session, appearance, onBoundary }: { session: FocusState; appearance: FocusAppearance; onBoundary: () => void }) {
  const now = useVisibleClock();
  const display = focusDisplay(session, now);
  const boundary = session.status === "resting" ? display.rest === 0 : session.status === "away" ? display.leave === 0 : session.status === "focusing" && display.remaining === 0;
  const requested = useRef("");
  useEffect(() => { const key = `${session.id}-${session.status}-${session.observedAt}`; if (boundary && requested.current !== key) { requested.current = key; onBoundary(); } }, [boundary, session.id, session.status, session.observedAt, onBoundary]);
  const milliseconds = session.status === "resting" ? display.rest : session.status === "away" ? display.leave : session.status === "completed" ? display.elapsed : display.remaining ?? display.elapsed;
  const countingDown = session.status === "resting" || session.status === "away" || (session.status === "focusing" && display.remaining !== null);
  const ratio = session.status === "resting" ? display.rest / Math.max(1, session.restDurationSeconds * 1000) : session.status === "away" ? display.leave / Math.max(1, session.leaveBudgetSeconds * 1000) : session.durationSeconds ? (display.remaining ?? 0) / (session.durationSeconds * 1000) : 1;
  return <div className={`fw-timer-center fw-size-${appearance.size}`} data-testid="focus-clock">
    <div className="fw-clock-ring"><svg viewBox="0 0 340 340" aria-hidden="true"><circle className="fw-ring-track" cx="170" cy="170" r="162"/><circle className="fw-ring-progress" cx="170" cy="170" r="162" pathLength="100" strokeDasharray="100" strokeDashoffset={100 - Math.max(0, Math.min(1, ratio)) * 100}/></svg>
      <div className="fw-clock-face"><span className="fw-clock-label">{session.status === "away" ? "暂停剩余" : session.status === "resting" ? "休息" : session.status === "completed" ? "本次专注" : session.timerKind === "countup" ? "正向计时" : "专注剩余"}</span><ClockDigits value={secondsText(milliseconds, countingDown)} style={appearance.style}/>{session.status === "away" && <small>{display.remaining === null ? "已专注" : "学习剩余"} {secondsText(display.remaining ?? display.elapsed, display.remaining !== null)}</small>}</div>
    </div>
    <h1>{session.label}</h1><p role="status">{statusText(session)}{session.mode === "strict" && session.status !== "resting" && session.status !== "completed" ? " · 严格模式" : ""}</p>
  </div>;
}

function TaskEditor({ task, pauseMinutes, close, save }: { task: FocusTask | null; pauseMinutes: number; close: () => void; save: (input: FocusTaskInput) => Promise<void> }) {
  const [name, setName] = useState(task?.name ?? "");
  const [timerKind, setKind] = useState<FocusTaskInput["timerKind"]>(task?.timerKind ?? "countdown");
  const [duration, setDuration] = useState(task?.durationMinutes ?? 25);
  const [mode, setMode] = useState<FocusTaskInput["mode"]>(task?.mode ?? "normal");
  const [busy, setBusy] = useState(false); const [error, setError] = useState("");
  return <Modal title={task ? "编辑任务" : "添加任务"} close={close}><form onSubmit={async event => { event.preventDefault(); if (busy) return; setBusy(true); try { await save({ id: task?.id, name: name.trim(), timerKind, durationMinutes: duration, mode }); close(); } catch (reason) { setError(String(reason)); } finally { setBusy(false); } }}>
    <label>任务名称<input autoFocus aria-label="任务名称" value={name} maxLength={100} onChange={event => setName(event.target.value)} placeholder="例如：英语刷题" required/></label>
    <fieldset><legend>计时方式</legend><div className="fw-choice-row">{(["countdown", "countup"] as const).map(kind => <button key={kind} type="button" aria-pressed={timerKind === kind} onClick={() => setKind(kind)}>{kind === "countdown" ? "倒计时" : "正向计时"}</button>)}</div></fieldset>
    {timerKind === "countdown" && <><div className="fw-presets">{[25, 35].map(value => <button key={value} type="button" aria-pressed={duration === value} onClick={() => setDuration(value)}>{value} 分钟</button>)}</div><label>专注时长（分钟）<input aria-label="专注时长（分钟）" type="number" min="1" max="720" value={duration} onChange={event => setDuration(Number(event.target.value))} required/></label></>}
    <label>专注模式<select value={mode} aria-label="专注模式" onChange={event => setMode(event.target.value as FocusTaskInput["mode"])}><option value="normal">普通模式</option><option value="strict">严格模式</option></select></label><p className="fw-field-note">{mode === "normal" ? pauseMinutes ? `每轮可累计暂停 ${pauseMinutes} 分钟` : "已在设置中关闭普通暂停" : "没有普通暂停，仍可提前完成"}</p>
    {error && <p role="alert" className="fw-error">{error}</p>}<footer><button type="button" className="secondary" onClick={close}>取消</button><button className="primary" disabled={busy || !name.trim()}>保存任务</button></footer>
  </form></Modal>;
}

function Whitelist({ workspace, catalog, close, changed }: { workspace: FocusWorkspaceData; catalog: TargetCatalog; close: () => void; changed: () => Promise<void> }) {
  const [selected, setSelected] = useState<FocusWhitelistInput | null>(null); const [error, setError] = useState(""); const [busy, setBusy] = useState(false);
  const applications = workspace.whitelist.filter(item => item.targetKind === "application");
  const icons = useApplicationIcons(applications.map(item => item.path || item.target));
  const act = async (action: () => Promise<unknown>) => { if (busy) return; setBusy(true); setError(""); try { await action(); await changed(); } catch (reason) { setError(String(reason)); } finally { setBusy(false); } };
  return <Modal title="全局白名单" close={close} wide><div className="fw-whitelist-add"><TargetPicker kind="application" value={selected?.target ?? ""} catalog={catalog} onChange={(target, displayName, path) => setSelected({ targetKind: "application", target, displayName, path: path ?? catalog.applications.find(app => app.executableName.toLowerCase() === target.toLowerCase())?.path })}/><button className="primary" disabled={!selected || busy} onClick={() => void act(async () => { if (selected) await api.saveFocusWhitelist(selected); setSelected(null); })}>添加</button></div>
    {error && <p className="fw-error" role="alert">{error}</p>}
    <ul className="fw-whitelist-grid">{applications.map(item => <li key={item.id}><button className="fw-launch-app" type="button" disabled={item.availableAt > Date.now()} onClick={() => void act(() => api.launchFocusApplication(item.id))}><ApplicationIcon name={item.displayName} target={item.path || item.target} icons={icons}/><b>{item.displayName}</b>{item.availableAt > Date.now() && <small>等待生效</small>}</button><button type="button" className="fw-remove" aria-label={`移除 ${item.displayName}`} onClick={() => void act(() => api.removeFocusWhitelist(item.id))}>×</button></li>)}</ul>
    {!applications.length && <div className="fw-empty">尚未添加应用</div>}
    <footer><button type="button" className="secondary" onClick={close}>完成</button></footer>
  </Modal>;
}

function AppearanceEditor({ appearance, save, close }: { appearance: FocusAppearance; save: (value: FocusAppearance) => Promise<void>; close: () => void }) {
  const [value, setValue] = useState(appearance); const [busy, setBusy] = useState(false); const [error, setError] = useState("");
  return <Modal title="计时器外观" close={close}><form onSubmit={async event => { event.preventDefault(); setBusy(true); try { await save(value); close(); } catch (reason) { setError(String(reason)); } finally { setBusy(false); } }}><fieldset><legend>数字样式</legend><div className="fw-style-options">{(["normal", "digital", "flip"] as const).map(style => <button key={style} type="button" aria-pressed={value.style === style} onClick={() => setValue(current => ({ ...current, style }))}><ClockDigits value="25:00" style={style}/><span>{style === "normal" ? "普通" : style === "digital" ? "电子时钟" : "翻页时钟"}</span></button>)}</div></fieldset><label>数字大小<select value={value.size} onChange={event => setValue(current => ({ ...current, size: event.target.value as FocusAppearance["size"] }))}><option value="small">小</option><option value="normal">普通</option><option value="large">大</option></select></label><label className="fw-check"><input type="checkbox" checked={value.autoImmerse} onChange={event => setValue(current => ({ ...current, autoImmerse: event.target.checked }))}/>屏幕自动沉浸</label><p className="fw-field-note">计时页前台 3 秒无操作后，只显示黑底翻页时钟。</p>{error && <p role="alert" className="fw-error">{error}</p>}<footer><button type="button" className="secondary" onClick={close}>取消</button><button className="primary" disabled={busy}>保存外观</button></footer></form></Modal>;
}

function ScheduleEditor({ item, date, tasks, close, save }: { item: ScheduleItem | null; date: string; tasks: FocusTask[]; close: () => void; save: (date: string, item: ScheduleImportItem) => Promise<void> }) {
  const [taskId, setTaskId] = useState(item?.taskId ?? 0); const [name, setName] = useState(item?.label ?? "学习"); const [minutes, setMinutes] = useState(item?.durationMinutes ?? 25); const [mode, setMode] = useState(item?.mode ?? "normal"); const [error, setError] = useState(""); const [busy, setBusy] = useState(false);
  return <Modal title={item ? "编辑专注时段" : "添加专注时段"} close={close}><form onSubmit={async event => { event.preventDefault(); const form = new FormData(event.currentTarget); setBusy(true); try { await save(String(form.get("date")), { label: name, taskId: taskId || null, start: String(form.get("start")), durationMinutes: minutes, mode }); close(); } catch (reason) { setError(String(reason)); } finally { setBusy(false); } }}><label>执行日期<input type="date" name="date" defaultValue={item?.date ?? date} required/></label><label>关联任务<select aria-label="关联任务" value={taskId} onChange={event => { const id = Number(event.target.value); setTaskId(id); const task = tasks.find(entry => entry.id === id); if (task) { setName(task.name); setMinutes(task.durationMinutes); setMode(task.mode); } }}><option value={0}>不关联任务</option>{tasks.map(task => <option key={task.id} value={task.id}>{task.name}</option>)}</select></label><label>时段名称<input value={name} onChange={event => setName(event.target.value)} required/></label><div className="field-row"><label>开始时间<input name="start" type="time" defaultValue={item?.start ?? "09:00"} required/></label><label>时长（分钟）<input type="number" min="1" max="720" value={minutes} onChange={event => setMinutes(Number(event.target.value))} required/></label></div><label>模式<select value={mode} onChange={event => setMode(event.target.value as "normal" | "strict")}><option value="normal">普通</option><option value="strict">严格</option></select></label>{error && <p role="alert" className="fw-error">{error}</p>}<footer><button className="secondary" type="button" onClick={close}>取消</button><button className="primary" disabled={busy}>保存时段</button></footer></form></Modal>;
}

function ImportTaskOptions({ tasks, link }: { tasks: FocusTask[]; link: string }) {
  return <><option value="none">不关联任务</option><option value="new">创建同名任务</option>{!( ["none", "new"].includes(link) || tasks.some(task => String(task.id) === link)) && <option value={link}>任务编号 {link} 不存在，请重新选择</option>}{tasks.map(task => <option key={task.id} value={task.id}>{task.name}</option>)}</>;
}

function importTaskDescription(link: string, tasks: FocusTask[]): string {
  if (link === "none") return "不关联任务";
  if (link === "new") return "创建同名任务";
  const task = tasks.find(entry => String(entry.id) === link);
  return task ? `关联任务：${task.name}` : `任务编号 ${link} 不存在，请在编辑中重新选择`;
}

function ImportItemEditor({ item, link, tasks, close, save }: { item: ScheduleImportItem; link: string; tasks: FocusTask[]; close: () => void; save: (item: ScheduleImportItem, link: string) => void }) {
  const [value, setValue] = useState(item); const [selectedLink, setSelectedLink] = useState(link); const [error, setError] = useState("");
  return <Modal title="编辑导入时段" close={close} closeBeforeRestore><form onSubmit={event => {
    event.preventDefault();
    if (!value.label.trim()) { setError("时段名称不能为空"); return; }
    if (selectedLink === "new" && [...value.label.trim()].length > 100) { setError("任务名称不能超过 100 个字"); return; }
    if (!Number.isInteger(value.durationMinutes) || value.durationMinutes < 1 || value.durationMinutes > 720) { setError("时长须为 1 到 720 分钟"); return; }
    save(value, selectedLink); close();
  }}>
    <label>时段名称<input autoFocus aria-label="时段名称" value={value.label} onChange={event => setValue({ ...value, label: event.target.value })} required/></label>
    <div className="field-row"><label>开始时间<input aria-label="开始时间" type="time" value={value.start} onChange={event => setValue({ ...value, start: event.target.value })} required/></label><label>时长（分钟）<input aria-label="时长（分钟）" type="number" min="1" max="720" step="1" value={value.durationMinutes} onChange={event => setValue({ ...value, durationMinutes: Number(event.target.value) })} required/></label></div>
    <label>模式<select value={value.mode} onChange={event => setValue({ ...value, mode: event.target.value as ScheduleImportItem["mode"] })}><option value="normal">普通</option><option value="strict">严格</option></select></label>
    <label>关联任务<select aria-label="关联任务" value={selectedLink} onChange={event => setSelectedLink(event.target.value)}><ImportTaskOptions tasks={tasks} link={selectedLink}/></select></label>
    {error && <p role="alert" className="fw-error">{error}</p>}<footer><button type="button" className="secondary" onClick={close}>取消</button><button className="primary">保存时段</button></footer>
  </form></Modal>;
}

function ScheduleImporter({ date, tasks, close, imported }: { date: string; tasks: FocusTask[]; close: () => void; imported: (date: string) => Promise<void> }) {
  const [selectedDate, setDate] = useState(date);
  const [draft, setDraft] = useState(() => createScheduleExample(tasks));
  const [preview, setPreview] = useState<ScheduleImport | null>(null);
  const [links, setLinks] = useState<string[]>([]);
  const [error, setError] = useState(""); const [busy, setBusy] = useState(false);
  const [createdTasks, setCreatedTasks] = useState<FocusTask[]>([]);
  const [editingIndex, setEditingIndex] = useState<number | null>(null);
  const availableTasks = [...tasks, ...createdTasks];
  const parse = () => {
    try {
      const value = JSON.parse(draft) as ScheduleImport;
      if (!value || typeof value.name !== "string" || !Array.isArray(value.items) || value.items.length < 1 || value.items.length > 48) throw new Error("需要日程名称和 1 到 48 个时段");
      if (value.items.some(item => !item || typeof item.label !== "string" || typeof item.start !== "string" || typeof item.durationMinutes !== "number" || !["normal", "strict"].includes(item.mode) || (item.taskId != null && (!Number.isSafeInteger(item.taskId) || item.taskId <= 0)))) throw new Error("请检查每个时段的名称、时间、时长、模式及任务编号格式");
      setPreview(value);
      setLinks(value.items.map(item => item.taskId == null ? "none" : String(item.taskId)));
      setError("");
    } catch (reason) { setPreview(null); setError(String(reason)); }
  };
  const validate = () => {
    if (!preview) return "请先预览日程";
    if (!preview.name.trim()) return "日程名称不能为空";
    if (!preview.items.length || preview.items.length > 48) return "日程需要 1 到 48 个时段";
    const ranges: { start: number; end: number }[] = [];
    for (const [index, item] of preview.items.entries()) {
      if (!item.label.trim()) return `第 ${index + 1} 个时段的名称不能为空`;
      if (links[index] === "new" && [...item.label.trim()].length > 100) return `第 ${index + 1} 个时段的任务名称不能超过 100 个字`;
      const match = /^(\d{2}):(\d{2})$/.exec(item.start);
      if (!match || Number(match[1]) > 23 || Number(match[2]) > 59) return `第 ${index + 1} 个时段的开始时间无效`;
      if (!Number.isInteger(item.durationMinutes) || item.durationMinutes < 1 || item.durationMinutes > 720) return `第 ${index + 1} 个时段的时长须为 1 到 720 分钟`;
      if (!["normal", "strict"].includes(item.mode)) return `第 ${index + 1} 个时段的模式无效`;
      if (!["none", "new"].includes(links[index]) && !availableTasks.some(task => String(task.id) === links[index])) return `第 ${index + 1} 个时段关联的任务不存在，请重新选择`;
      const start = Number(match[1]) * 60 + Number(match[2]); ranges.push({ start, end: start + item.durationMinutes });
    }
    ranges.sort ( (a, b) => a.start - b.start );
    if (ranges.some((range, index) => index > 0 && range.start < ranges[index - 1].end)) return "日程时段不能互相重叠";
    return "";
  };
  return <><Modal title="导入日程" close={close} wide><form onSubmit={async event => {
    event.preventDefault(); if (!preview || busy) return;
    const validationError = validate(); if (validationError) { setError(validationError); return; }
    setBusy(true); setError("");
    try {
      const items = []; const resolved = [...links];
      for (const [index, item] of preview.items.entries()) {
        let id = resolved[index];
        if (id === "new") { const task = await api.saveFocusTask({ name: item.label.trim(), timerKind: "countdown", durationMinutes: item.durationMinutes, mode: item.mode }); id = String(task.id); resolved[index] = id; setCreatedTasks(current => [...current, task]); setLinks([...resolved]); }
        items.push({ ...item, taskId: id === "none" ? null : Number(id) });
      }
      await api.importSchedule(selectedDate, { ...preview, items }); await imported(selectedDate); close();
    } catch (reason) { setError(String(reason)); } finally { setBusy(false); }
  }}>
    <label>执行日期<input type="date" value={selectedDate} onChange={event => setDate(event.target.value)} required/></label>
    <ScheduleImportGuide tasks={tasks} setDraft={value => { setDraft(value); setPreview(null); setError(""); }}/>
    <label>日程 JSON<textarea aria-label="日程 JSON" spellCheck={false} value={draft} onChange={event => { setDraft(event.target.value); setPreview(null); }}/></label>
    <p className="fw-field-note">请先点击“预览”，检查或编辑时段后再导入。修改 JSON 后需要重新预览。</p>
    {error && <p role="alert" className="fw-error">{error}</p>}
    {preview && <div className="fw-import-preview">
      <label>日程名称<input value={preview.name} onChange={event => setPreview({ ...preview, name: event.target.value })}/></label>
      {preview.items.map((item, index) => <div className="fw-import-row" key={index}>
        <span><time>{item.start}</time><b>{item.label}</b><small>{item.durationMinutes} 分钟 · {item.mode === "strict" ? "严格" : "普通"} · {importTaskDescription(links[index], availableTasks)}</small></span>
        <button type="button" className="secondary" aria-label={`编辑时段 ${item.label}`} onClick={() => setEditingIndex(index)}>编辑</button>
        <button type="button" className="secondary" aria-label={`删除时段 ${item.label}`} onClick={() => { setPreview({ ...preview, items: preview.items.filter((_, i) => i !== index) }); setLinks(current => current.filter((_, i) => i !== index)); }}>删除</button>
      </div>)}
      {!preview.items.length && <p className="fw-field-note">没有可导入的时段，请重新预览 JSON。</p>}
    </div>}
    <p className="fw-field-note">将替换所选日期尚未开始的安排。</p><footer><button type="button" className="secondary" disabled={busy} onClick={parse}>预览</button><button className="primary" disabled={!preview?.items.length || busy}>导入</button></footer>
  </form></Modal>{editingIndex !== null && preview && <ImportItemEditor item={preview.items[editingIndex]} link={links[editingIndex]} tasks={availableTasks} close={() => setEditingIndex(null)} save={(item, link) => { setPreview({ ...preview, items: preview.items.map((entry, i) => i === editingIndex ? item : entry) }); setLinks(current => current.map((entry, i) => i === editingIndex ? link : entry)); setError(""); }}/>}</>;
}

function ScheduleList({ tasks, changed }: { tasks: FocusTask[]; changed: () => Promise<void> }) {
  const [date, setDate] = useState(dateKey); const [editor, setEditor] = useState<ScheduleItem | "new" | null>(null); const [importing, setImporting] = useState(false); const [error, setError] = useState("");
  const query = useCallback(() => api.listSchedule(date), [date]); const { data: schedule, error: loadError, reload } = usePolledQuery(query, () => [] as ScheduleItem[], { intervalMs: 5000 });
  const run = async (action: () => Promise<unknown>) => { try { await action(); reload(); await changed(); setError(""); } catch (reason) { setError(String(reason)); } };
  const all = schedule.length > 0 && schedule.every(item => item.enabled || item.started);
  return <section className="fw-schedules"><header><input type="date" aria-label="日程日期" value={date} onChange={event => setDate(event.target.value)}/><div><button className="secondary" disabled={!schedule.length} onClick={() => void run(() => api.setScheduleDayEnabled(date, !all))}>{all ? "全部关闭" : "全部开启"}</button><button className="secondary" onClick={() => setImporting(true)}>导入日程</button><button className="primary" onClick={() => setEditor("new")}>添加时段</button></div></header>{(error || loadError) && <p role="alert" className="fw-error">{error || loadError}</p>}
    {schedule.length ? <ul>{schedule.map(item => <li key={item.id} className={item.enabled || item.started ? "" : "fw-schedule-off"}>
      <div className="fw-schedule-time"><time>{item.start}</time><small>至 {scheduleEnd(item)}</small></div>
      <div className="fw-schedule-copy"><h3>{item.label}</h3><p>{item.durationMinutes} 分钟 · {item.mode === "strict" ? "严格" : "普通"}{item.taskId ? ` · ${tasks.find(task => task.id === item.taskId)?.name ?? "已删除任务"}` : ""}</p></div>
      <span className={`fw-schedule-status ${item.enabled && !item.started ? "is-enabled" : ""}`}>{item.started ? "已开始" : item.enabled ? "已开启" : "已关闭"}</span>
      <div className="fw-row-actions"><button disabled={item.started} aria-label={`编辑时段 ${item.label}`} onClick={() => setEditor(item)}>编辑</button><button disabled={item.started} aria-label={`删除时段 ${item.label}`} onClick={() => { if (window.confirm("删除这个专注时段？")) void run(() => api.deleteScheduleItem(item.id)); }}>删除</button><label className="switch"><input type="checkbox" aria-label={`${item.label}启用`} disabled={item.started} checked={item.enabled} onChange={event => void run(() => api.setScheduleItemEnabled(item.id, event.target.checked))}/><i/></label></div>
    </li>)}</ul> : <div className="fw-empty fw-empty-schedule"><p>这一天还没有安排</p><button className="secondary" onClick={() => setEditor("new")}>添加时段</button></div>}
    {editor && <ScheduleEditor item={editor === "new" ? null : editor} date={date} tasks={tasks} close={() => setEditor(null)} save={async (selectedDate, input) => { await api.saveScheduleItem(editor === "new" ? null : editor.id, selectedDate, input); setDate(selectedDate); reload(); await changed(); }}/>}
    {importing && <ScheduleImporter date={date} tasks={tasks} close={() => setImporting(false)} imported={async selectedDate => { setDate(selectedDate); reload(); await changed(); }}/>}
  </section>;
}

export function FocusWorkspace({ workspace, catalog, changed, timerRequest = 0, modalOpen = false }: { workspace: FocusWorkspaceData; catalog: TargetCatalog; changed: () => Promise<void>; timerRequest?: number; modalOpen?: boolean }) {
  const [tab, setTab] = useState<"tasks" | "schedule" | "statistics">("tasks"); const [timer, setTimer] = useState(false); const [editing, setEditing] = useState<FocusTask | "new" | null>(null); const [whitelist, setWhitelist] = useState(false); const [appearance, setAppearance] = useState(false); const [error, setError] = useState(""); const [busy, setBusy] = useState(false); const [immersed, setImmersed] = useState(false); const [fullscreen, setFullscreen] = useState(false);
  const [taskSearch, setTaskSearch] = useState("");
  const host = useRef<HTMLDivElement>(null); const session = workspace.session;
  useEffect(() => { if (timerRequest) setTimer(true); }, [timerRequest]);
  const run = async (action: () => Promise<unknown>) => { if (busy) return; setBusy(true); setError(""); try { await action(); await changed(); } catch (reason) { setError(String(reason)); } finally { setBusy(false); } };
  const leaveTimer = async () => { if (fullscreen) { try { if (api.isDesktop()) await getCurrentWindow().setFullscreen(false); else if (document.fullscreenElement) await document.exitFullscreen(); } catch (reason) { setError(String(reason)); } setFullscreen(false); } setTimer(false); setImmersed(false); };
  const toggleFullscreen = async () => { try { if (api.isDesktop()) { const win = getCurrentWindow(); const next = !(await win.isFullscreen()); await win.setFullscreen(next); setFullscreen(next); } else if (document.fullscreenElement) { await document.exitFullscreen(); setFullscreen(false); } else { await host.current?.requestFullscreen(); setFullscreen(true); } } catch (reason) { setError(`无法全屏：${String(reason)}`); } };
  useEffect(() => { if (!timer || !session || !workspace.appearance.autoImmerse || whitelist || appearance || modalOpen || session.status === "completed") { setImmersed(false); return; } let timeout: number | undefined; const wake = () => { clearTimeout(timeout); setImmersed(false); if (!document.hidden && document.hasFocus()) timeout = window.setTimeout(() => { if (!document.hidden && document.hasFocus()) setImmersed(true); }, 3000); }; const key = (event: KeyboardEvent) => { if (event.key === "Escape") setImmersed(false); wake(); }; wake(); window.addEventListener("pointermove", wake); window.addEventListener("pointerdown", wake); window.addEventListener("keydown", key); window.addEventListener("focus", wake); window.addEventListener("blur", wake); document.addEventListener("visibilitychange", wake); return () => { clearTimeout(timeout); window.removeEventListener("pointermove", wake); window.removeEventListener("pointerdown", wake); window.removeEventListener("keydown", key); window.removeEventListener("focus", wake); window.removeEventListener("blur", wake); document.removeEventListener("visibilitychange", wake); }; }, [timer, session?.id, session?.status, workspace.appearance.autoImmerse, whitelist, appearance, modalOpen]);
  useEffect(() => { const onChange = () => { if (!api.isDesktop()) setFullscreen(Boolean(document.fullscreenElement)); }; document.addEventListener("fullscreenchange", onChange); return () => document.removeEventListener("fullscreenchange", onChange); }, []);
  const active = session && session.status !== "completed" ? session : null;
  const search = taskSearch.trim().toLocaleLowerCase();
  const visibleTasks = search ? workspace.tasks.filter(task => task.name.toLocaleLowerCase().includes(search)) : workspace.tasks;
  return <div className={`fw-workspace ${timer && session ? "fw-timer-workspace" : "kd-page"}`} ref={host} data-testid="focus-workspace">
    {timer && session ? <section className={`fw-timer-scene ${session.status === "resting" ? "fw-rest" : ""} ${session.status === "away" ? "fw-away" : ""} ${immersed ? "fw-immersed" : ""}`} data-testid="focus-timer">
      {!immersed && <header><button type="button" onClick={() => void leaveTimer()}><Icon name="chevronLeft" size={15}/>任务列表</button><button type="button" onClick={() => void toggleFullscreen()}>{fullscreen ? "退出全屏" : "全屏"}</button></header>}
      <TimerFace session={session} appearance={immersed ? { ...workspace.appearance, style: "flip" } : workspace.appearance} onBoundary={changed}/>
      {!immersed && <>{error && <p className="fw-error" role="alert">{error}</p>}<footer className="fw-timer-controls">
        <div className="fw-timer-primary">{session.status === "focusing" && session.mode === "normal" && <button className="fw-main-control" disabled={busy || session.leaveRemainingMilliseconds <= 0} onClick={() => void run(() => api.pauseFocus(session.id))}>暂停</button>}{session.status === "away" && <button className="fw-main-control" disabled={busy} onClick={() => void run(() => api.resumeFocus(session.id))}>继续学习</button>}{["focusing", "away"].includes(session.status) && <button className={session.mode === "strict" ? "fw-main-control" : undefined} disabled={busy} onClick={() => void run(() => api.completeFocus(session.id))}>{session.timerKind === "countup" ? "完成专注" : "提前完成"}</button>}{session.status === "resting" && <button className="fw-main-control" disabled={busy} onClick={() => void run(() => api.skipFocusBreak(session.id))}>跳过休息</button>}{session.status === "completed" && <button className="fw-main-control" onClick={() => void leaveTimer()}>返回任务</button>}</div>
        {session.mode === "normal" && session.status === "focusing" && <p className="fw-pause-available">暂停可用 {secondsText(session.leaveRemainingMilliseconds)}</p>}
        <div className="fw-timer-tools"><button type="button" onClick={() => setWhitelist(true)}>白名单</button><button type="button" onClick={() => setAppearance(true)}>外观</button></div>
      </footer></>}
    </section> : <><PageHeader title="专注" subtitle="选择任务开始计时；专注期间只允许白名单中的应用。" actions={<><button className="secondary" onClick={() => setWhitelist(true)}><Icon name="shield" size={14}/>白名单</button>{tab === "tasks" && <button className="primary" onClick={() => setEditing("new")}><Icon name="plus" size={14}/>添加任务</button>}</>}/><header className="fw-workspace-header"><Tabs value={tab} onChange={setTab} label="专注页面" options={[{ value: "tasks", label: "任务" }, { value: "schedule", label: "日程" }, { value: "statistics", label: "统计" }]}/>
      {tab === "tasks" && workspace.tasks.length > 0 && <label className="fw-task-search"><svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="8.5" cy="8.5" r="5.5"/><path d="m12.5 12.5 4 4"/></svg><input type="search" aria-label="搜索任务" placeholder="搜索任务" value={taskSearch} onChange={event => setTaskSearch(event.target.value)} onKeyDown={event => { if (event.key === "Escape") setTaskSearch(""); }}/></label>}</header>
      {error && <p className="fw-error" role="alert">{error}</p>}{active && <button className="fw-resume-banner" onClick={() => setTimer(true)}><span className="fw-status-dot"/><span><b>{active.label}</b><small>{statusText(active)}</small></span><strong>返回计时<Icon name="chevronRight" size={14}/></strong></button>}
      {tab === "tasks" ? workspace.tasks.length ? visibleTasks.length ? <ul className="fw-task-list">{visibleTasks.map(task => <li key={task.id}><article><header><h2>{task.name}</h2><div><button className="fw-edit-task" aria-label={`编辑 ${task.name}`} title="编辑" onClick={() => setEditing(task)}><Icon name="edit" size={14}/></button><button className="fw-delete-task" aria-label={`删除 ${task.name}`} title="删除" onClick={() => { if (window.confirm(`删除“${task.name}”？历史专注记录会保留。`)) void run(() => api.deleteFocusTask(task.id)); }}><Icon name="trash" size={14}/></button></div></header><p>{task.timerKind === "countup" ? <b className="fw-countup-label">正向计时</b> : <><b>{task.durationMinutes}</b> 分钟</>}{task.mode === "strict" && <span>严格</span>}</p><footer><small>{task.todayCompleted ? `今日已专注 ${task.todayCompleted} 次` : "今日尚未开始"}</small><button className="primary fw-start" disabled={busy || Boolean(active && active.status !== "resting")} onClick={() => void run(async () => { await api.startFocusTask(task.id); setTimer(true); })}><Icon name="play" size={12}/>开始</button></footer></article></li>)}</ul> : <div className="fw-empty fw-empty-search"><p role="status">没有找到匹配的任务</p><button className="secondary" onClick={() => setTaskSearch("")}>清除搜索</button></div> : <div className="fw-empty fw-empty-tasks"><span className="fw-empty-icon"><Icon name="focus" size={28} strokeWidth={1.5}/></span><h2>还没有任务</h2><p>为常做的事建一个任务，例如“英语精读 · 25 分钟”，之后一键开始。</p><button className="primary" onClick={() => setEditing("new")}><Icon name="plus" size={14}/>添加任务</button></div> : tab === "schedule" ? <ScheduleList tasks={workspace.tasks} changed={changed}/> : <FocusStatistics refreshKey={`${session?.id}-${session?.status}-${workspace.tasks.map(task => `${task.id}:${task.name}`).join("|")}`}/>}</>}
    {editing && <TaskEditor task={editing === "new" ? null : editing} pauseMinutes={workspace.preferences.pauseMinutes} close={() => setEditing(null)} save={async input => { await api.saveFocusTask(input); await changed(); }}/>}
    {whitelist && <Whitelist workspace={workspace} catalog={catalog} changed={changed} close={() => setWhitelist(false)}/>}
    {appearance && <AppearanceEditor appearance={workspace.appearance} close={() => setAppearance(false)} save={async value => { await api.saveFocusAppearance(value); await changed(); }}/>}
  </div>;
}
