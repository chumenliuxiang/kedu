import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent, PointerEvent } from "react";
import type { TargetCatalog, TimelineItem } from "./types";
import { ApplicationIcon, useApplicationIcons } from "./components/ApplicationIcon";
import { duration } from "./components/TrendChart";
import { usePolledQuery } from "./hooks/usePolledQuery";
import { chartColor, useTheme } from "./theme";
import { annotateTimeline, getTimelineWorkspace, undoTimelineAnnotation } from "./timelineApi";
import { blockAt, intersectRanges, itemRange, mergeTimeline, normalizeRanges, rangeDuration, subtractRanges, type TimeRange, type TimelineBlock } from "./timelineSelection";
import "./TimelineWorkspace.css";

const MIN_SPAN = 60_000;
const clock = (time: number) => new Date(time).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false });
const elapsed = (ms: number) => duration(ms / 1000);
const clamp = (n: number, low: number, high: number) => Math.min(high, Math.max(low, n));
const complete = (selected: TimeRange[], ranges: TimeRange[]) => ranges.length > 0 && rangeDuration(subtractRanges(ranges, selected)) === 0;

function MarkBox({ ranges, selected, onToggle, label, disabled = false }: { ranges: TimeRange[]; selected: TimeRange[]; onToggle: (ranges: TimeRange[]) => void; label: string; disabled?: boolean }) {
  const input = useRef<HTMLInputElement>(null);
  const checked = complete(selected, ranges);
  const partial = !checked && intersectRanges(selected, ranges).length > 0;
  useLayoutEffect(() => { if (input.current) input.current.indeterminate = partial; }, [partial]);
  return <input ref={input} type="checkbox" checked={checked} aria-label={label} disabled={disabled} onChange={() => onToggle(ranges)} />;
}

// Canvas keeps dense days cheap; the summary and detail checkboxes provide the
// equivalent keyboard-accessible selection without thousands of off-screen nodes.
function Rail({ blocks, view, selected, small = false }: { blocks: TimelineBlock[]; view: TimeRange; selected: TimeRange[]; small?: boolean }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const { resolved } = useTheme();
  useLayoutEffect(() => {
    const element = canvas.current;
    if (!element) return;
    const draw = () => {
      const rect = element.getBoundingClientRect(), scale = devicePixelRatio || 1;
      element.width = Math.round(rect.width * scale); element.height = Math.round(rect.height * scale);
      const ctx = element.getContext("2d"); if (!ctx) return;
      ctx.scale(scale, scale);
      const span = view.end - view.start, x = (time: number) => (time - view.start) / span * rect.width;
      const styles = getComputedStyle(document.documentElement);
      const accent = styles.getPropertyValue("--kd-accent").trim() || "#32776f";
      const idle = styles.getPropertyValue("--kd-idle").trim() || "#c8d3d1";
      const trackTop = small ? 0 : 12, trackHeight = small ? rect.height : rect.height - 24;
      for (const block of blocks) {
        if (block.end <= view.start || block.start >= view.end) continue;
        const left = x(Math.max(view.start, block.start)), right = x(Math.min(view.end, block.end));
        ctx.fillStyle = block.idle ? idle : chartColor(block.color, resolved);
        ctx.globalAlpha = block.idle ? .65 : .9;
        ctx.fillRect(left, trackTop, Math.max(.5, right - left - (small ? 0 : .5)), trackHeight);
      }
      ctx.globalAlpha = 1;
      if (!small) for (const r of intersectRanges(selected, [view])) {
        const left = x(r.start), width = Math.max(1, x(r.end) - left);
        ctx.fillStyle = accent; ctx.globalAlpha = .13; ctx.fillRect(left, 0, width, rect.height); ctx.globalAlpha = 1;
        ctx.strokeStyle = accent; ctx.lineWidth = 1.5; ctx.strokeRect(left + .75, .75, Math.max(0, width - 1.5), rect.height - 1.5);
      }
    };
    draw(); const observer = new ResizeObserver(draw); observer.observe(element);
    return () => observer.disconnect();
  }, [blocks, view, selected, small, resolved]);
  return <canvas ref={canvas} aria-hidden="true" className="tw-rail-canvas" />;
}

export function TimelineWorkspace({ start, end, catalog, discoverApplications, onChanged }: { start: number; end: number; catalog?: TargetCatalog; discoverApplications?: (targets: string[]) => void; onChanged?: () => void }) {
  const query = useCallback(async () => ({ ...await getTimelineWorkspace(start, end), start, end }), [start, end]);
  const { data, state, error, reload } = usePolledQuery(query, () => ({ items: [] as TimelineItem[], categories: [], undoAvailable: false, start, end }), { intervalMs: end > Date.now() && start <= Date.now() ? 5000 : 0 });
  const current = data.start === start && data.end === end;
  const [mode, setMode] = useState<"application" | "category">("application");
  const [view, setView] = useState<TimeRange>({ start, end });
  const [scope, setScope] = useState<TimeRange[] | null>(null);
  const [selected, setSelected] = useState<TimeRange[]>([]);
  const [search, setSearch] = useState("");
  const [onlyUnclassified, setOnlyUnclassified] = useState(false);
  const [category, setCategory] = useState("");
  const [detailOpen, setDetailOpen] = useState(false), [page, setPage] = useState(0);
  const [busy, setBusy] = useState(false), [message, setMessage] = useState(""), [actionError, setActionError] = useState("");
  const [hover, setHover] = useState<{ block: TimelineBlock; title: string } | null>(null);
  const [preview, setPreview] = useState<TimeRange[] | null>(null);
  const rail = useRef<HTMLDivElement>(null), navigator = useRef<HTMLDivElement>(null);
  const viewRef = useRef(view); viewRef.current = view;
  const selectionDrag = useRef<{ x: number; time: number; append: boolean; base: TimeRange[]; scope: TimeRange[]; moved: boolean } | null>(null);
  const navigationDrag = useRef<{ x: number; view: TimeRange; kind: string; width: number } | null>(null);
  const keyboardBlock = useRef(-1);
  const clickSnapshot = useRef<{ at: number; x: number; scope: TimeRange[] | null; selected: TimeRange[] } | null>(null);
  const wholeDay = useMemo(() => ({ start, end }), [start, end]);
  const colors = useMemo(() => new Map(data.categories.map(c => [c.name, c.color])), [data.categories]);
  const items = useMemo(() => {
    if (!current) return [];
    const names = new Map(catalog?.applications.map(a => [a.executableName.toLowerCase(), a.displayName]) || []);
    return data.items.map(item => {
      const name = names.get((item.target || "").toLowerCase());
      return name && (item.appName.toLowerCase() === item.target?.replace(/\.exe$/i, "").toLowerCase() || item.target?.toLowerCase() === "explorer.exe") ? { ...item, appName: name } : item;
    }).sort((a, b) => a.startedAt - b.startedAt);
  }, [data.items, current, catalog]);
  const targets = useMemo(() => [...new Set(items.filter(i => !i.idle).map(i => i.target || i.appName))], [items]);
  useEffect(() => { discoverApplications?.(targets); }, [targets, discoverApplications]);
  const icons = useApplicationIcons(targets);
  const matching = useMemo(() => { const term = search.trim().toLocaleLowerCase(); return items.filter(item => (!term || `${item.appName} ${item.title}`.toLocaleLowerCase().includes(term)) && (!onlyUnclassified || !item.category || item.category === "未分类")); }, [items, search, onlyUnclassified]);
  const blocks = useMemo(() => mergeTimeline(matching, mode, colors), [matching, mode, colors]);
  const dayBlocks = useMemo(() => mergeTimeline(items, mode, colors), [items, mode, colors]);
  const candidates = useMemo(() => matching.filter(i => !i.idle).flatMap(item => intersectRanges([itemRange(item)], scope || [wholeDay]).map(range => ({ item, range }))), [matching, scope, wholeDay]);
  const eligible = useMemo(() => normalizeRanges(candidates.map(c => c.range)), [candidates]);
  const actualSelected = useMemo(() => intersectRanges(selected, eligible), [selected, eligible]);
  const total = rangeDuration(eligible), selectedTime = rangeDuration(actualSelected);
  const summary = useMemo(() => {
    const rows = new Map<string, { key: string; name: string; color: string; ranges: TimeRange[]; milliseconds: number }>();
    for (const { item, range } of candidates) {
      const key = mode === "application" ? item.target || item.appName : item.category || "未分类";
      let row = rows.get(key);
      if (!row) { row = { key, name: mode === "application" ? item.appName : key, color: mode === "application" ? item.color || "#32776f" : colors.get(key) || "#88938f", ranges: [], milliseconds: 0 }; rows.set(key, row); }
      row.ranges.push(range); row.milliseconds += range.end - range.start;
    }
    return [...rows.values()].sort((a, b) => b.milliseconds - a.milliseconds);
  }, [candidates, mode, colors]);
  useEffect(() => { setView({ start, end }); setScope(null); setSelected([]); setPreview(null); setHover(null); setPage(0); setMessage(""); keyboardBlock.current = -1; selectionDrag.current = null; navigationDrag.current = null; }, [start, end]);
  useEffect(() => { setPage(0); }, [search, onlyUnclassified, scope]);
  const setViewport = useCallback((a: number, b: number) => {
    const span = clamp(b - a, Math.min(MIN_SPAN, end - start), end - start);
    const left = Math.round(clamp(a, start, end - span)); setView({ start: left, end: Math.round(left + span) });
  }, [start, end]);
  const zoom = useCallback((factor: number, ratio = .5) => {
    const v = viewRef.current, span = clamp((v.end - v.start) * factor, Math.min(MIN_SPAN, end - start), end - start);
    const anchor = v.start + (v.end - v.start) * ratio; setViewport(anchor - span * ratio, anchor + span * (1 - ratio));
  }, [start, end, setViewport]);
  useEffect(() => {
    const element = rail.current; if (!element) return;
    const wheel = (event: WheelEvent) => { if (busy) return; event.preventDefault(); const bounds = element.getBoundingClientRect(); zoom(Math.exp(clamp(event.deltaY, -500, 500) * .002), clamp((event.clientX - bounds.left) / bounds.width, 0, 1)); };
    element.addEventListener("wheel", wheel, { passive: false }); return () => element.removeEventListener("wheel", wheel);
  }, [zoom, busy]);
  const pointTime = (x: number) => { const rect = rail.current!.getBoundingClientRect(); const v = viewRef.current; return Math.round(v.start + clamp((x - rect.left) / rect.width, 0, 1) * (v.end - v.start)); };
  const choose = (ranges: TimeRange[], append: boolean) => { setScope(old => normalizeRanges([...(append ? old || selected : []), ...ranges])); setSelected(old => normalizeRanges([...(append ? old : []), ...ranges])); setMessage(""); };
  const toggle = (ranges: TimeRange[]) => { setSelected(old => complete(old, ranges) ? subtractRanges(old, ranges) : normalizeRanges([...old, ...ranges])); setMessage(""); };
  const cancelSelection = () => { selectionDrag.current = null; setPreview(null); };
  const selectDown = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || busy) return;
    event.currentTarget.focus(); event.currentTarget.setPointerCapture(event.pointerId);
    selectionDrag.current = { x: event.clientX, time: pointTime(event.clientX), append: event.ctrlKey || event.metaKey, base: selected, scope: scope || [], moved: false }; setHover(null);
  };
  const selectMove = (event: PointerEvent<HTMLDivElement>) => {
    const drag = selectionDrag.current, time = pointTime(event.clientX);
    if (drag) { if (!drag.moved && Math.abs(event.clientX - drag.x) < 4) return; drag.moved = true; setPreview(normalizeRanges([...(drag.append ? drag.base : []), { start: Math.min(time, drag.time), end: Math.max(time, drag.time) }])); return; }
    const block = blockAt(blocks, time); const item = block ? matching.find(i => i.startedAt <= time && i.endedAt > time) : undefined;
    const title = item?.title || "";
    setHover(old => old?.block === block && old?.title === title ? old : block ? { block, title } : null);
  };
  const selectUp = (event: PointerEvent<HTMLDivElement>) => {
    const drag = selectionDrag.current; if (!drag) return;
    const time = pointTime(event.clientX);
    if (drag.moved) choose([{ start: Math.min(time, drag.time), end: Math.max(time, drag.time) }], drag.append);
    else {
      const previous = clickSnapshot.current;
      if (!previous || performance.now() - previous.at > 600 || Math.abs(previous.x - event.clientX) > 4) clickSnapshot.current = { at: performance.now(), x: event.clientX, scope, selected };
      const block = blockAt(blocks, time); if (block && !block.idle) { if (drag.append && complete(selected, [block])) setSelected(old => subtractRanges(old, [block])); else choose([block], drag.append); } else if (!drag.append) { setScope(null); setSelected([]); }
    }
    cancelSelection(); if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
  };
  const keyRail = (event: KeyboardEvent<HTMLDivElement>) => {
    const v = viewRef.current, span = v.end - v.start;
    if (["+", "=", "-", "0", "Home", "ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Enter", " ", "Escape"].includes(event.key)) event.preventDefault(); else return;
    if (busy) return;
    if (event.key === "+" || event.key === "=") zoom(1 / 1.5);
    else if (event.key === "-") zoom(1.5);
    else if (event.key === "Home" || event.key === "0") setViewport(start, end);
    else if (event.key === "ArrowLeft" || event.key === "ArrowRight") { const delta = span * (event.key === "ArrowLeft" ? -.2 : .2); setViewport(v.start + delta, v.end + delta); }
    else if (event.key === "Escape") { cancelSelection(); setScope(null); setSelected([]); }
    else if (event.key === "ArrowUp" || event.key === "ArrowDown") { keyboardBlock.current = clamp(keyboardBlock.current + (event.key === "ArrowDown" ? 1 : -1), 0, blocks.length - 1); const block = blocks[keyboardBlock.current]; if (block) { setHover({ block, title: matching.find(i => i.startedAt === block.start)?.title || "" }); if (block.start < v.start || block.end > v.end) setViewport(block.start, block.start + span); } }
    else { const block = blocks[keyboardBlock.current]; if (block && !block.idle) choose([block], event.ctrlKey || event.metaKey); }
  };
  const navDown = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || busy) return;
    const target = (event.target as HTMLElement).closest<HTMLElement>("[data-nav-kind]");
    const rect = event.currentTarget.getBoundingClientRect();
    if (!target) { const center = start + clamp((event.clientX - rect.left) / rect.width, 0, 1) * (end - start), span = view.end - view.start; setViewport(center - span / 2, center + span / 2); return; }
    target.focus(); event.currentTarget.setPointerCapture(event.pointerId);
    navigationDrag.current = { x: event.clientX, view, kind: target.dataset.navKind!, width: rect.width };
  };
  const navMove = (event: PointerEvent<HTMLDivElement>) => {
    const drag = navigationDrag.current; if (!drag) return; const delta = (event.clientX - drag.x) / drag.width * (end - start), v = drag.view;
    if (drag.kind === "start") setViewport(clamp(v.start + delta, start, v.end - MIN_SPAN), v.end);
    else if (drag.kind === "end") setViewport(v.start, clamp(v.end + delta, v.start + MIN_SPAN, end));
    else setViewport(v.start + delta, v.end + delta);
  };
  const navKey = (event: KeyboardEvent<HTMLDivElement>, kind: string) => {
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
    event.preventDefault(); if (busy) return;
    const span = view.end - view.start, step = Math.max(60_000, span / 20), direction = event.key === "ArrowLeft" ? -1 : 1;
    if (kind === "start") setViewport(event.key === "Home" ? start : event.key === "End" ? view.end - MIN_SPAN : clamp(view.start + direction * step, start, view.end - MIN_SPAN), view.end);
    else if (kind === "end") setViewport(view.start, event.key === "End" ? end : event.key === "Home" ? view.start + MIN_SPAN : clamp(view.end + direction * step, view.start + MIN_SPAN, end));
    else { const left = event.key === "Home" ? start : event.key === "End" ? end - span : view.start + direction * step; setViewport(left, left + span); }
  };
  const run = async (action: "apply" | "auto" | "undo") => {
    if (busy) return; setBusy(true); setActionError(""); setMessage("");
    try {
      if (action === "undo") { const result = await undoTimelineAnnotation(); setMessage(`已撤销 · ${elapsed(result.restoredMilliseconds)}`); }
      else { const result = await annotateTimeline(actualSelected, action === "auto" ? null : Number(category), onlyUnclassified); setMessage(result.changedMilliseconds ? `${action === "auto" ? "已恢复自动分类" : "已归入分类"} · ${elapsed(result.changedMilliseconds)}` : "所选时段无需更改"); }
      reload(); onChanged?.();
    } catch (reason) { setActionError(String(reason)); } finally { setBusy(false); }
  };
  const visiblePage = Math.min(page, Math.max(0, Math.ceil(candidates.length / 30) - 1));
  const navLeft = (view.start - start) / (end - start) * 100, navWidth = (view.end - view.start) / (end - start) * 100;
  const resetFromDoubleClick = () => { setViewport(start, end); if (clickSnapshot.current) { setScope(clickSnapshot.current.scope); setSelected(clickSnapshot.current.selected); clickSnapshot.current = null; } };
  return <section className="tw-workspace" data-testid="timeline-workspace" aria-label="时间轴整理" aria-busy={busy || !current || state === "loading"}>
    <header className="tw-toolbar"><div className="tw-modes" aria-label="时间轴视图">{(["application", "category"] as const).map(value => <button key={value} aria-pressed={mode === value} onClick={() => setMode(value)}>{value === "application" ? "程序" : "标签"}</button>)}</div><input type="search" aria-label="搜索时间轴" placeholder="搜索应用或窗口标题" value={search} onChange={e => setSearch(e.target.value)} /><label className="tw-check"><input type="checkbox" checked={onlyUnclassified} onChange={e => setOnlyUnclassified(e.target.checked)} />仅未分类</label><button onClick={() => setViewport(start, end)}>显示全天</button></header>
    {(error || actionError) && <p role="alert" className="tw-error">{actionError || error}<button onClick={reload}>重试</button></p>}
    <div className="tw-track-panel">
      <div className="tw-ticks">{Array.from({ length: 5 }, (_, i) => <span key={i}>{clock(view.start + (view.end - view.start) * i / 4).slice(0, 5)}</span>)}</div>
      <div ref={rail} className="tw-track" tabIndex={0} role="region" aria-label="活动时间轴。拖动选择时间，Ctrl追加；加减键缩放，左右键平移，上下键浏览活动，回车选择，Home显示全天。" onPointerDown={selectDown} onPointerMove={selectMove} onPointerUp={selectUp} onPointerCancel={cancelSelection} onLostPointerCapture={cancelSelection} onPointerLeave={() => { if (!selectionDrag.current) setHover(null); }} onDoubleClick={resetFromDoubleClick} onKeyDown={keyRail}><Rail blocks={blocks} view={view} selected={preview || actualSelected} />{!matching.length && <span className="tw-empty-track">{!current || state === "loading" ? "加载中…" : error ? "读取失败" : "没有匹配记录"}</span>}</div>
      <div className="tw-hover" aria-live="polite">{hover ? <><strong>{hover.block.name}</strong><span>{clock(hover.block.start)}–{clock(hover.block.end)}</span><span className="tw-hover-title" title={hover.title}>{hover.title}</span><b>{elapsed(hover.block.end - hover.block.start)}</b></> : <span>拖选时间 · Ctrl 追加选择</span>}</div>
      <div ref={navigator} className="tw-navigator" aria-label="全天导航" onPointerDown={navDown} onPointerMove={navMove} onPointerUp={e => { navigationDrag.current = null; if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId); }} onPointerCancel={() => { navigationDrag.current = null; }} onLostPointerCapture={() => { navigationDrag.current = null; }}><Rail blocks={dayBlocks} view={wholeDay} selected={[]} small /><div className="tw-nav-shade" style={{ left: 0, width: `${navLeft}%` }} /><div className="tw-nav-shade" style={{ left: `${navLeft + navWidth}%`, right: 0 }} /><div className="tw-nav-window" style={{ left: `${navLeft}%`, width: `${navWidth}%` }}><div data-nav-kind="pan" role="slider" tabIndex={0} aria-label="平移可见时间" aria-valuemin={start} aria-valuemax={end - (view.end - view.start)} aria-valuenow={view.start} aria-valuetext={`${clock(view.start)}至${clock(view.end)}`} onKeyDown={e => navKey(e, "pan")} /><div data-nav-kind="start" role="slider" tabIndex={0} aria-label="可见时间起点" aria-valuemin={start} aria-valuemax={view.end - MIN_SPAN} aria-valuenow={view.start} aria-valuetext={clock(view.start)} onKeyDown={e => navKey(e, "start")} /><div data-nav-kind="end" role="slider" tabIndex={0} aria-label="可见时间终点" aria-valuemin={view.start + MIN_SPAN} aria-valuemax={end} aria-valuenow={view.end} aria-valuetext={clock(view.end)} onKeyDown={e => navKey(e, "end")} /></div></div>
    </div>
    <div className="tw-selection"><div><span>{scope ? "选区" : "全天"} <b data-testid="selection-span">{elapsed(rangeDuration(scope || [wholeDay]))}</b></span><span>有效使用 <b data-testid="scope-effective">{elapsed(total)}</b></span><span>已选 <b data-testid="selected-effective">{elapsed(selectedTime)}</b></span></div><button disabled={!selected.length && !scope} onClick={() => { setScope(null); setSelected([]); }}>清除选择</button></div>
    <div className="tw-actions"><select aria-label="归入分类" value={category} onChange={e => setCategory(e.target.value)} disabled={busy}><option value="">选择分类</option><option value="0">未分类</option>{data.categories.map(c => <option key={c.id} value={c.id}>{c.name}</option>)}</select><button className="tw-primary" disabled={busy || !selectedTime || category === ""} onClick={() => void run("apply")}>归入分类</button><button disabled={busy || !selectedTime} onClick={() => void run("auto")}>恢复自动分类</button><button disabled={busy || !data.undoAvailable} onClick={() => void run("undo")}>撤销最近一次标注</button>{message && <span role="status">{message}</span>}</div>
    <div className="tw-table-wrap"><table className="tw-summary"><thead><tr><th><MarkBox ranges={eligible} selected={actualSelected} onToggle={toggle} label="选择范围内全部活动" disabled={!eligible.length || busy} /></th><th>{mode === "application" ? "程序" : "标签"}</th><th>时长</th><th>占比</th></tr></thead><tbody>{summary.map(row => <tr key={row.key}><td><MarkBox ranges={row.ranges} selected={actualSelected} onToggle={toggle} label={`选择${row.name}的时段`} disabled={busy} /></td><td><div className="tw-name">{mode === "application" ? <ApplicationIcon name={row.name} target={row.key} icons={icons} /> : <i style={{ background: row.color }} />}<span title={row.name}>{row.name}</span></div></td><td>{elapsed(row.milliseconds)}</td><td><div className="tw-share"><i style={{ width: `${total ? row.milliseconds / total * 100 : 0}%`, background: row.color }} /><span>{total ? (row.milliseconds / total * 100).toFixed(1) : "0.0"}%</span></div></td></tr>)}</tbody></table>{!summary.length && <p className="tw-empty">{!current || state === "loading" ? "加载中…" : "没有匹配记录"}</p>}</div>
    <details className="tw-details" open={detailOpen} onToggle={e => setDetailOpen(e.currentTarget.open)}><summary>活动明细 <span>{candidates.length}</span></summary>{detailOpen && <><div className="tw-detail-list">{candidates.slice(visiblePage * 30, visiblePage * 30 + 30).map(({ item, range }, i) => <label key={`${item.id}:${range.start}:${i}`} className="tw-detail"><MarkBox ranges={[range]} selected={actualSelected} onToggle={toggle} label={`选择 ${clock(range.start)} ${item.appName}`} disabled={busy} /><time>{clock(range.start)}–{clock(range.end)}</time><div><strong>{item.appName}</strong><span title={item.title}>{item.title || "无窗口标题"}</span></div><span>{item.category || "未分类"}</span><b>{elapsed(range.end - range.start)}</b></label>)}</div><footer><button disabled={!visiblePage} onClick={() => setPage(visiblePage - 1)}>上一页</button><span>{visiblePage + 1} / {Math.max(1, Math.ceil(candidates.length / 30))}</span><button disabled={(visiblePage + 1) * 30 >= candidates.length} onClick={() => setPage(visiblePage + 1)}>下一页</button></footer></>}</details>
  </section>;
}
