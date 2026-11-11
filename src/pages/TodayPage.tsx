import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as api from "../api";
import type { AppUsage, Dashboard, LimitRule, StatisticsKind, TargetCatalog, TimelineItem, UsageInsights } from "../types";
import { ApplicationIcon, useApplicationIcons } from "../components/ApplicationIcon";
import { InteractiveTimeline } from "../components/InteractiveTimeline";
import { DurationValue } from "../components/TrendChart";
import { StackedColumns, Legend, Meter, type Column } from "../components/charts";
import { DeltaBadge, PageHeader, Segmented, StatTile } from "../components/ui";
import { Icon } from "../components/Icon";
import { usePolledQuery } from "../hooks/usePolledQuery";
import { queryFocusStatistics, type FocusStatistics } from "../focusStatisticsApi";
import { CategoryBreakdown, Ranking, TargetDetail, UNCLASSIFIED_COLOR, appTarget, categoryColor, friendlyApplicationRows, friendlyExplorerTimeline, totalSeconds, useStats, type DetailTarget } from "../StatisticsPages";
import { clockTime, compactDuration, greeting, monthDay, numberFormat, weekday } from "../format";

const HOUR = 3_600_000;
const MAX_STACKED = 6;

/** 24 hourly columns stacked by category, in the day's overall category order. */
function hourlyColumns(items: TimelineItem[], start: number, categories: AppUsage[], now: number): { columns: Column[]; legend: { key: string; name: string; color: string }[] } {
  const order = categories.slice(0, MAX_STACKED).map(row => row.appName);
  const colors = new Map(categories.map(row => [row.appName, categoryColor(row)]));
  const keyFor = (category: string) => order.includes(category) ? category : "其他";
  const buckets = Array.from({ length: 24 }, () => new Map<string, number>());
  for (const item of items) {
    if (item.idle) continue;
    const category = keyFor(item.category || "未分类");
    for (let cursor = Math.max(item.startedAt, start); cursor < item.endedAt;) {
      const index = Math.floor((cursor - start) / HOUR);
      if (index < 0 || index >= 24) break;
      const until = Math.min(item.endedAt, start + (index + 1) * HOUR);
      buckets[index].set(category, (buckets[index].get(category) ?? 0) + until - cursor);
      cursor = until;
    }
  }
  const keys = [...order, ...(buckets.some(bucket => bucket.has("其他")) ? ["其他"] : [])];
  const colorOf = (key: string) => key === "其他" ? "#b0b0b5" : key === "未分类" ? UNCLASSIFIED_COLOR : colors.get(key) ?? "#8e8e93";
  const columns = buckets.map((bucket, index) => {
    const from = start + index * HOUR, hour = new Date(from).getHours();
    return {
      key: String(index), label: String(hour), tooltipTitle: `${String(hour).padStart(2, "0")}:00–${String((hour + 1) % 24).padStart(2, "0")}:00`,
      segments: keys.map(key => ({ key, name: key, color: colorOf(key), milliseconds: bucket.get(key) ?? 0 })),
      highlight: from <= now && now < from + HOUR, muted: from > now,
    };
  });
  return { columns, legend: keys.filter(key => buckets.some(bucket => bucket.has(key))).map(key => ({ key, name: key, color: colorOf(key) })) };
}

function NowCard({ dashboard, focus, focusAction, openFocus }: { dashboard: Dashboard; focus: Dashboard["focus"]; focusAction: TodayPageProps["focusAction"]; openFocus: () => void }) {
  const current = dashboard.currentActivity;
  const icons = useApplicationIcons(current ? [current.processPath || current.appName] : []);
  const [now, setNow] = useState(Date.now);
  useEffect(() => { const timer = window.setInterval(() => setNow(Date.now()), 1000); return () => clearInterval(timer); }, []);
  if (focus) {
    const remaining = focus.timerKind === "countdown" ? Math.max(0, focus.remainingMilliseconds ?? 0) - (focus.status === "focusing" ? Math.max(0, now - focus.observedAt) : 0) : null;
    const elapsed = focus.elapsedMilliseconds + (focus.status === "focusing" ? Math.max(0, now - focus.observedAt) : 0);
    const ratio = focus.durationSeconds ? 1 - Math.max(0, remaining ?? 0) / (focus.durationSeconds * 1000) : 0;
    const clock = (ms: number) => { const s = Math.max(0, Math.ceil(ms / 1000)); const h = Math.floor(s / 3600); return `${h ? `${h}:` : ""}${String(Math.floor(s / 60) % 60).padStart(h ? 2 : 1, "0")}:${String(s % 60).padStart(2, "0")}`; };
    return <section className="kd-card kd-now is-focus">
      <header><span className="kd-live-dot is-focus"/><span className="kd-eyebrow">{focus.status === "away" ? "专注已暂停" : "正在专注"}</span><small>{focus.mode === "strict" ? "严格模式" : "普通模式"}</small></header>
      <div className="kd-now-focus"><strong className="kd-number">{clock(remaining ?? elapsed)}</strong><span>{focus.label}</span></div>
      {focus.timerKind === "countdown" && <Meter value={ratio} max={1} label="专注进度"/>}
      <footer>
        {focus.status === "away" ? <button className="primary" onClick={() => focusAction("return")}>继续专注</button> : focus.mode === "normal" && <button onClick={() => focusAction("leave")} disabled={focus.leaveRemainingMilliseconds <= 0}><Icon name="pause" size={13}/>暂停</button>}
        <button onClick={() => focusAction("end")}>{focus.timerKind === "countup" ? "完成" : "提前完成"}</button>
        <button className="kd-text-button" onClick={openFocus}>打开计时<Icon name="arrowUpRight" size={13}/></button>
      </footer>
    </section>;
  }
  return <section className="kd-card kd-now">
    <header><span className={`kd-live-dot ${current?.idle ? "is-idle" : ""}`}/><span className="kd-eyebrow">{!current ? "等待记录" : current.idle ? "闲置中" : "正在使用"}</span>{current?.idle && <small>{compactDuration(current.idleSeconds * 1000)}</small>}</header>
    {current ? <div className="kd-now-app">
      <ApplicationIcon name={current.appName} target={current.processPath || current.appName} icons={icons} className="kd-now-icon"/>
      <div><strong title={current.appName}>{current.appName}</strong><small title={current.title}>{current.title || "无窗口标题"}</small></div>
    </div> : <p className="kd-muted">刻度会在后台记录前台应用。</p>}
    <footer><button className="primary" onClick={openFocus}><Icon name="focus" size={14}/>开始专注</button></footer>
  </section>;
}

function QuotaList({ rules }: { rules: LimitRule[] }) {
  const enabled = rules.filter(rule => rule.enabled).sort((a, b) => a.remainingSeconds / Math.max(1, a.dailyLimitMinutes * 60) - b.remainingSeconds / Math.max(1, b.dailyLimitMinutes * 60));
  if (!enabled.length) return <div className="kd-empty compact"><Icon name="limits" size={24} strokeWidth={1.4}/>没有启用的每日额度</div>;
  return <ul className="kd-quota-list">{enabled.slice(0, 5).map(rule => {
    const limit = rule.dailyLimitMinutes * 60, used = Math.max(0, rule.usedSeconds), exhausted = rule.remainingSeconds <= 0;
    const tone = exhausted ? "var(--kd-danger-fill)" : rule.remainingSeconds < limit * .2 ? "var(--kd-warning-fill)" : undefined;
    return <li key={rule.id}><div><strong>{rule.name}</strong><span className={exhausted ? "is-danger" : ""}>{exhausted ? "已用完" : `剩 ${compactDuration(rule.remainingSeconds * 1000)}`}</span></div><Meter value={used} max={limit} color={tone} label={`${rule.name}今日用量`}/><small>{compactDuration(used * 1000)} / {rule.dailyLimitMinutes} 分钟</small></li>;
  })}</ul>;
}

export type TodayPageProps = { dashboard: Dashboard; catalog: TargetCatalog; discoverApplications: (targets: string[]) => void; rules: LimitRule[]; focus: Dashboard["focus"]; openFocus: () => void; openRules: () => void; focusAction: (action: "leave" | "return" | "end" | "cancelEnd") => void };

export function TodayPage({ dashboard, catalog, discoverApplications, rules, focus, openFocus, openRules, focusAction }: TodayPageProps) {
  // Keep query identity stable while results render; advance only at the day boundary.
  const bounds = useMemo(() => { const end = new Date(dashboard.dayStartedAt); end.setDate(end.getDate() + 1); return { start: dashboard.dayStartedAt, end: end.getTime() }; }, [dashboard.dayStartedAt]);
  const [refreshKey, setRefreshKey] = useState(0);
  const [rankingKind, setRankingKind] = useState<StatisticsKind>("application"); const [rankingPage, setRankingPage] = useState(0); const [detail, setDetail] = useState<DetailTarget | null>(null);
  const { stats, state, error, reload } = useStats(bounds.start, bounds.end, "hour", refreshKey, !detail);
  const insightsQuery = useCallback(() => api.queryInsights(bounds.start, bounds.end), [bounds.start, bounds.end]);
  const insights = usePolledQuery<UsageInsights | null>(insightsQuery, () => null, { enabled: !detail, refreshKey, intervalMs: 30_000 });
  // A finished round changes the session status; re-query so the total updates at once.
  const focusKey = `${focus?.id ?? 0}-${focus?.status ?? "none"}`;
  const focusQuery = useCallback(() => queryFocusStatistics(bounds), [bounds, focusKey]);
  const focusStats = usePolledQuery<FocusStatistics | null>(focusQuery, () => null, { enabled: !detail, refreshKey, intervalMs: 30_000 });
  useEffect(() => discoverApplications(stats.appUsage.map(appTarget)), [stats.appUsage, discoverApplications]);
  const rankingScroll = useRef(0);
  const appRows = useMemo(() => friendlyApplicationRows(stats.appUsage, catalog), [stats.appUsage, catalog]);
  const timeline = useMemo(() => friendlyExplorerTimeline(stats.timeline, catalog), [stats.timeline, catalog]);
  const now = dashboard.generatedAt;
  const hourly = useMemo(() => hourlyColumns(timeline, bounds.start, stats.categoryUsage, now), [timeline, bounds.start, stats.categoryUsage, now]);
  const rows = rankingKind === "application" ? appRows : rankingKind === "website" ? stats.siteUsage : stats.categoryUsage;
  const open = (kind: StatisticsKind, row: AppUsage) => { rankingScroll.current = document.querySelector<HTMLElement>(".kd-today .kd-ranking")?.scrollTop ?? 0; setDetail({ kind, target: appTarget(row), name: row.appName, color: kind === "category" ? categoryColor(row) : row.color }); };
  useEffect(() => { if (!detail) requestAnimationFrame(() => { const list = document.querySelector<HTMLElement>(".kd-today .kd-ranking"); if (list) list.scrollTop = rankingScroll.current; }); }, [detail, rankingKind]);
  if (detail) return <TargetDetail target={detail} bounds={bounds} bucket="hour" onBack={() => setDetail(null)} onChanged={() => setRefreshKey(value => value + 1)}/>;

  const total = totalSeconds(stats) * 1000;
  const data = insights.data;
  const top = appRows[0];
  const date = new Date(bounds.start + 12 * HOUR);
  const focusMs = focusStats.data?.range.milliseconds ?? 0;
  const subtitle = total < 60_000 ? `${greeting()}，今天还没有太多记录。` : `${greeting()}，今天已有效使用 ${compactDuration(total)}${top ? `，最常用的是 ${top.appName}` : ""}。`;
  return <div className="kd-page kd-today" data-testid="today-page">
    <PageHeader eyebrow={weekday(date)} title={monthDay(date)} subtitle={subtitle} actions={<button className="primary kd-button-large" onClick={openFocus}><Icon name="focus" size={15}/>开始专注</button>}/>
    {state === "error" && <button className="kd-message" onClick={() => void reload()}>{error} · 点击重试</button>}
    <div className="kd-today-hero">
      <section className="kd-card kd-hero">
        <div className="kd-hero-main">
          <span className="kd-eyebrow">今日有效使用</span>
          <strong className="kd-hero-number">{state === "loading" && !total ? "—" : <DurationValue seconds={total / 1000}/>}</strong>
          {data && <DeltaBadge current={data.effectiveMilliseconds} previous={data.previousEffectiveMilliseconds} label="较昨日同时段"/>}
        </div>
        <div className="kd-hero-tiles">
          <StatTile icon="focus" label="专注" value={focusMs ? compactDuration(focusMs) : "0分钟"} detail={focusStats.data ? `完成 ${focusStats.data.range.completedCount} 轮` : undefined} tone="var(--kd-accent)"/>
          <StatTile icon="flame" label="最长连续" value={data?.longestStretch ? compactDuration(data.longestStretch.milliseconds) : "—"} detail={data?.longestStretch ? `${clockTime(data.longestStretch.startedAt)} 开始` : "持续使用的最长时段"} tone="#eb6834"/>
          <StatTile icon="switch" label="应用切换" value={data ? numberFormat.format(data.switches) : "—"} detail={data && data.switchesPerHour ? `约 ${data.switchesPerHour.toFixed(0)} 次 / 小时` : "切换越少越专注"} tone="#6250d6"/>
          <StatTile icon="sunrise" label="开始于" value={data?.days[0]?.firstActivityAt ? clockTime(data.days[0].firstActivityAt) : "—"} detail={data?.days[0]?.lastActivityAt ? `最近活动 ${clockTime(data.days[0].lastActivityAt)}` : undefined} tone="#1baf7a"/>
        </div>
      </section>
      <NowCard dashboard={dashboard} focus={focus} focusAction={focusAction} openFocus={openFocus}/>
    </div>
    <section className="kd-card kd-hourly-card">
      <header className="kd-card-header"><h2>时段分布</h2>{data?.peakHour != null && <span className="kd-card-hint">高峰 {String(data.peakHour).padStart(2, "0")}:00</span>}<div className="kd-card-aside"><Legend items={hourly.legend}/></div></header>
      <StackedColumns columns={hourly.columns} fixedMax={HOUR} labelEvery={3} caption="今日每小时有效使用" height={150}/>
    </section>
    <InteractiveTimeline items={timeline} start={bounds.start} end={bounds.end} compact onSelect={item => setDetail({ kind: "application", target: item.target || item.appName, name: item.appName, color: item.color || "#2a78d6", source: item })}/>
    <div className="kd-today-grid">
      <section className="kd-card kd-rank-panel"><header className="kd-card-header"><Segmented value={rankingKind} onChange={value => { setRankingKind(value); setRankingPage(0); }} label="今日排行" options={[{ value: "application", label: "应用" }, { value: "website", label: "网站" }, { value: "category", label: "分类" }]}/><span>{rows.length} 项</span></header><Ranking rows={rows} kind={rankingKind} onOpen={open} page={rankingPage} setPage={setRankingPage} pageSize={12}/></section>
      <div className="kd-today-side">
        <section className="kd-card"><header className="kd-card-header"><h2>分类</h2></header><CategoryBreakdown rows={stats.categoryUsage} total={total} onOpen={row => open("category", row)}/></section>
        <section className="kd-card"><header className="kd-card-header"><h2>每日额度</h2><button className="kd-text-button" onClick={openRules}>管理</button></header><QuotaList rules={rules}/></section>
      </div>
    </div>
  </div>;
}
