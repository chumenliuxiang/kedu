import { memo, useMemo, useState, type ReactNode } from "react";
import { chartColor, useTheme } from "../theme";
import { compactDuration, shortDuration } from "../format";

export type Segment = { key: string; name: string; color: string; milliseconds: number };
export type Column = { key: string; label: string; tooltipTitle: string; segments: Segment[]; muted?: boolean; highlight?: boolean; onSelect?: () => void };

const niceSteps = [60_000, 5 * 60_000, 10 * 60_000, 15 * 60_000, 30 * 60_000, 3_600_000, 2 * 3_600_000, 3 * 3_600_000, 4 * 3_600_000, 6 * 3_600_000, 12 * 3_600_000];
function scaleFor(maximum: number, fixed?: number) {
  if (fixed) return { max: fixed, ticks: [0, fixed / 2, fixed] };
  const step = niceSteps.find(value => value * 2 >= maximum) ?? Math.ceil(maximum / 2 / 3_600_000) * 3_600_000;
  return { max: step * 2, ticks: [0, step, step * 2] };
}

/**
 * Stacked columns: hours of a day or days of a range. Each column is one hit
 * target; segments keep a 2px surface gap and only the top carries rounding.
 */
export const StackedColumns = memo(function StackedColumns({ columns, fixedMax, average, labelEvery = 1, caption, height = 168 }: { columns: Column[]; fixedMax?: number; average?: number; labelEvery?: number; caption: string; height?: number }) {
  const { resolved } = useTheme();
  const [hovered, setHovered] = useState<number | null>(null);
  const totals = useMemo(() => columns.map(column => column.segments.reduce((sum, segment) => sum + segment.milliseconds, 0)), [columns]);
  const scale = scaleFor(Math.max(1, ...totals, average ?? 0), fixedMax);
  const active = hovered === null ? null : columns[hovered];
  const left = hovered === null ? 0 : (hovered + .5) / columns.length * 100;
  return <figure className="kd-columns" style={{ ["--chart-height" as string]: `${height}px` }}>
    <div className="kd-columns-plot" onPointerLeave={() => setHovered(null)}>
      <div className="kd-columns-grid" aria-hidden="true">{scale.ticks.slice().reverse().map(tick => <span key={tick} style={{ bottom: `${tick / scale.max * 100}%` }}><em>{shortDuration(tick)}</em></span>)}</div>
      {average !== undefined && average > 0 && <div className="kd-columns-average" style={{ bottom: `${Math.min(100, average / scale.max * 100)}%` }} aria-hidden="true"><em>平均 {compactDuration(average)}</em></div>}
      <div className="kd-columns-bars">{columns.map((column, index) => {
        const total = totals[index];
        const ordered = column.segments.filter(segment => segment.milliseconds > 0);
        return <button key={column.key} type="button" className={`kd-column ${column.highlight ? "is-highlight" : ""} ${column.muted ? "is-muted" : ""} ${hovered === index ? "is-hovered" : ""}`} aria-label={`${column.tooltipTitle}，${compactDuration(total)}`} onPointerEnter={() => setHovered(index)} onFocus={() => setHovered(index)} onBlur={() => setHovered(null)} onClick={column.onSelect} tabIndex={column.onSelect ? 0 : -1}>
          <span className="kd-column-stack" style={{ height: `${Math.min(100, total / scale.max * 100)}%` }}>
            {ordered.map(segment => <i key={segment.key} style={{ flexGrow: segment.milliseconds, background: chartColor(segment.color, resolved) }}/>)}
          </span>
        </button>;
      })}</div>
      {active && <div className={`kd-chart-tip ${left > 70 ? "is-left" : left < 30 ? "is-right" : ""}`} style={{ left: `${left}%` }} role="status">
        <strong>{active.tooltipTitle}</strong><b>{compactDuration(totals[hovered!])}</b>
        {active.segments.filter(segment => segment.milliseconds >= 30_000).sort((a, b) => b.milliseconds - a.milliseconds).slice(0, 5).map(segment => <span key={segment.key}><i style={{ background: chartColor(segment.color, resolved) }}/>{segment.name}<em>{compactDuration(segment.milliseconds)}</em></span>)}
      </div>}
    </div>
    <div className="kd-columns-labels" aria-hidden="true">{columns.map((column, index) => <span key={column.key} className={column.highlight ? "is-highlight" : ""}>{index % labelEvery === 0 ? column.label : ""}</span>)}</div>
    <table className="visually-hidden"><caption>{caption}</caption><thead><tr><th>时间</th><th>时长</th></tr></thead><tbody>{columns.map((column, index) => <tr key={column.key}><th>{column.tooltipTitle}</th><td>{compactDuration(totals[index])}</td></tr>)}</tbody></table>
  </figure>;
});

export function Legend({ items }: { items: { key: string; name: string; color: string; value?: ReactNode }[] }) {
  const { resolved } = useTheme();
  if (items.length < 2) return null;
  return <ul className="kd-legend">{items.map(item => <li key={item.key}><i style={{ background: chartColor(item.color, resolved) }}/>{item.name}{item.value !== undefined && <em>{item.value}</em>}</li>)}</ul>;
}

/** Sequential single-hue scale: five steps by share of the busiest cell. */
export function heatLevel(value: number, maximum: number) {
  if (value <= 0 || maximum <= 0) return 0;
  const ratio = value / maximum;
  return ratio > .75 ? 4 : ratio > .5 ? 3 : ratio > .25 ? 2 : 1;
}
export function HeatLegend() {
  return <div className="kd-heat-legend" aria-hidden="true"><span>少</span>{[0, 1, 2, 3, 4].map(level => <i key={level} className={`kd-heat-${level}`}/>)}<span>多</span></div>;
}

const weekdays = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];
export const WeekHourHeatmap = memo(function WeekHourHeatmap({ heatmap, weeks }: { heatmap: number[][]; weeks: number }) {
  const [hovered, setHovered] = useState<{ day: number; hour: number } | null>(null);
  const maximum = Math.max(0, ...heatmap.flat());
  const value = hovered ? heatmap[hovered.day][hovered.hour] : 0;
  return <figure className="kd-weekheat">
    <div className="kd-weekheat-grid" onPointerLeave={() => setHovered(null)}>
      <span/>{Array.from({ length: 24 }, (_, hour) => <span key={hour} className="kd-weekheat-hour">{hour % 3 === 0 ? hour : ""}</span>)}
      {heatmap.map((row, day) => <div key={day} className="kd-weekheat-row" role="row">
        <span className="kd-weekheat-day">{weekdays[day]}</span>
        {row.map((cell, hour) => <i key={hour} role="gridcell" className={`kd-heat-${heatLevel(cell, maximum)} ${hovered?.day === day && hovered.hour === hour ? "is-hovered" : ""}`} aria-label={`${weekdays[day]} ${hour}时 ${compactDuration(cell)}`} onPointerEnter={() => setHovered({ day, hour })}/>)}
      </div>)}
    </div>
    <figcaption>
      <span role="status">{hovered ? <><b>{weekdays[hovered.day]} {String(hovered.hour).padStart(2, "0")}:00</b> · {weeks > 1 ? `平均 ${compactDuration(value / weeks)}` : compactDuration(value)}</> : "悬停查看各时段的使用时长"}</span>
      <HeatLegend/>
    </figcaption>
  </figure>;
});

export type CalendarDay = { key: string; date: Date; milliseconds: number; onSelect?: () => void };
/** GitHub-style calendar: weeks run left to right, Monday at the top. */
export const CalendarHeatmap = memo(function CalendarHeatmap({ days }: { days: CalendarDay[] }) {
  const [hovered, setHovered] = useState<CalendarDay | null>(null);
  const maximum = Math.max(0, ...days.map(day => day.milliseconds));
  const weeks: (CalendarDay | null)[][] = [];
  if (days.length) {
    let week: (CalendarDay | null)[] = Array((days[0].date.getDay() + 6) % 7).fill(null);
    for (const day of days) { week.push(day); if (week.length === 7) { weeks.push(week); week = []; } }
    if (week.length) weeks.push([...week, ...Array(7 - week.length).fill(null)]);
  }
  // Label the first column and every column containing the 1st of a month.
  const monthLabels = weeks.map((week, index) => {
    const marker = index === 0 ? week.find(Boolean) : week.find(day => day?.date.getDate() === 1);
    return marker ? `${marker.date.getMonth() + 1}月` : "";
  });
  return <figure className="kd-calendar">
    <div className="kd-calendar-scroll"><div className="kd-calendar-grid" style={{ gridTemplateColumns: `repeat(${weeks.length}, 12px)` }} onPointerLeave={() => setHovered(null)}>
      {monthLabels.map((label, index) => <span key={`m${index}`} className="kd-calendar-month" style={{ gridColumn: index + 1 }}>{label}</span>)}
      {weeks.map((week, column) => week.map((day, row) => day ? <button key={day.key} type="button" className={`kd-heat-${heatLevel(day.milliseconds, maximum)}`} style={{ gridColumn: column + 1, gridRow: row + 2 }} aria-label={`${day.key} ${compactDuration(day.milliseconds)}`} onPointerEnter={() => setHovered(day)} onFocus={() => setHovered(day)} onClick={day.onSelect}/> : <span key={`e${column}-${row}`} style={{ gridColumn: column + 1, gridRow: row + 2 }}/>))}
    </div></div>
    <figcaption><span role="status">{hovered ? <><b>{new Intl.DateTimeFormat("zh-CN", { month: "long", day: "numeric", weekday: "short" }).format(hovered.date)}</b> · {compactDuration(hovered.milliseconds)}</> : "每格代表一天，颜色越深使用越多"}</span><HeatLegend/></figcaption>
  </figure>;
});

/** Part-to-whole ring for at most six named shares plus "其他". */
export const ShareRing = memo(function ShareRing({ items, total, center, size = 148 }: { items: Segment[]; total: number; center: ReactNode; size?: number }) {
  const { resolved } = useTheme();
  const radius = 42, circumference = 2 * Math.PI * radius, gap = items.length > 1 ? 1.4 : 0;
  let offset = 0;
  return <div className="kd-ring" style={{ width: size, height: size }}>
    <svg viewBox="0 0 100 100" role="img" aria-label="占比">
      <circle cx="50" cy="50" r={radius} className="kd-ring-track"/>
      {total > 0 && items.map(item => {
        const length = item.milliseconds / total * circumference;
        const dash = Math.max(.01, length - gap);
        const element = <circle key={item.key} cx="50" cy="50" r={radius} fill="none" stroke={chartColor(item.color, resolved)} strokeWidth="10" strokeDasharray={`${dash} ${circumference - dash}`} strokeDashoffset={-offset} transform="rotate(-90 50 50)"><title>{item.name} · {compactDuration(item.milliseconds)}</title></circle>;
        offset += length;
        return element;
      })}
    </svg>
    <div className="kd-ring-center">{center}</div>
  </div>;
});

/** Thin horizontal meter, fully rounded, for quotas and shares. */
export function Meter({ value, max, color, label }: { value: number; max: number; color?: string; label: string }) {
  const { resolved } = useTheme();
  const ratio = max > 0 ? Math.min(1, value / max) : 0;
  return <span className="kd-meter" role="meter" aria-label={label} aria-valuemin={0} aria-valuemax={max} aria-valuenow={Math.min(value, max)}><i style={{ width: `${ratio * 100}%`, background: color ? chartColor(color, resolved) : undefined }}/></span>;
}
