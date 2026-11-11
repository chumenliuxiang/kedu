import { memo, useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent, MouseEvent, PointerEvent as ReactPointerEvent } from "react";
import type { TimelineItem } from "../types";
import { chartColor, seriesPalette, useTheme } from "../theme";

const colors = seriesPalette.map(([light]) => light);
const dateFormatter = new Intl.DateTimeFormat("zh-CN", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit", hour12: false });
const timeFormatter = new Intl.DateTimeFormat("zh-CN", { hour: "2-digit", minute: "2-digit", hour12: false });
const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));
const formatDuration = (seconds: number) => { const h = Math.floor(seconds / 3600), m = Math.floor(seconds % 3600 / 60); return h ? `${h}小时 ${m}分` : m ? `${m}分钟` : `${seconds}秒`; };
const colorFor = (name: string) => colors[[...name].reduce((hash, character) => (hash * 31 + (character.codePointAt(0) ?? 0)) >>> 0, 0) % colors.length];
const description = (item: TimelineItem) => `${dateFormatter.format(item.startedAt)} · ${item.appName} · ${item.title} · ${formatDuration(Math.round((item.milliseconds ?? item.seconds * 1000) / 1000))}`;
const descriptions = new WeakMap<TimelineItem, string>();
function cachedDescription(item: TimelineItem) {
  let text = descriptions.get(item);
  if (text === undefined) {
    text = description(item);
    descriptions.set(item, text);
  }
  return text;
}

type Segment = { item: TimelineItem; left: number; width: number; color: string; precedingRight: number };
type View = { zoom: number; offset: number };
const INITIAL_VIEW: View = { zoom: 1, offset: 0 };
const CANVAS_THRESHOLD = 800;

// Dense days retain every record without thousands of overlapping DOM buttons.
// Keyboard navigation and hit testing use the complete segment list below.
const SegmentLayer = memo(function SegmentLayer({ segments }: { segments: Segment[] }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const dense = segments.length > CANVAS_THRESHOLD;
  useLayoutEffect(() => {
    const element = canvas.current;
    if (!element || !dense) return;
    const draw = () => {
      const bounds = element.getBoundingClientRect();
      const scale = window.devicePixelRatio || 1;
      element.width = Math.round(bounds.width * scale);
      element.height = Math.round(bounds.height * scale);
      const context = element.getContext("2d");
      if (!context) return;
      context.scale(scale, scale);
      for (const segment of segments) {
        context.fillStyle = segment.color;
        context.globalAlpha = segment.item.idle ? .7 : 1;
        context.beginPath();
        context.roundRect(segment.left * bounds.width, 0, Math.max(2, segment.width * bounds.width), bounds.height, 2);
        context.fill();
      }
    };
    draw();
    const observer = new ResizeObserver(draw);
    observer.observe(element);
    return () => observer.disconnect();
  }, [dense, segments]);
  return <div className="timeline-canvas">
    {[0, 25, 50, 75, 100].map(left => <i key={left} className="timeline-grid" style={{ left: `${left}%` }} />)}
    {dense ? <canvas ref={canvas} aria-hidden="true" data-testid="dense-timeline" style={{ position: "absolute", inset: 0, width: "100%", height: "100%", zIndex: 1 }} /> : segments.map(({ item, left, width, color }, index) => { const text = cachedDescription(item); return <button key={`${item.id}-${item.startedAt}`} data-segment-index={index} tabIndex={-1} type="button" className={`timeline-segment ${item.idle ? "idle" : ""}`} aria-label={text} title={text} style={{ left: `${left * 100}%`, width: `${width * 100}%`, background: color }} />; })}
  </div>;
});

export const InteractiveTimeline = memo(function InteractiveTimeline({ items, start, end, compact = false, onSelect }: { items: TimelineItem[]; start: number; end: number; compact?: boolean; onSelect?: (item: TimelineItem) => void }) {
  const { resolved } = useTheme();
  const [view, setViewState] = useState<View>(INITIAL_VIEW);
  const viewRef = useRef(view);
  const frame = useRef<number | null>(null);
  const [dragging, setDragging] = useState(false);
  const [selectedIndex, setSelectedIndex] = useState(-1);
  const [hovered, setHovered] = useState<TimelineItem | null>(null);
  const hoveredRef = useRef<TimelineItem | null>(null);
  const hoverFrame = useRef<number | null>(null);
  const tipId = useId();
  const moved = useRef(false);
  const pressedItem = useRef<TimelineItem | null>(null);
  const drag = useRef<{ x: number; offset: number; width: number } | null>(null);
  const viewport = useRef<HTMLDivElement>(null);
  const range = Math.max(1, end - start);
  const { zoom, offset } = view;
  const visibleRange = range / zoom;
  const maxShift = Math.max(0, range - visibleRange);
  const viewStart = start + offset * maxShift;
  const sorted = useMemo(() => items.slice().sort((left, right) => left.startedAt - right.startedAt), [items]);
  const segments = useMemo(() => {
    let precedingRight = 0;
    const idle = getComputedStyle(document.documentElement).getPropertyValue("--kd-idle").trim() || "#c8d3d1";
    return sorted.filter(item => item.endedAt > viewStart && item.startedAt < viewStart + visibleRange).map(item => {
      const left = (Math.max(viewStart, item.startedAt) - viewStart) / visibleRange;
      const width = Math.max(.0012, (Math.min(viewStart + visibleRange, item.endedAt) - Math.max(viewStart, item.startedAt)) / visibleRange);
      precedingRight = Math.max(precedingRight, left + width);
      return { item, left, width, precedingRight, color: item.idle ? idle : chartColor(item.color || colorFor(item.target || item.appName), resolved) };
    });
  }, [sorted, viewStart, visibleRange, resolved]);

  const updateHovered = useCallback((next: TimelineItem | null, immediate = false) => {
    if (hoveredRef.current === next) return;
    hoveredRef.current = next;
    if (immediate) {
      if (hoverFrame.current !== null) cancelAnimationFrame(hoverFrame.current);
      hoverFrame.current = null;
      setHovered(next);
    } else if (hoverFrame.current === null) {
      hoverFrame.current = requestAnimationFrame(() => { hoverFrame.current = null; setHovered(hoveredRef.current); });
    }
  }, []);

  const updateView = useCallback((next: View, immediate = false) => {
    viewRef.current = next;
    if (immediate) {
      if (frame.current !== null) cancelAnimationFrame(frame.current);
      frame.current = null;
      setViewState(next);
    } else if (frame.current === null) {
      frame.current = requestAnimationFrame(() => { frame.current = null; setViewState(viewRef.current); });
    }
  }, []);
  const setView = useCallback((nextZoom: number, nextStart: number) => {
    const boundedZoom = clamp(nextZoom, 1, 16);
    const nextMaxShift = Math.max(0, range - range / boundedZoom);
    updateView({ zoom: boundedZoom, offset: nextMaxShift ? clamp((nextStart - start) / nextMaxShift, 0, 1) : 0 });
  }, [range, start, updateView]);
  const reset = useCallback(() => updateView(INITIAL_VIEW, true), [updateView]);
  useEffect(() => { reset(); setSelectedIndex(-1); updateHovered(null, true); }, [start, end, reset, updateHovered]);
  useEffect(() => () => {
    if (frame.current !== null) cancelAnimationFrame(frame.current);
    if (hoverFrame.current !== null) cancelAnimationFrame(hoverFrame.current);
  }, []);

  useEffect(() => {
    const element = viewport.current;
    if (!element) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      const rect = element.getBoundingClientRect();
      const ratio = clamp((event.clientX - rect.left) / Math.max(1, rect.width), 0, 1);
      const current = viewRef.current;
      const span = range / current.zoom;
      const anchor = start + current.offset * (range - span) + ratio * span;
      const nextZoom = clamp(current.zoom * Math.exp(-event.deltaY * .0018), 1, 16);
      setView(nextZoom, anchor - ratio * (range / nextZoom));
    };
    element.addEventListener("wheel", onWheel, { passive: false });
    return () => element.removeEventListener("wheel", onWheel);
  }, [range, start, setView, items.length > 0]);

  const itemAtPointer = (event: ReactPointerEvent<HTMLDivElement> | MouseEvent<HTMLDivElement>) => {
    const button = (event.target as HTMLElement).closest?.<HTMLElement>("[data-segment-index]");
    if (button) return segments[Number(button.dataset.segmentIndex)]?.item ?? null;
    if (segments.length <= CANVAS_THRESHOLD) return null;
    const rect = event.currentTarget.getBoundingClientRect();
    const x = event.clientX - rect.left;
    const ratio = x / Math.max(1, rect.width);
    let low = 0, high = segments.length;
    while (low < high) {
      const middle = (low + high) >>> 1;
      if (segments[middle].left <= ratio) low = middle + 1;
      else high = middle;
    }
    // Reverse paint order matches the button layer when subpixel records overlap.
    for (let index = low - 1; index >= 0; index--) {
      const segment = segments[index];
      if (ratio > Math.max(segment.precedingRight, segment.left + 2 / Math.max(1, rect.width))) break;
      const left = segment.left * rect.width;
      if (x >= left && x <= left + Math.max(2, segment.width * rect.width)) return segment.item;
    }
    return null;
  };
  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    // Pointer events preserve subpixel coordinates; the later mouse click can
    // round them and otherwise choose a neighbouring record on a dense day.
    pressedItem.current = itemAtPointer(event);
    drag.current = { x: event.clientX, offset: viewRef.current.offset, width: event.currentTarget.clientWidth || 1 };
    moved.current = false;
  };
  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!drag.current) { updateHovered(itemAtPointer(event)); return; }
    if (Math.abs(event.clientX - drag.current.x) <= 4 && !moved.current) return;
    if (!moved.current) { moved.current = true; event.currentTarget.setPointerCapture(event.pointerId); setDragging(true); updateHovered(null, true); }
    const current = viewRef.current;
    if (current.zoom <= 1) return;
    const delta = (drag.current.x - event.clientX) / drag.current.width;
    updateView({ zoom: current.zoom, offset: clamp(drag.current.offset + delta / (current.zoom - 1), 0, 1) });
  };
  const stopDrag = () => { drag.current = null; setDragging(false); };
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "+" || event.key === "=") { event.preventDefault(); const next = Math.min(16, zoom * 1.5); setView(next, viewStart + visibleRange / 2 - range / next / 2); }
    else if (event.key === "-") { event.preventDefault(); const next = Math.max(1, zoom / 1.5); setView(next, viewStart + visibleRange / 2 - range / next / 2); }
    else if (event.key === "ArrowLeft" || event.key === "ArrowRight") { event.preventDefault(); updateView({ zoom, offset: clamp(offset + (event.key === "ArrowLeft" ? -.1 : .1), 0, 1) }); }
    else if (event.key === "Home" || event.key === "0") { event.preventDefault(); reset(); }
    else if (event.key === "ArrowDown" || event.key === "ArrowUp") { event.preventDefault(); const index = clamp(selectedIndex + (event.key === "ArrowDown" ? 1 : -1), 0, segments.length - 1); setSelectedIndex(index); updateHovered(segments[index]?.item ?? null, true); }
    else if (event.key === "Enter" && segments[selectedIndex]) { event.preventDefault(); onSelect?.(segments[selectedIndex].item); }
  };
  const ticks = useMemo(() => {
    const firstDate = new Date(viewStart).toDateString();
    const nextDay = new Date(viewStart);
    nextDay.setDate(nextDay.getDate() + 1);
    const withinDay = visibleRange <= nextDay.getTime() - viewStart;
    return Array.from({ length: 5 }, (_, index) => {
      const time = viewStart + visibleRange * index / 4;
      const date = new Date(time);
      const nextMidnight = date.toDateString() !== firstDate && date.getHours() === 0 && date.getMinutes() === 0 && date.getSeconds() === 0;
      const label = !withinDay ? dateFormatter.format(time) : nextMidnight ? "24:00" : timeFormatter.format(time);
      return { time, label };
    });
  }, [viewStart, visibleRange]);

  return <section className={`kd-timeline ${compact ? "is-compact" : ""}`} data-testid="single-track-timeline" aria-label="应用时间轴">
    <header><h2>时间轴</h2><div className="timeline-legend" aria-label="时间轴图例"><span><i className="is-active" aria-hidden="true"/>活动</span><span><i className="is-idle" aria-hidden="true"/>闲置</span></div>{zoom > 1 && <button className="kd-text-button" onClick={reset}>复位</button>}</header>
    {items.length ? <>
      <div ref={viewport} className={`timeline-viewport ${dragging ? "dragging" : ""}`} tabIndex={0} role="region" aria-label={`活动时间轴，当前可见 ${segments.length} 段。加减键缩放，左右键移动，上下键选择活动，回车打开详情，Home 键复位。`} aria-describedby={hovered ? tipId : undefined} onDoubleClick={reset} onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerUp={stopDrag} onPointerCancel={stopDrag} onPointerLeave={() => { stopDrag(); updateHovered(null, true); }} onKeyDown={onKeyDown} onClick={event => { if (!moved.current) { const item = pressedItem.current ?? itemAtPointer(event); if (item) onSelect?.(item); } pressedItem.current = null; }}>
        <SegmentLayer segments={segments} />
      </div>
      <div className="timeline-labels">{ticks.map(tick => <span key={tick.time}>{tick.label}</span>)}</div>
    </> : <div className="timeline-empty">没有匹配记录</div>}
    {hovered && <div id={tipId} className="kd-timeline-tip" role="status"><strong>{hovered.appName}</strong><time>{dateFormatter.format(hovered.startedAt)} — {timeFormatter.format(hovered.endedAt)}</time><span>{hovered.title || "无窗口标题"}</span><b>{formatDuration(Math.round((hovered.milliseconds ?? hovered.seconds * 1000) / 1000))}</b></div>}
  </section>;
});
