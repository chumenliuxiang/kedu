import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import * as api from "./api";
import { listFocusRecords, queryFocusStatistics, type FocusDistribution, type FocusRecordsPage, type FocusStatisticsQuery } from "./focusStatisticsApi";
import { usePolledQuery } from "./hooks/usePolledQuery";
import { dateFromKey, rangeBounds, shiftDate, statisticalDateKey } from "./statisticsDates";
import { chartColor, seriesPalette, useTheme } from "./theme";
import "./FocusStatistics.css";

type Range = "day" | "week" | "month" | "custom";
const ranges: { value: Range; label: string }[] = [{ value: "day", label: "日" }, { value: "week", label: "近 7 天" }, { value: "month", label: "近 30 天" }, { value: "custom", label: "自定义" }];
const palette = seriesPalette.map(([light]) => light);
const numberText = new Intl.NumberFormat("zh-CN");
const fullDate = new Intl.DateTimeFormat("zh-CN", { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false });
const shortDate = new Intl.DateTimeFormat("zh-CN", { month: "long", day: "numeric" });
const emptyRecords = (): FocusRecordsPage => ({ items: [], total: 0, offset: 0, limit: 20 });
function identityColor(key: string) { let hash = 0; for (const character of key) hash = (hash * 31 + character.charCodeAt(0)) >>> 0; return palette[hash % palette.length]; }
function parts(milliseconds: number) { const seconds = Math.max(0, Math.floor(milliseconds / 1000)); const hours = Math.floor(seconds / 3600), minutes = Math.floor(seconds % 3600 / 60); return hours ? [{ value: hours, unit: "小时" }, { value: minutes, unit: "分" }] : minutes ? [{ value: minutes, unit: "分" }, { value: seconds % 60, unit: "秒" }] : [{ value: seconds, unit: "秒" }]; }
function exactDuration(milliseconds: number) { return parts(milliseconds).filter(part => part.value > 0).map(part => `${part.value}${part.unit}`).join(" ") || "0秒"; }
function Duration({ milliseconds }: { milliseconds: number }) { return <span className="fs-duration">{parts(milliseconds).map(part => <span key={part.unit}><b>{numberText.format(part.value)}</b><small>{part.unit}</small></span>)}</span>; }

function RangeTabs({ value, setValue }: { value: Range; setValue: (next: Range) => void }) {
  const buttons = useRef<(HTMLButtonElement | null)[]>([]);
  return <div className="fs-range-tabs" role="tablist" aria-label="专注统计范围">{ranges.map((range, index) => <button type="button" key={range.value} ref={button => { buttons.current[index] = button; }} role="tab" tabIndex={range.value === value ? 0 : -1} aria-selected={range.value === value} onClick={() => setValue(range.value)} onKeyDown={event => {
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
    event.preventDefault(); const next = event.key === "Home" ? 0 : event.key === "End" ? ranges.length - 1 : (index + (event.key === "ArrowRight" ? 1 : -1) + ranges.length) % ranges.length;
    setValue(ranges[next].value); buttons.current[next]?.focus();
  }}>{range.label}</button>)}</div>;
}

function TaskPie({ items, total, selected, onSelect, onHover }: { items: FocusDistribution[]; total: number; selected: string | null; onSelect: (key: string) => void; onHover: (key: string | null) => void }) {
  const { resolved } = useTheme();
  const positive = items.filter(item => item.milliseconds > 0);
  let angle = -Math.PI / 2;
  const keyboard = (event: KeyboardEvent<SVGElement>, key: string) => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); onSelect(key); } };
  if (!positive.length) return <div className="fs-pie-empty"><svg viewBox="0 0 240 240" aria-hidden="true"><circle cx="120" cy="120" r="105"/></svg><span>暂无专注时长</span></div>;
  return <svg viewBox="0 0 240 240" className="fs-pie" role="group" aria-label="按任务的专注时长分布">{positive.map(item => {
    const start = angle; angle += item.milliseconds / total * 2 * Math.PI;
    const path = `M120 120 L${120 + 105 * Math.cos(start)} ${120 + 105 * Math.sin(start)} A105 105 0 ${angle - start > Math.PI ? 1 : 0} 1 ${120 + 105 * Math.cos(angle)} ${120 + 105 * Math.sin(angle)} Z`;
    const properties = { className: `fs-pie-slice ${selected && selected !== item.key ? "is-muted" : ""}`, fill: chartColor(identityColor(item.key), resolved), tabIndex: 0, role: "button", "aria-label": `${item.label}，${exactDuration(item.milliseconds)}，${(item.milliseconds / total * 100).toFixed(1)}%，查看记录`, onClick: () => onSelect(item.key), onKeyDown: (event: KeyboardEvent<SVGElement>) => keyboard(event, item.key), onMouseEnter: () => onHover(item.key), onMouseLeave: () => onHover(null), onFocus: () => onHover(item.key), onBlur: () => onHover(null) };
    const title = <title>{item.label} · {exactDuration(item.milliseconds)}</title>;
    return positive.length === 1 ? <circle key={item.key} cx="120" cy="120" r="105" {...properties}>{title}</circle> : <path key={item.key} d={path} {...properties}>{title}</path>;
  })}</svg>;
}

function FocusRecords({ bounds, tasks, initialTask, close }: { bounds: FocusStatisticsQuery; tasks: FocusDistribution[]; initialTask: string; close: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [taskKey, setTaskKey] = useState(initialTask), [search, setSearch] = useState(""), [settledSearch, setSettledSearch] = useState("");
  const [page, setPage] = useState(0);
  useEffect(() => { const before = document.activeElement; dialog.current?.showModal(); return () => { if (before instanceof HTMLElement && before.isConnected) before.focus(); }; }, []);
  useEffect(() => { const timer = window.setTimeout(() => { setSettledSearch(search); setPage(0); }, 200); return () => clearTimeout(timer); }, [search]);
  const query = useCallback(() => listFocusRecords({ ...bounds, taskKey: taskKey || undefined, search: settledSearch, offset: page * 20, limit: 20 }), [bounds.start, bounds.end, taskKey, settledSearch, page]);
  const { data, state, error, reload } = usePolledQuery(query, emptyRecords);
  return <dialog ref={dialog} className="modal fs-records-dialog" aria-label="专注记录" onCancel={event => { event.preventDefault(); close(); }}>
    <header><div><h2>专注记录</h2><p>{shortDate.format(bounds.start)} — {shortDate.format(bounds.end - 1)}</p></div><button type="button" className="fs-close" aria-label="关闭专注记录" onClick={close}>×</button></header>
    <div className="fs-record-filters"><label><span className="visually-hidden">筛选专注任务</span><select value={taskKey} onChange={event => { setTaskKey(event.target.value); setPage(0); }}><option value="">全部任务</option>{tasks.map(task => <option key={task.key} value={task.key}>{task.label}</option>)}</select></label><label><span className="visually-hidden">搜索专注记录</span><input type="search" placeholder="搜索任务名称" value={search} onChange={event => setSearch(event.target.value)}/></label></div>
    {state === "error" ? <div className="fs-query-state" role="alert"><p>{error}</p><button type="button" onClick={reload}>重试</button></div> : state === "loading" ? <div className="fs-query-state" role="status">正在读取记录…</div> : <>
      <div className="fs-record-list" role="region" aria-label="专注记录列表" tabIndex={0}>
        {data.items.length ? <table><thead><tr><th scope="col">任务</th><th scope="col">开始／完成</th><th scope="col">专注时长</th></tr></thead><tbody>{data.items.map(record => <tr key={record.id}><th scope="row"><strong>{record.label}</strong><span>{record.timerKind === "countup" ? "正向计时" : "倒计时"}{record.mode === "strict" ? " · 严格" : ""} · {record.completionReason === "manual" ? "手动完成" : "自然完成"}</span></th><td><time dateTime={new Date(record.startedAt).toISOString()}>{fullDate.format(record.startedAt)}</time><time dateTime={new Date(record.completedAt).toISOString()}>{fullDate.format(record.completedAt)}</time></td><td><b title={`${record.milliseconds} 毫秒`}>{exactDuration(record.milliseconds)}</b>{record.rangeMilliseconds !== record.milliseconds && <small>本范围 {exactDuration(record.rangeMilliseconds)}</small>}{record.pauseMilliseconds > 0 && <small>暂停 {exactDuration(record.pauseMilliseconds)}</small>}</td></tr>)}</tbody></table> : <p className="fs-query-state">{search || taskKey ? "没有匹配的专注记录" : "没有专注记录"}</p>}
      </div>
      <footer><span>{data.total} 条记录</span><div><button type="button" aria-label="上一页专注记录" disabled={page === 0} onClick={() => setPage(value => value - 1)}>‹</button><span>{page + 1} / {Math.max(1, Math.ceil(data.total / 20))}</span><button type="button" aria-label="下一页专注记录" disabled={(page + 1) * 20 >= data.total} onClick={() => setPage(value => value + 1)}>›</button></div></footer>
    </>}
  </dialog>;
}

export function FocusStatistics({ refreshKey }: { refreshKey?: string | number }) {
  const [hour, setHour] = useState<number | null>(null), [settingsError, setSettingsError] = useState("");
  const [today, setToday] = useState(() => statisticalDateKey(0));
  const [selected, setSelected] = useState<string | null>(null), [range, setRange] = useState<Range>("day");
  const [customStart, setCustomStart] = useState(shiftDate(today, -6)), [customEnd, setCustomEnd] = useState(today);
  const [hovered, setHovered] = useState<string | null>(null), [recordsTask, setRecordsTask] = useState<string | null>(null);
  const { resolved } = useTheme();
  const loadSettings = useCallback(() => { setSettingsError(""); void api.getSettings().then(settings => { setHour(settings.dayStartHour); setToday(statisticalDateKey(settings.dayStartHour)); }).catch(reason => setSettingsError(String(reason))); }, []);
  useEffect(loadSettings, [loadSettings]);
  useEffect(() => {
    if (hour === null) return;
    let timer: number;
    const update = () => { const current = new Date(); setToday(statisticalDateKey(hour, current)); const next = new Date(current); next.setHours(hour, 0, 0, 0); if (next.getTime() <= current.getTime()) next.setDate(next.getDate() + 1); window.clearTimeout(timer); timer = window.setTimeout(update, next.getTime() - current.getTime() + 100); };
    const visible = () => { if (!document.hidden) update(); };
    update(); document.addEventListener("visibilitychange", visible);
    return () => { window.clearTimeout(timer); document.removeEventListener("visibilitychange", visible); };
  }, [hour]);
  const endpoint = selected && selected < today ? selected : today;
  const invalidRange = range === "custom" && (!customStart || !customEnd || customStart > customEnd || customEnd > today);
  const bounds = useMemo(() => rangeBounds(range, endpoint, customStart || today, customEnd || today, hour ?? 0, today), [range, endpoint, customStart, customEnd, hour, today]);
  const query = useCallback(() => queryFocusStatistics(bounds), [bounds.start, bounds.end, refreshKey]);
  const { data, state, error, reload } = usePolledQuery(query, () => null, { enabled: hour !== null && !invalidRange });
  const shift = (direction: -1 | 1) => {
    const days = range === "week" ? 7 : range === "month" ? 30 : 1;
    const next = shiftDate(endpoint, direction * days); setSelected(next > today ? today : next);
  };
  const rangeLabel = range === "day" ? (endpoint === today ? "今日专注" : "当日专注") : "所选时段";
  const hoveredTask = data?.distribution.find(item => item.key === hovered);
  const changeRange = (next: Range) => { setRange(next); setHovered(null); };
  return <section className="fs-workspace" aria-label="专注统计" data-testid="focus-statistics">
    {settingsError ? <div className="fs-query-state" role="alert"><p>{settingsError}</p><button type="button" onClick={loadSettings}>重试</button></div> : <>
      <div className="fs-cumulative"><h2>累计专注</h2><div className="fs-metrics"><div><span>次数</span><strong>{data ? numberText.format(data.total.completedCount) : "—"}<small>次</small></strong></div><div><span>时长</span>{data ? <Duration milliseconds={data.total.milliseconds}/> : <strong>—</strong>}</div><div><span tabIndex={0} title={data?.total.daysCount ? `从首次专注至今天，共 ${data.total.daysCount} 天（包含没有专注的日期），按每日换日点划分。` : "完成专注后计算日均时长"} aria-label={data?.total.daysCount ? `日均时长，按首次专注至今天共${data.total.daysCount}天计算，包含没有专注的日期` : "日均时长"}>日均时长 ⓘ</span>{data ? <Duration milliseconds={data.total.averageMilliseconds}/> : <strong>—</strong>}</div></div></div>
      <div className="fs-period-toolbar"><RangeTabs value={range} setValue={changeRange}/>{range !== "custom" ? <div className="fs-date-nav"><button type="button" aria-label="上一段专注统计" onClick={() => shift(-1)}>‹</button><label><span className="visually-hidden">专注统计日期</span><input type="date" value={endpoint} max={today} onChange={event => { if (event.target.value && event.target.value <= today) setSelected(event.target.value); }}/></label><button type="button" aria-label="下一段专注统计" disabled={endpoint >= today} onClick={() => shift(1)}>›</button><button type="button" onClick={() => setSelected(null)}>今天</button></div> : <div className="fs-custom-dates"><label><span className="visually-hidden">专注统计开始日期</span><input type="date" value={customStart} max={customEnd < today ? customEnd : today} onChange={event => setCustomStart(event.target.value)}/></label><span>—</span><label><span className="visually-hidden">专注统计结束日期</span><input type="date" value={customEnd} min={customStart} max={today} onChange={event => setCustomEnd(event.target.value)}/></label></div>}</div>
      {invalidRange ? <p className="fs-query-state" role="alert">请选择有效日期范围，结束日期不能晚于今天。</p> : state === "error" ? <div className="fs-query-state" role="alert"><p>专注统计读取失败：{error}</p><button type="button" onClick={reload}>重试</button></div> : state === "loading" || !data ? <div className="fs-query-state" role="status">正在读取专注统计…</div> : <>
        <div className="fs-period-summary"><div><h2>{rangeLabel}</h2><p>{range === "day" ? shortDate.format(dateFromKey(endpoint)) : `${shortDate.format(bounds.start)} — ${shortDate.format(bounds.end - 1)}`}</p></div><div className="fs-period-values"><span><b>{numberText.format(data.range.completedCount)}</b> 次</span><Duration milliseconds={data.range.milliseconds}/></div><button type="button" className="fs-record-button" onClick={() => setRecordsTask("")}>查看专注记录 <span aria-hidden="true">↗</span></button></div>
        <div className="fs-distribution"><div className="fs-chart-panel"><h2>专注时长分布</h2><TaskPie items={data.distribution} total={data.range.milliseconds} selected={hovered} onSelect={setRecordsTask} onHover={setHovered}/><p className="fs-chart-caption"><span>{hoveredTask?.label ?? "总计"}</span><b>{exactDuration(hoveredTask?.milliseconds ?? data.range.milliseconds)}</b></p></div><div className="fs-task-panel"><div className="fs-task-heading"><h2>任务</h2><span>时长 / 占比</span></div>{data.distribution.length ? <ul className="fs-task-list">{data.distribution.map(task => <li key={task.key}><button type="button" onClick={() => setRecordsTask(task.key)} onMouseEnter={() => setHovered(task.key)} onMouseLeave={() => setHovered(null)} onFocus={() => setHovered(task.key)} onBlur={() => setHovered(null)}><i style={{ background: chartColor(identityColor(task.key), resolved) }} aria-hidden="true"/><span><strong>{task.label}</strong><small>{task.completedCount} 次</small></span><time>{exactDuration(task.milliseconds)}</time><b>{data.range.milliseconds ? (task.milliseconds / data.range.milliseconds * 100).toFixed(1) : "0.0"}%</b></button></li>)}</ul> : <div className="fs-query-state">这个时段还没有完成的专注</div>}</div></div>
      </>}
    </>}
    {recordsTask !== null && data && <FocusRecords bounds={bounds} tasks={data.distribution} initialTask={recordsTask} close={() => setRecordsTask(null)}/>}
  </section>;
}
