import { useCallback, useEffect, useMemo, useState } from "react";
import * as api from "../api";
import type { TrendPoint, UsageInsights } from "../types";
import { usePolledQuery } from "../hooks/usePolledQuery";
import { CalendarHeatmap, StackedColumns, WeekHourHeatmap, type Column } from "../components/charts";
import { TrendColumns } from "../components/TrendChart";
import { DeltaBadge, PageHeader, Segmented, StatTile } from "../components/ui";
import { Icon } from "../components/Icon";
import { dateFromKey, rangeBounds, shiftDate, statisticalDateKey } from "../statisticsDates";
import { compactDuration, hourRange, minuteClock, monthDay, numberFormat, percent } from "../format";

type Span = "4w" | "12w" | "year";
const spans: { value: Span; label: string; days: number; title: string }[] = [
  { value: "4w", label: "4 周", days: 28, title: "过去 4 周" },
  { value: "12w", label: "12 周", days: 84, title: "过去 12 周" },
  { value: "year", label: "一年", days: 364, title: "过去一年" },
];
const weekdays = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];

type Observation = { icon: string; title: string; body: string; tone: string };
function observations(data: UsageInsights, weeks: number): Observation[] {
  const result: Observation[] = [];
  const active = data.days.filter(day => day.milliseconds >= 60_000);
  if (!active.length) return result;
  let best = { day: 0, hour: 0, value: 0 };
  data.heatmap.forEach((row, day) => row.forEach((value, hour) => { if (value > best.value) best = { day, hour, value }; }));
  if (best.value > 0) result.push({ icon: "clock", tone: "#2a78d6", title: `${weekdays[best.day]} ${hourRange(best.hour)}`, body: `是你最常使用电脑的时段，平均每周约 ${compactDuration(best.value / Math.max(1, weeks))}。` });
  const weekday = active.filter(day => { const value = (dateFromKey(day.label).getDay() + 6) % 7; return value < 5; });
  const weekend = active.filter(day => { const value = (dateFromKey(day.label).getDay() + 6) % 7; return value >= 5; });
  const mean = (items: typeof active) => items.reduce((sum, day) => sum + day.milliseconds, 0) / Math.max(1, items.length);
  if (weekday.length && weekend.length) {
    const ratio = mean(weekend) / Math.max(1, mean(weekday));
    result.push({ icon: "calendar", tone: "#6250d6", title: ratio > 1.1 ? "周末用得更多" : ratio < .9 ? "工作日用得更多" : "工作日与周末相近", body: `工作日平均 ${compactDuration(mean(weekday))}，周末平均 ${compactDuration(mean(weekend))}。` });
  }
  if (data.averageFirstMinute != null && data.averageLastMinute != null) {
    result.push({ icon: "sunrise", tone: "#1baf7a", title: `通常 ${minuteClock(data.averageFirstMinute)} 开始`, body: `平均在 ${minuteClock(data.averageLastMinute)} 前后结束一天的使用。` });
  }
  const busiest = active.reduce((top, day) => day.milliseconds > top.milliseconds ? day : top, active[0]);
  result.push({ icon: "flame", tone: "#eb6834", title: `${monthDay(dateFromKey(busiest.label))}用得最多`, body: `当天有效使用 ${compactDuration(busiest.milliseconds)}，是这段时间的最高纪录。` });
  if (data.longestSession?.name) result.push({ icon: "bolt", tone: "#e87ba4", title: `连续使用 ${data.longestSession.name} ${compactDuration(data.longestSession.milliseconds)}`, body: `这是这段时间里在单个应用中停留最久的一次，开始于 ${new Intl.DateTimeFormat("zh-CN", { month: "long", day: "numeric", hour: "2-digit", minute: "2-digit", hour12: false }).format(data.longestSession.startedAt)}。` });
  return result;
}

export function InsightsPage() {
  const [span, setSpan] = useState<Span>("4w");
  const [dayStartHour, setDayStartHour] = useState<number | null>(null);
  useEffect(() => { api.getSettings().then(settings => setDayStartHour(settings.dayStartHour)).catch(() => setDayStartHour(0)); }, []);
  const config = spans.find(item => item.value === span)!;
  const bounds = useMemo(() => {
    const hour = dayStartHour ?? 0, today = statisticalDateKey(hour);
    return rangeBounds("custom", today, shiftDate(today, -(config.days - 1)), today, hour, today);
  }, [config.days, dayStartHour]);
  const query = useCallback(() => api.queryInsights(bounds.start, bounds.end), [bounds.start, bounds.end]);
  const { data, state, error, reload } = usePolledQuery<UsageInsights | null>(query, () => null, { enabled: dayStartHour !== null, intervalMs: 5 * 60_000 });
  const weeks = config.days / 7;
  const notes = useMemo(() => data ? observations(data, weeks) : [], [data, weeks]);
  const dailyPoints = useMemo<TrendPoint[]>(() => (data?.days ?? []).map(day => ({ startedAt: day.startedAt, endedAt: day.endedAt, label: day.label, seconds: day.milliseconds / 1000, milliseconds: day.milliseconds })), [data]);
  const hourlyColumns = useMemo<Column[]>(() => (data?.hourly ?? []).map((value, hour) => ({ key: String(hour), label: String(hour), tooltipTitle: `${hourRange(hour)} · 每个活跃日平均`, segments: [{ key: "value", name: "平均", color: "var(--kd-accent)", milliseconds: value / Math.max(1, data?.activeDays ?? 1) }], highlight: hour === data?.peakHour })), [data]);
  const weekdayColumns = useMemo<Column[]>(() => {
    if (!data) return [];
    const totals = Array(7).fill(0), counts = Array(7).fill(0);
    for (const day of data.days) { const index = (dateFromKey(day.label).getDay() + 6) % 7; totals[index] += day.milliseconds; counts[index]++; }
    return weekdays.map((name, index) => ({ key: name, label: name.slice(1), tooltipTitle: `${name} · 平均`, segments: [{ key: "value", name: "平均", color: index >= 5 ? "#6250d6" : "var(--kd-accent)", milliseconds: totals[index] / Math.max(1, counts[index]) }] }));
  }, [data]);
  const calendarDays = useMemo(() => (data?.days ?? []).map(day => ({ key: day.label, date: dateFromKey(day.label), milliseconds: day.milliseconds })), [data]);
  const average = data && data.activeDays ? data.effectiveMilliseconds / data.activeDays : 0;
  const empty = data && data.effectiveMilliseconds < 60_000;

  return <div className="kd-page kd-insights" data-testid="insights-page">
    <PageHeader title="洞察" subtitle={`基于${config.title}的记录，了解你的使用节奏。`} actions={<Segmented value={span} onChange={setSpan} label="洞察范围" options={spans.map(item => ({ value: item.value, label: item.label }))}/>}/>
    {state === "error" && <button className="kd-message" onClick={reload}>{error} · 点击重试</button>}
    {empty ? <section className="kd-card kd-empty-hero"><Icon name="insights" size={34} strokeWidth={1.3}/><h2>还没有足够的记录</h2><p>保持刻度在后台运行几天，这里会出现你的使用节奏、高峰时段与习惯变化。</p></section> : <>
      <div className="kd-kpis">
        <div className="kd-kpi kd-kpi-primary"><span className="kd-eyebrow">日均有效使用</span><strong className="kd-hero-number kd-number">{data ? compactDuration(average) : "—"}</strong>{data && <DeltaBadge current={data.effectiveMilliseconds} previous={data.previousEffectiveMilliseconds} label={`较之前${config.label}`}/>}</div>
        <StatTile icon="calendar" label="活跃天数" value={data ? `${data.activeDays} 天` : "—"} detail={data ? `共 ${data.dayCount} 天 · ${percent(data.activeDays / Math.max(1, data.dayCount))}` : undefined}/>
        <StatTile icon="sunrise" label="平均开始" value={data?.averageFirstMinute != null ? minuteClock(data.averageFirstMinute) : "—"} detail={data?.averageLastMinute != null ? `平均结束 ${minuteClock(data.averageLastMinute)}` : undefined}/>
        <StatTile icon="clock" label="高峰时段" value={data?.peakHour != null ? hourRange(data.peakHour) : "—"} detail="全天最常使用的一小时"/>
        <StatTile icon="switch" label="应用切换" value={data ? `${data.switchesPerHour.toFixed(0)} 次/时` : "—"} detail={data ? `共 ${numberFormat.format(data.switches)} 次` : undefined}/>
      </div>
      {notes.length > 0 && <section className="kd-observations" aria-label="使用观察">{notes.map(note => <article key={note.title} className="kd-observation" style={{ ["--note-tone" as string]: note.tone }}><span><Icon name={note.icon} size={16} strokeWidth={1.9}/></span><div><h3>{note.title}</h3><p>{note.body}</p></div></article>)}</section>}
      <section className="kd-card">
        <header className="kd-card-header"><h2>一周节奏</h2><span className="kd-card-hint">按星期与小时汇总{weeks > 1 ? `，数值为每周平均` : ""}</span></header>
        {data ? <WeekHourHeatmap heatmap={data.heatmap} weeks={weeks}/> : <div className="kd-empty">{state === "loading" ? "正在分析…" : "没有数据"}</div>}
      </section>
      <div className="kd-insight-grid">
        <section className="kd-card"><header className="kd-card-header"><h2>一天中的分布</h2><span className="kd-card-hint">每个活跃日平均</span></header><StackedColumns columns={hourlyColumns} labelEvery={3} caption="每小时平均有效使用" height={150}/></section>
        <section className="kd-card"><header className="kd-card-header"><h2>星期分布</h2><span className="kd-card-hint">每天平均</span></header><StackedColumns columns={weekdayColumns} caption="各星期平均有效使用" height={150}/></section>
      </div>
      <section className="kd-card">
        <header className="kd-card-header"><h2>{span === "year" ? "全年日历" : "每日有效使用"}</h2>{data && <span className="kd-card-hint">合计 {compactDuration(data.effectiveMilliseconds)}</span>}</header>
        {span === "year" ? <CalendarHeatmap days={calendarDays}/> : <TrendColumns points={dailyPoints} bucket="day" height={190}/>}
      </section>
    </>}
  </div>;
}
