import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as api from "./api";
import type { ActivityStatistics, AppUsage, StatisticsExclusion, StatisticsKind, TimelineItem, TargetCatalog, UsageInsights, WebsitePageUsage } from "./types";
import { ApplicationIcon, useApplicationIcons } from "./components/ApplicationIcon";
import { TimelineWorkspace } from "./TimelineWorkspace";
import { DurationValue, TrendColumns, duration } from "./components/TrendChart";
import { usePolledQuery } from "./hooks/usePolledQuery";
import { TargetPicker } from "./components/TargetPicker";
import { chartColor, useTheme } from "./theme";
import { dateKey, dateFromKey, dayBounds, rangeBounds, shiftDate, statisticalDateKey, type RangeKind } from "./statisticsDates";
import { Icon } from "./components/Icon";
import { DeltaBadge, PageHeader, Segmented, StatTile } from "./components/ui";
import { ShareRing, type Segment } from "./components/charts";
import { compactDuration, monthDay, numberFormat } from "./format";

const DAY = 86_400_000;
export const emptyStats = (start: number, end: number): ActivityStatistics => ({ start, end, effectiveSeconds: 0, trackedSeconds: 0, effectiveMilliseconds: 0, trackedMilliseconds: 0, recordsCount: 0, appUsage: [], siteUsage: [], categoryUsage: [], trend: [], trendSeries: [], timeline: [] });
export const totalSeconds = (stats: ActivityStatistics) => (stats.effectiveMilliseconds ?? stats.effectiveSeconds * 1000) / 1000;
export const appTarget = (item: AppUsage) => item.target || item.appName;
const kindLabel = (kind: StatisticsKind) => kind === "application" ? "应用" : kind === "website" ? "网站" : "分类";
export const UNCLASSIFIED_COLOR = "#8e8e93";
export const categoryColor = (row: AppUsage) => row.appName === "未分类" ? UNCLASSIFIED_COLOR : row.color;
const rowMilliseconds = (row: AppUsage) => row.milliseconds ?? row.seconds * 1000;

export function friendlyApplicationRows(rows: AppUsage[], catalog: TargetCatalog): AppUsage[] {
  const names = new Map(catalog.applications.filter(app => app.source === "history").map(app => [app.executableName.toLowerCase(), app.displayName]));
  return rows.map(row => {
    const target = appTarget(row).toLowerCase();
    const base = target.replace(/\.exe$/i, "");
    const name = names.get(target);
    // Preserve meaningful recorded names and never change the stable target.
    return name && (row.appName.toLowerCase() === base || (target === "explorer.exe" && name === "Windows 资源管理器")) ? { ...row, appName: name } : row;
  });
}

export function friendlyExplorerTimeline(items: TimelineItem[], catalog: TargetCatalog): TimelineItem[] {
  const explorer = catalog.applications.find(app => app.source === "history" && app.executableName.toLowerCase() === "explorer.exe" && app.displayName === "Windows 资源管理器");
  if (!explorer) return items;
  return items.map(item => item.sourceKind !== "website" && item.target?.toLowerCase() === "explorer.exe" ? { ...item, appName: explorer.displayName } : item);
}

export function Ranking({ rows, kind, onOpen, page, setPage, pageSize = 25 }: { rows: AppUsage[]; kind: StatisticsKind; onOpen: (kind: StatisticsKind, row: AppUsage) => void; page: number; setPage: React.Dispatch<React.SetStateAction<number>>; pageSize?: number }) {
  const { resolved } = useTheme();
  const pages = Math.max(1, Math.ceil(rows.length / pageSize));
  useEffect(() => setPage(current => kind ? Math.min(current, pages - 1) : 0), [kind, pages]);
  const visibleRows = rows.slice(page * pageSize, (page + 1) * pageSize);
  const icons = useApplicationIcons(kind === "application" ? visibleRows.map(appTarget) : []);
  const max = rows[0] ? rowMilliseconds(rows[0]) : 0;
  if (!rows.length) return <div className="kd-empty"><Icon name={kind === "website" ? "globe" : kind === "category" ? "tag" : "apps"} size={26} strokeWidth={1.4}/>{kind === "website" ? "还没有网站记录" : "还没有记录"}</div>;
  return <div className="kd-ranking-wrap"><ol className="kd-ranking">{visibleRows.map(row => {
    const milliseconds = rowMilliseconds(row);
    const color = kind === "category" ? categoryColor(row) : row.color;
    return <li key={`${kind}-${appTarget(row)}`}><button type="button" onClick={() => onOpen(kind, row)}>
      {kind === "application" ? <ApplicationIcon name={row.appName} target={appTarget(row)} icons={icons}/> : kind === "website" ? <span className="kd-kind-mark website" aria-hidden="true">{row.appName.slice(0, 1).toUpperCase()}</span> : <span className="kd-kind-mark category" style={{ background: chartColor(color, resolved) }} aria-hidden="true"/>}
      <span className="kd-rank-copy"><strong title={row.appName}>{row.appName}</strong><i><span style={{ width: `${max ? Math.max(1.5, milliseconds / max * 100) : 0}%`, background: chartColor(color, resolved) }}/></i></span>
      <span className="kd-rank-value"><time>{compactDuration(milliseconds)}</time><small>{row.percent.toFixed(row.percent < 10 ? 1 : 0)}%</small></span>
      <Icon name="chevronRight" size={14} className="kd-rank-chevron"/>
    </button></li>;
  })}</ol>{pages > 1 && <footer className="kd-ranking-pages"><button disabled={!page} onClick={() => setPage(value => value - 1)}>上一页</button><span>{page + 1} / {pages}</span><button disabled={page + 1 >= pages} onClick={() => setPage(value => value + 1)}>下一页</button></footer>}</div>;
}

/** Ring + legend for category shares; folds anything past six into "其他". */
export function CategoryBreakdown({ rows, total, onOpen }: { rows: AppUsage[]; total: number; onOpen?: (row: AppUsage) => void }) {
  const { resolved } = useTheme();
  const segments = useMemo<Segment[]>(() => {
    const head = rows.slice(0, 6).map(row => ({ key: appTarget(row), name: row.appName, color: categoryColor(row), milliseconds: rowMilliseconds(row) }));
    const rest = rows.slice(6).reduce((sum, row) => sum + rowMilliseconds(row), 0);
    return rest > 0 ? [...head, { key: "__other", name: "其他", color: "#b0b0b5", milliseconds: rest }] : head;
  }, [rows]);
  if (!rows.length) return <div className="kd-empty compact"><Icon name="tag" size={24} strokeWidth={1.4}/>还没有分类记录</div>;
  const top = segments[0];
  return <div className="kd-breakdown">
    <ShareRing items={segments} total={total} center={<><strong>{total ? `${Math.round(top.milliseconds / total * 100)}%` : "—"}</strong><span>{top.name}</span></>}/>
    <ul className="kd-breakdown-list">{segments.map(segment => {
      const row = rows.find(item => appTarget(item) === segment.key);
      const content = <><i style={{ background: chartColor(segment.color, resolved) }}/><span>{segment.name}</span><time>{compactDuration(segment.milliseconds)}</time><small>{total ? `${(segment.milliseconds / total * 100).toFixed(0)}%` : ""}</small></>;
      return <li key={segment.key}>{row && onOpen ? <button type="button" onClick={() => onOpen(row)}>{content}</button> : <div>{content}</div>}</li>;
    })}</ul>
  </div>;
}

export type DetailTarget = { kind: StatisticsKind; target: string; name: string; color: string; source?: TimelineItem; pageKey?: string };

function DetailHeader({ target, onBack, onExclude, eyebrow }: { target: DetailTarget; onBack: () => void; onExclude?: () => void; eyebrow: string }) {
  const icons = useApplicationIcons(target.kind === "application" ? [target.target] : []);
  return <header className="kd-detail-header">
    <button type="button" className="kd-back" onClick={onBack}><Icon name="chevronLeft" size={16}/>返回</button>
    <div className="kd-detail-identity">
      {target.kind === "application" ? <ApplicationIcon name={target.name} target={target.target} icons={icons} className="kd-detail-icon"/> : <span className={`kd-detail-mark ${target.kind}`}><Icon name={target.kind === "website" ? "globe" : "tag"} size={20}/></span>}
      <div><span>{eyebrow}</span><h1 title={target.name}>{target.name}</h1></div>
    </div>
    {onExclude && <button type="button" className="kd-text-button kd-danger-text" onClick={onExclude}><Icon name="eyeOff" size={14}/>不计入统计</button>}
  </header>;
}

function DefaultCategory({ kind, target, onSaved }: { kind: "application" | "website"; target: string; onSaved: (message: string) => void }) {
  const [category, setCategory] = useState("");
  return <form className="kd-inline-form" onSubmit={async event => { event.preventDefault(); try { await api.setDefaultCategory(kind, target, category); onSaved("分类已保存"); } catch (reason) { onSaved(String(reason)); } }}>
    <label><span>默认分类</span><input value={category} onChange={event => setCategory(event.target.value)} placeholder="未分类"/></label><button type="submit">保存</button>
  </form>;
}

function DetailView({ target, bounds, bucket, onBack, onChanged }: { target: DetailTarget; bounds: { start: number; end: number }; bucket: "hour" | "day" | "month"; onBack: () => void; onChanged: () => void }) {
  const [page, setPage] = useState(0); const [refreshKey, setRefreshKey] = useState(0);
  const [search, setSearch] = useState(""); const [message, setMessage] = useState("");
  const [settledSearch, setSettledSearch] = useState(search);
  useEffect(() => { const timer = window.setTimeout(() => setSettledSearch(search), 180); return () => clearTimeout(timer); }, [search]);
  const { stats, error: summaryError } = useStats(bounds.start, bounds.end, bucket, refreshKey, true, false, target.kind, target.target, target.pageKey);
  const detailsQuery = useCallback(() => api.queryActivityDetails({ start: bounds.start, end: bounds.end, bucket, targetKind: target.kind, target: target.target, pageKey: target.pageKey, offset: page * 30, limit: 30, search: settledSearch }), [bounds.start, bounds.end, bucket, target.kind, target.target, target.pageKey, page, settledSearch]);
  const { data: details, error: detailsError } = usePolledQuery(detailsQuery, () => ({ items: [] as TimelineItem[], total: 0, offset: 0, limit: 30 }), { refreshKey, intervalMs: Date.now() >= bounds.start && Date.now() < bounds.end ? 5000 : 0 });
  const { items, total } = details;
  const load = () => setRefreshKey(value => value + 1);
  const exclude = async () => { if (target.kind === "category") return; try { await api.setStatisticsExcluded(target.kind, target.target, true); onChanged(); onBack(); } catch (reason) { setMessage(String(reason)); } };
  const override = async (item: TimelineItem, value: string | null) => { try { await api.setActivityCategory(item.sourceKind || "application", item.id, value, item.sourceIds); load(); onChanged(); } catch (reason) { setMessage(String(reason)); } };
  const timeFormat = new Intl.DateTimeFormat("zh-CN", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit", hour12: false });
  return <div className="kd-page kd-detail" data-testid="statistics-detail">
    <DetailHeader target={target} onBack={onBack} eyebrow={target.pageKey ? "页面" : kindLabel(target.kind)} onExclude={target.kind !== "category" && !target.pageKey ? exclude : undefined}/>
    {message && <button className="kd-message" onClick={() => setMessage("")}>{message}<Icon name="close" size={14}/></button>}
    {(summaryError || detailsError) && <button className="kd-message" onClick={load}>{summaryError || detailsError} · 点击重试</button>}
    <section className="kd-card kd-detail-summary">
      <div className="kd-detail-total"><span className="kd-eyebrow">有效使用</span><strong className="kd-hero-number"><DurationValue seconds={totalSeconds(stats)}/></strong>{target.kind !== "category" && !target.pageKey && <DefaultCategory kind={target.kind} target={target.target} onSaved={text => { setMessage(text); load(); onChanged(); }}/>}</div>
      <TrendColumns points={stats.trend} bucket={bucket} name={target.name} color={target.color} height={150}/>
    </section>
    <section className="kd-card kd-detail-list">
      <header className="kd-card-header"><h2>{target.pageKey ? "访问记录" : "活动记录"}</h2><small className="kd-count">{numberFormat.format(total)}</small><label className="kd-card-search"><span className="visually-hidden">搜索标题</span><input type="search" value={search} onChange={event => { setSearch(event.target.value); setPage(0); }} placeholder="搜索窗口标题"/></label></header>
      {items.length ? <ol>{items.map(item => <li key={`${item.sourceKind}-${item.id}-${item.startedAt}`}>
        <time className="kd-activity-time">{timeFormat.format(item.startedAt)}</time>
        <span className="kd-activity-copy"><strong>{item.appName}</strong><small title={item.title}>{item.title || "无窗口标题"}</small></span>
        <span className="kd-chip">{item.category}</span>
        <time className="kd-activity-duration">{duration((item.milliseconds ?? item.seconds * 1000) / 1000)}</time>
        <details className="kd-popover"><summary aria-label="更改分类"><Icon name="tag" size={14}/></summary><form onSubmit={event => { event.preventDefault(); const data = new FormData(event.currentTarget); void override(item, String(data.get("category"))); }}><label>这段记录的分类<input name="category" defaultValue={item.category === "未分类" ? "" : item.category}/></label><footer><button type="button" onClick={() => void override(item, null)}>恢复自动</button><button className="primary">保存</button></footer></form></details>
      </li>)}</ol> : <div className="kd-empty">没有匹配记录</div>}
      {total > 30 && <footer className="kd-pagination"><button disabled={!page} onClick={() => setPage(value => value - 1)}>上一页</button><span>{page + 1} / {Math.ceil(total / 30)}</span><button disabled={(page + 1) * 30 >= total} onClick={() => setPage(value => value + 1)}>下一页</button></footer>}
    </section>
  </div>;
}

function Exclusions({ onClose, onChanged }: { onClose: () => void; onChanged: () => void }) {
  const [items, setItems] = useState<StatisticsExclusion[]>([]); const [error, setError] = useState("");
  const [catalog, setCatalog] = useState<TargetCatalog>({ applications: [], websites: [], groups: [] });
  const [kind, setKind] = useState<"application" | "website">("application"); const [target, setTarget] = useState(""); const [busy, setBusy] = useState(false);
  const load = () => api.listStatisticsExclusions().then(setItems);
  useEffect(() => { void Promise.all([load(), api.getTargetCatalog().then(setCatalog)]).catch(reason => setError(String(reason))); }, []);
  const change = async (targetKind: "application" | "website", value: string, excluded: boolean) => { if (busy) return; setBusy(true); setError(""); try { await api.setStatisticsExcluded(targetKind, value, excluded); await load(); if (excluded) setTarget(""); onChanged(); } catch (reason) { setError(String(reason)); } finally { setBusy(false); } };
  return <div className="kd-page kd-excluded">
    <header className="kd-detail-header"><button className="kd-back" onClick={onClose}><Icon name="chevronLeft" size={16}/>返回</button></header>
    <PageHeader title="不计入统计" subtitle="排除的应用和网站保留原始记录，不影响每日额度与专注白名单。"/>
    <section className="kd-card">
      <form className="kd-exclusion-add" onSubmit={event => { event.preventDefault(); if (target) void change(kind, target, true); }}>
        <Segmented value={kind} onChange={value => { setKind(value); setTarget(""); }} label="排除目标类型" options={[{ value: "application", label: "应用" }, { value: "website", label: "网站" }]}/>
        <TargetPicker kind={kind} value={target} catalog={catalog} onChange={setTarget}/>
        <button type="submit" className="primary" disabled={!target || busy}>添加</button>
      </form>
      {error && <p role="alert" className="form-error">{error}</p>}
      {items.length ? <ul className="kd-plain-list">{items.map(item => <li key={`${item.targetKind}-${item.target}`}><span className="kd-detail-mark small"><Icon name={item.targetKind === "website" ? "globe" : "apps"} size={15}/></span><span><strong>{catalog.applications.find(app => app.executableName.toLowerCase() === item.target.toLowerCase())?.displayName ?? item.displayName}</strong><small>{kindLabel(item.targetKind)}</small></span><button disabled={busy} onClick={() => void change(item.targetKind, item.target, false)}>恢复统计</button></li>)}</ul> : <div className="kd-empty"><Icon name="eye" size={26} strokeWidth={1.4}/>所有应用和网站都在统计中</div>}
    </section>
  </div>;
}

export function useStats(start: number, end: number, bucket: "hour" | "day" | "month", refreshKey = 0, enabled = true, includeTimeline = true, targetKind?: StatisticsKind, target?: string, pageKey?: string) {
  const query = useCallback(() => api.queryStatistics({ start, end, bucket, targetKind, target, pageKey }, includeTimeline), [start, end, bucket, targetKind, target, pageKey, includeTimeline]);
  const result = usePolledQuery(query, () => emptyStats(start, end), { enabled, refreshKey, intervalMs: Date.now() >= start && Date.now() < end ? 5000 : 0 });
  return { stats: result.data, state: result.state, error: result.error, reload: result.reload };
}

function WebsiteDetail({ target, bounds, bucket, onBack, onChanged }: { target: DetailTarget; bounds: { start: number; end: number }; bucket: "hour" | "day" | "month"; onBack: () => void; onChanged: () => void }) {
  const [page, setPage] = useState(0); const [search, setSearch] = useState(""); const [settledSearch, setSettledSearch] = useState("");
  const [selectedPage, setSelectedPage] = useState<WebsitePageUsage | null>(null); const [message, setMessage] = useState(""); const [revision, setRevision] = useState(0);
  const scroll = useRef(0); const list = useRef<HTMLOListElement>(null);
  useEffect(() => { const timer = window.setTimeout(() => setSettledSearch(search), 180); return () => clearTimeout(timer); }, [search]);
  const query = useCallback(() => api.queryWebsitePages({ ...bounds, bucket }, target.target, page * 30, 30, settledSearch), [bounds.start, bounds.end, bucket, target.target, page, settledSearch]);
  const pages = usePolledQuery(query, () => ({ items: [] as WebsitePageUsage[], total: 0, offset: 0, limit: 30 }), { enabled: !selectedPage, refreshKey: revision, intervalMs: Date.now() >= bounds.start && Date.now() < bounds.end ? 5000 : 0 });
  const summary = useStats(bounds.start, bounds.end, bucket, revision, !selectedPage, false, "website", target.target);
  const changed = () => { setRevision(value => value + 1); onChanged(); };
  useEffect(() => { if (!selectedPage) requestAnimationFrame(() => { if (list.current) list.current.scrollTop = scroll.current; }); }, [selectedPage]);
  if (selectedPage) return <DetailView target={{ ...target, pageKey: selectedPage.pageKey, name: selectedPage.title || selectedPage.domain }} bounds={bounds} bucket={bucket} onBack={() => setSelectedPage(null)} onChanged={changed}/>;
  return <div className="kd-page kd-detail kd-website-detail" data-testid="website-detail">
    <DetailHeader target={target} onBack={onBack} eyebrow="网站" onExclude={async () => { try { await api.setStatisticsExcluded("website", target.target, true); onChanged(); onBack(); } catch (reason) { setMessage(String(reason)); } }}/>
    {message && <p role="status" className="kd-message">{message}</p>}
    {(summary.error || pages.error) && <button className="kd-message" onClick={() => { summary.reload(); pages.reload(); }}>{summary.error || pages.error} · 点击重试</button>}
    <section className="kd-card kd-detail-summary">
      <div className="kd-detail-total"><span className="kd-eyebrow">有效使用</span><strong className="kd-hero-number"><DurationValue seconds={totalSeconds(summary.stats)}/></strong><DefaultCategory kind="website" target={target.target} onSaved={text => { setMessage(text); changed(); }}/></div>
      <TrendColumns points={summary.stats.trend} bucket={bucket} name={target.name} color={target.color} height={150}/>
    </section>
    <section className="kd-card kd-detail-list kd-website-pages">
      <header className="kd-card-header"><h2>页面与视频</h2><small className="kd-count">{numberFormat.format(pages.data.total)}</small><label className="kd-card-search"><span className="visually-hidden">搜索页面</span><input type="search" value={search} placeholder="搜索页面或视频" onChange={event => { setSearch(event.target.value); setPage(0); scroll.current = 0; }}/></label></header>
      {pages.data.items.length ? <ol ref={list}>{pages.data.items.map(item => <li key={item.pageKey}><button type="button" className="kd-page-row" onClick={() => { scroll.current = list.current?.scrollTop ?? 0; setSelectedPage(item); }}><span className="kd-activity-copy"><strong>{item.title || item.domain}</strong><small>{item.domain}</small></span><small className="kd-muted">{item.visits} 次访问</small><time className="kd-activity-duration">{duration(item.milliseconds / 1000)}</time><Icon name="chevronRight" size={14} className="kd-rank-chevron"/></button></li>)}</ol> : <div className="kd-empty">{pages.state === "loading" ? "正在读取…" : search ? "没有匹配页面" : "没有页面记录"}</div>}
      {pages.data.total > 30 && <footer className="kd-pagination"><button disabled={!page} onClick={() => setPage(value => value - 1)}>上一页</button><span>{page + 1} / {Math.ceil(pages.data.total / 30)}</span><button disabled={(page + 1) * 30 >= pages.data.total} onClick={() => setPage(value => value + 1)}>下一页</button></footer>}
    </section>
  </div>;
}

export function TargetDetail(props: Parameters<typeof DetailView>[0]) { return props.target.kind === "website" ? <WebsiteDetail {...props}/> : <DetailView {...props}/>; }

const rangeOptions: { value: RangeKind; label: string }[] = [{ value: "day", label: "日" }, { value: "week", label: "7 天" }, { value: "month", label: "30 天" }, { value: "year", label: "年" }, { value: "custom", label: "自定义" }];
function rangeTitle(range: RangeKind, bounds: { start: number; end: number }) {
  if (range === "day") return new Intl.DateTimeFormat("zh-CN", { month: "long", day: "numeric", weekday: "long" }).format(bounds.start);
  if (range === "year") return `${new Date(bounds.start).getFullYear()} 年`;
  const startYear = new Date(bounds.start).getFullYear(), endYear = new Date(bounds.end - 1).getFullYear();
  const format = (time: number) => startYear !== endYear ? new Intl.DateTimeFormat("zh-CN", { year: "numeric", month: "long", day: "numeric" }).format(time) : monthDay(time);
  return `${format(bounds.start)} – ${format(bounds.end - 1)}`;
}

export function ReviewPage({ catalog, discoverApplications }: { catalog: TargetCatalog; discoverApplications: (targets: string[]) => void }) {
  const [section, setSection] = useState<"statistics" | "timeline">("statistics"); const [range, setRange] = useState<RangeKind>("week"); const [selectedDate, setSelected] = useState<string | null>(null);
  const [customStart, setCustomStart] = useState(shiftDate(dateKey(new Date()), -6)); const [customEnd, setCustomEnd] = useState(dateKey(new Date())); const [dayStartHour, setDayStartHour] = useState(0);
  const [currentDay, setCurrentDay] = useState(() => statisticalDateKey(dayStartHour));
  useEffect(() => { setCustomStart(value => value > currentDay ? currentDay : value); setCustomEnd(value => value > currentDay ? currentDay : value); }, [currentDay]);
  // A null selection follows the current statistical day; explicit dates stay put.
  // Only a changed day updates state, so this clock never changes query identity
  // on each tick or pulls a user who is browsing history back to today.
  useEffect(() => {
    const updateDay = () => { if (!document.hidden) setCurrentDay(statisticalDateKey(dayStartHour)); };
    updateDay();
    const timer = window.setInterval(updateDay, 5000);
    document.addEventListener("visibilitychange", updateDay);
    return () => { window.clearInterval(timer); document.removeEventListener("visibilitychange", updateDay); };
  }, [dayStartHour]);
  const selected = selectedDate ?? currentDay;
  const [rankingKind, setRankingKind] = useState<StatisticsKind>("application"); const [rankingPage, setRankingPage] = useState(0); const [trendTarget, setTrendTarget] = useState(""); const [detail, setDetail] = useState<DetailTarget | null>(null); const [excluded, setExcluded] = useState(false); const [refreshKey, setRefreshKey] = useState(0);
  useEffect(() => { api.getSettings().then(settings => setDayStartHour(settings.dayStartHour)).catch(() => {}); }, []);
  const bounds = useMemo(() => section === "timeline" ? dayBounds(selected, dayStartHour) : rangeBounds(range, selected, customStart, customEnd, dayStartHour, currentDay), [section, range, selected, customStart, customEnd, dayStartHour, currentDay]);
  const bucket: "hour" | "day" | "month" = section === "statistics" && range === "year" ? "month" : section === "timeline" || bounds.end - bounds.start <= 2 * DAY + 3_600_000 ? "hour" : bounds.end - bounds.start <= 90 * DAY + 3_600_000 ? "day" : "month";
  const statisticsVisible = !detail && !excluded && section === "statistics";
  const { stats, state, error, reload } = useStats(bounds.start, bounds.end, bucket, refreshKey, statisticsVisible, false);
  const insightsQuery = useCallback(() => api.queryInsights(bounds.start, bounds.end), [bounds.start, bounds.end]);
  const insights = usePolledQuery<UsageInsights | null>(insightsQuery, () => null, { enabled: statisticsVisible, refreshKey, intervalMs: Date.now() < bounds.end ? 60_000 : 0 });
  useEffect(() => discoverApplications(stats.appUsage.map(appTarget)), [stats.appUsage, discoverApplications]);
  const [targetKind, ...targetParts] = trendTarget.split(":");
  const selectedTrend = useStats(bounds.start, bounds.end, bucket, refreshKey, !!trendTarget && statisticsVisible, false, targetKind as StatisticsKind, targetParts.join(":"));
  const trend = trendTarget ? selectedTrend.stats.trend : stats.trend;
  const rankingScroll = useRef(0);
  const appRows = useMemo(() => friendlyApplicationRows(stats.appUsage, catalog), [stats.appUsage, catalog]);
  const rows = rankingKind === "application" ? appRows : rankingKind === "website" ? stats.siteUsage : stats.categoryUsage;
  const trendInfo = useMemo(() => {
    if (!trendTarget) return { name: "有效使用", color: "var(--kd-accent)" };
    const [kind, ...parts] = trendTarget.split(":");
    const target = parts.join(":");
    const candidates = kind === "application" ? appRows : kind === "website" ? stats.siteUsage : stats.categoryUsage;
    const row = candidates.find(item => appTarget(item) === target);
    return { name: row?.appName ?? target, color: row ? (kind === "category" ? categoryColor(row) : row.color) : "var(--kd-accent)" };
  }, [trendTarget, appRows, stats.siteUsage, stats.categoryUsage]);
  const open = (kind: StatisticsKind, row: AppUsage) => { rankingScroll.current = document.querySelector<HTMLElement>(".kd-review .kd-ranking")?.scrollTop ?? 0; setDetail({ kind, target: appTarget(row), name: row.appName, color: kind === "category" ? categoryColor(row) : row.color }); };
  useEffect(() => { if (!detail) requestAnimationFrame(() => { const list = document.querySelector<HTMLElement>(".kd-review .kd-ranking"); if (list) list.scrollTop = rankingScroll.current; }); }, [detail, rankingKind]);
  const shift = (direction: number) => { const date = dateFromKey(selected); if (section === "timeline") date.setDate(date.getDate() + direction); else if (range === "year") { date.setDate(1); date.setFullYear(date.getFullYear() + direction); } else if (range === "custom") { const length = Math.round((dateFromKey(customEnd).getTime() - dateFromKey(customStart).getTime()) / DAY) + 1; const nextEnd = shiftDate(customEnd, direction * length); const end = nextEnd > currentDay ? currentDay : nextEnd; setCustomEnd(end); setCustomStart(shiftDate(end, -(length - 1))); setSelected(end); return; } else date.setDate(date.getDate() + direction * (range === "week" ? 7 : range === "month" ? 30 : 1)); setSelected(dateKey(date) > currentDay ? currentDay : dateKey(date)); };
  if (detail) return <TargetDetail target={detail} bounds={bounds} bucket={bucket} onBack={() => setDetail(null)} onChanged={() => setRefreshKey(value => value + 1)}/>;
  if (excluded) return <Exclusions onClose={() => setExcluded(false)} onChanged={() => setRefreshKey(value => value + 1)}/>;
  const total = totalSeconds(stats) * 1000;
  const data = insights.data;
  const activeDays = data?.activeDays ?? 0;
  const atLatest = (section === "statistics" && range === "custom" ? customEnd : selected) >= currentDay;
  const loading = state === "loading" && !total;
  return <div className="kd-page kd-review" data-testid="review-page">
    <PageHeader title="回顾" subtitle={rangeTitle(section === "timeline" ? "day" : range, bounds)} actions={<>
      <button className="kd-text-button" onClick={() => setExcluded(true)}><Icon name="eyeOff" size={14}/>不计入统计</button>
      <Segmented value={section} onChange={setSection} label="回顾内容" options={[{ value: "statistics", label: "统计" }, { value: "timeline", label: "时间轴" }]}/>
    </>}/>
    <div className="kd-toolbar">
      {section === "statistics" && <Segmented value={range} onChange={setRange} label="统计范围" options={rangeOptions}/>}
      {section === "statistics" && range === "custom" ? <div className="kd-custom-range"><input aria-label="开始日期" type="date" max={customEnd < currentDay ? customEnd : currentDay} value={customStart} onChange={event => { const value = event.target.value; if (value && value <= customEnd && value <= currentDay) setCustomStart(value); }}/><span>至</span><input aria-label="结束日期" type="date" min={customStart} max={currentDay} value={customEnd} onChange={event => { const value = event.target.value; if (value && value >= customStart && value <= currentDay) setCustomEnd(value); }}/></div> : null}
      <div className="kd-date-nav">
        <button className="kd-icon-control" aria-label="上一段" onClick={() => shift(-1)}><Icon name="chevronLeft" size={16}/></button>
        {(section === "timeline" || range !== "custom") && <input aria-label="选择日期" type="date" max={currentDay} value={selected} onChange={event => { if (event.target.value && event.target.value <= currentDay) setSelected(event.target.value); }}/>}
        <button className="kd-icon-control" aria-label="下一段" disabled={atLatest} onClick={() => shift(1)}><Icon name="chevronRight" size={16}/></button>
        <button disabled={atLatest && selectedDate === null} onClick={() => { setCurrentDay(statisticalDateKey(dayStartHour)); setSelected(null); }}>今天</button>
      </div>
    </div>
    {section === "statistics" ? <>
      {state === "error" && <button className="kd-message" onClick={() => void reload()}>{error} · 点击重试</button>}
      {trendTarget && selectedTrend.error && <button className="kd-message" onClick={selectedTrend.reload}>{selectedTrend.error} · 点击重试</button>}
      <div className="kd-kpis">
        <div className="kd-kpi kd-kpi-primary"><span className="kd-eyebrow">有效使用</span><strong className="kd-hero-number">{loading ? "—" : <DurationValue seconds={total / 1000}/>}</strong>{data && <DeltaBadge current={data.effectiveMilliseconds} previous={data.previousEffectiveMilliseconds} label="较上一周期"/>}</div>
        <StatTile icon="calendar" label="日均" value={range === "day" ? "—" : activeDays ? compactDuration(total / Math.max(1, activeDays)) : "—"} detail={range === "day" ? "单日视图" : `按 ${activeDays} 个活跃日`}/>
        <StatTile icon="bolt" label="活跃天数" value={data ? `${activeDays} / ${data.dayCount}` : "—"} detail="使用超过 1 分钟"/>
        <StatTile icon="switch" label="应用切换" value={data ? `${data.switchesPerHour.toFixed(0)} 次/时` : "—"} detail={data ? `共 ${numberFormat.format(data.switches)} 次` : undefined}/>
        <StatTile icon="flame" label="最长连续使用" value={data?.longestStretch ? compactDuration(data.longestStretch.milliseconds) : "—"} detail={data?.longestStretch ? new Intl.DateTimeFormat("zh-CN", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit", hour12: false }).format(data.longestStretch.startedAt) : undefined}/>
      </div>
      <section className="kd-card kd-trend-card">
        <header className="kd-card-header"><h2>{trendInfo.name}</h2><span className="kd-card-hint">{range === "year" ? "点按月份查看该月" : "点按柱形查看当天时间轴"}</span>
          <label className="kd-card-select"><span className="visually-hidden">趋势项目</span><select aria-label="趋势项目" value={trendTarget} onChange={event => setTrendTarget(event.target.value)}><option value="">总有效使用</option><optgroup label="应用">{appRows.slice(0, 40).map(item => <option key={`application:${appTarget(item)}`} value={`application:${appTarget(item)}`}>{item.appName}</option>)}</optgroup>{stats.siteUsage.length > 0 && <optgroup label="网站">{stats.siteUsage.slice(0, 40).map(item => <option key={`website:${appTarget(item)}`} value={`website:${appTarget(item)}`}>{item.appName}</option>)}</optgroup>}{stats.categoryUsage.length > 0 && <optgroup label="分类">{stats.categoryUsage.map(item => <option key={`category:${appTarget(item)}`} value={`category:${appTarget(item)}`}>{item.appName}</option>)}</optgroup>}</select></label>
        </header>
        <TrendColumns points={trend} bucket={bucket} name={trendInfo.name} color={trendInfo.color} height={200} onPoint={point => { setSelected(dateKey(new Date(point.startedAt))); if (range === "year") { const first = new Date(point.startedAt); const last = new Date(first.getFullYear(), first.getMonth() + 1, 0); setCustomStart(dateKey(first)); setCustomEnd(dateKey(last) > currentDay ? currentDay : dateKey(last)); setRange("custom"); setTrendTarget(""); } else setSection("timeline"); }}/>
      </section>
      <div className="kd-review-grid">
        <section className="kd-card"><header className="kd-card-header"><h2>分类</h2><span>{stats.categoryUsage.length} 项</span></header><CategoryBreakdown rows={stats.categoryUsage} total={total} onOpen={row => open("category", row)}/></section>
        <section className="kd-card kd-rank-panel"><header className="kd-card-header"><Segmented value={rankingKind} onChange={value => { setRankingKind(value); setRankingPage(0); }} label="排行" options={[{ value: "application", label: "应用" }, { value: "website", label: "网站" }, { value: "category", label: "分类" }]}/><span>{rows.length} 项</span></header><Ranking rows={rows} kind={rankingKind} onOpen={open} page={rankingPage} setPage={setRankingPage}/></section>
      </div>
    </> : <TimelineWorkspace start={bounds.start} end={bounds.end} catalog={catalog} discoverApplications={discoverApplications} onChanged={() => setRefreshKey(value => value + 1)}/>}
  </div>;
}
