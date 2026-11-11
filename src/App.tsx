import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import * as api from "./api";
import type { ClassificationRule, Dashboard, FocusState, LimitRule, NewClassificationRule, NewLimitRule, RestrictionCause, RestrictionNotice, TargetCatalog } from "./types";
import { ApplicationIcon, useApplicationIcons } from "./components/ApplicationIcon";
import { RuleDialog } from "./components/RuleDialog";
import { ClassificationDialog } from "./components/ClassificationDialog";
import { LimitsWorkspace } from "./LimitsWorkspace";
import { ClassificationWorkspace } from "./ClassificationWorkspace";
import { ClassificationExchange, type ExchangeRequest } from "./ClassificationExchange";
import { ReviewPage } from "./StatisticsPages";
import { TodayPage } from "./pages/TodayPage";
import { InsightsPage } from "./pages/InsightsPage";
import { SettingsPage } from "./pages/SettingsPage";
import { FocusWorkspace, emptyFocusWorkspace, focusDisplay } from "./FocusWorkspace";
import { usePolledQuery } from "./hooks/usePolledQuery";
import { Icon } from "./components/Icon";
import { CommandPalette, type Command } from "./components/CommandPalette";
import { pages, type Page } from "./shortcuts";
import { toggleColorMode, useTheme, chooseTheme } from "./theme";
import { compactDuration } from "./format";

const formatDuration = (seconds: number) => compactDuration(seconds * 1000);
const clockText = (milliseconds: number) => { const seconds = Math.max(0, Math.ceil(milliseconds / 1000)); const hours = Math.floor(seconds / 3600); return `${hours ? `${hours}:` : ""}${String(Math.floor(seconds / 60) % 60).padStart(hours ? 2 : 1, "0")}:${String(seconds % 60).padStart(2, "0")}`; };

function useTick(enabled: boolean) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => { if (!enabled) return; const timer = window.setInterval(() => { if (!document.hidden) setNow(Date.now()); }, 1000); return () => clearInterval(timer); }, [enabled]);
  return now;
}

/** Sidebar footer: what is being recorded right now, or the running focus round. */
function LiveStatus({ dashboard, session, onOpenFocus, onOpenToday }: { dashboard: Dashboard; session: FocusState | null; onOpenFocus: () => void; onOpenToday: () => void }) {
  const current = dashboard.currentActivity;
  const icons = useApplicationIcons(current && !session ? [current.processPath || current.appName] : []);
  const now = useTick(Boolean(session));
  if (session) {
    const display = focusDisplay(session, now);
    const value = session.status === "resting" ? display.rest : session.status === "away" ? display.leave : display.remaining ?? display.elapsed;
    const label = session.status === "resting" ? "休息中" : session.status === "away" ? "暂停中" : "专注中";
    return <button type="button" className={`sb-live is-focus is-${session.status}`} onClick={onOpenFocus} title="打开计时">
      <span className="sb-live-ring"><Icon name="focus" size={15} strokeWidth={2}/></span>
      <span className="sb-live-copy"><small>{label}</small><b>{session.label}</b></span>
      <time className="kd-number">{clockText(value)}</time>
    </button>;
  }
  return <button type="button" className="sb-live" onClick={onOpenToday} title="查看今日">
    {current && !current.idle ? <ApplicationIcon name={current.appName} target={current.processPath || current.appName} icons={icons} className="sb-live-icon"/> : <span className="sb-live-ring is-idle"><Icon name="idle" size={15}/></span>}
    <span className="sb-live-copy"><small><i className={`kd-live-dot ${!current || current.idle ? "is-idle" : ""}`}/>{!current ? "等待记录" : current.idle ? "闲置中" : "正在记录"}</small><b>{current && !current.idle ? current.appName : `今日 ${formatDuration(dashboard.effectiveSeconds)}`}</b></span>
  </button>;
}

function App() {
  const [page, setPage] = useState<Page>("today");
  const [categoryRevision, setCategoryRevision] = useState(0);
  const [categoryRulesOpen, setCategoryRulesOpen] = useState(false);
  const [exchange, setExchange] = useState<ExchangeRequest | null>(null);
  const [error, setError] = useState("");
  const [restriction, setRestriction] = useState<RestrictionNotice | null>(null);
  const [timerRequest, setTimerRequest] = useState(0);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [editingRule, setEditingRule] = useState<LimitRule | null>(null);
  const [editingClassification, setEditingClassification] = useState<ClassificationRule | null>(null);
  const [targetCatalog, setTargetCatalog] = useState<TargetCatalog>({ applications: [], websites: [], groups: [] });
  const ruleDialog = useRef<HTMLDialogElement>(null);
  const classificationDialog = useRef<HTMLDialogElement>(null);
  const content = useRef<HTMLElement>(null);
  const { resolved } = useTheme();
  const queryDashboard = useCallback(async () => {
    const [dashboard, rules, classifications] = await Promise.all([api.getDashboard(), api.listRules(), api.listClassificationRules()]);
    return { dashboard, rules, classifications };
  }, []);
  const { data: snapshot, error: refreshError, reload: refresh } = usePolledQuery<Awaited<ReturnType<typeof queryDashboard>> | null>(queryDashboard, () => null, { intervalMs: 5000 });
  const [workspace, setWorkspace] = useState(emptyFocusWorkspace);
  const workspaceRequest = useRef(0);
  const focusQuery = useCallback(async () => {
    const request = ++workspaceRequest.current;
    const next = await api.getFocusWorkspace();
    // A poll started before a user action must not restore the previous phase.
    if (request === workspaceRequest.current) setWorkspace(next);
    return next;
  }, []);
  const { error: focusError, reload: reloadFocus } = usePolledQuery(focusQuery, emptyFocusWorkspace, { intervalMs: 5000 });
  const changed = useCallback(async () => { await focusQuery(); refresh(); }, [focusQuery, refresh]);
  const refreshCatalog = useCallback(() => api.getTargetCatalog().then(value => { setTargetCatalog(value); return true; }).catch(reason => { setError(String(reason)); return false; }), []);
  const [catalogReady, setCatalogReady] = useState(false);
  useEffect(() => { void refreshCatalog().then(() => setCatalogReady(true)); }, [refreshCatalog]);
  const catalogRequests = useRef(new Set<string>());
  const catalogTimer = useRef<number | undefined>(undefined);
  const pendingCatalogTargets = useRef(new Set<string>());
  useEffect(() => () => window.clearTimeout(catalogTimer.current), []);
  const discoverApplications = useCallback((targets: string[]) => {
    if (!catalogReady) return;
    const known = new Set(targetCatalog.applications.filter(app => app.source !== "installed").map(app => app.executableName.toLowerCase()));
    const missing = targets.map(value => value.toLowerCase()).filter(value => value && !known.has(value) && !catalogRequests.current.has(value));
    if (!missing.length) return;
    missing.forEach(value => { catalogRequests.current.add(value); pendingCatalogTargets.current.add(value); });
    if (catalogTimer.current !== undefined) return;
    catalogTimer.current = window.setTimeout(() => {
      catalogTimer.current = undefined;
      const batch = [...pendingCatalogTargets.current]; pendingCatalogTargets.current.clear();
      void refreshCatalog().then(success => { if (!success) window.setTimeout(() => batch.forEach(value => catalogRequests.current.delete(value)), 60_000); });
    }, 750);
  }, [catalogReady, targetCatalog, refreshCatalog]);
  const navigate = useCallback((next: Page) => { setPage(next); if (next !== "classification") setCategoryRulesOpen(false); content.current?.scrollTo({ top: 0 }); }, []);
  const openTimer = useCallback(() => { navigate("focus"); setTimerRequest(value => value + 1); }, [navigate]);
  useEffect(() => {
    if (!api.isDesktop()) {
      if (new URLSearchParams(location.search).has("restriction")) setRestriction({ appName: "微信", title: "微信", subjectKind: "application", subject: "WeChat.exe", causes: [{ ruleId: 0, ruleName: "学习", kind: "focus", usedSeconds: 0, limitSeconds: 0 }] });
      return;
    }
    let stopped = false;
    const receive = async () => { try { const notice = await api.takeRestrictionNotice(); if (!stopped && notice) { setRestriction(notice); reloadFocus(); } } catch (reason) { if (!stopped) setError(String(reason)); } };
    const subscriptions = [listen("restriction-triggered", () => void receive()), listen("focus-updated", () => { reloadFocus(); refresh(); }), listen("focus-started", () => { reloadFocus(); refresh(); })];
    // Subscribe before consuming a notice saved while this WebView was being created.
    void Promise.all(subscriptions).then(() => { if (!stopped) void receive(); });
    return () => { stopped = true; for (const pending of subscriptions) void pending.then(unlisten => unlisten()); };
  }, [reloadFocus, refresh]);

  // Desktop keyboard habits: Ctrl+number pages, Ctrl+K palette, Ctrl+, settings.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!event.ctrlKey || event.altKey || event.metaKey) return;
      const key = event.key.toLowerCase();
      if (key === "k") { event.preventDefault(); setPaletteOpen(open => !open); return; }
      if (document.querySelector("dialog[open]") && !paletteOpen) return;
      if (event.shiftKey && key === "l") { event.preventDefault(); toggleColorMode(); return; }
      if (key === ",") { event.preventDefault(); navigate("settings"); return; }
      const index = Number(event.key);
      if (Number.isInteger(index) && index >= 1 && index <= 6) { event.preventDefault(); navigate(pages[index - 1].id); }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navigate, paletteOpen]);

  const dashboard = snapshot?.dashboard;
  const rules = (snapshot?.rules ?? []).filter(rule => rule.targetKind !== "website");
  const classifications = snapshot?.classifications ?? [];
  const session = workspace.session;
  const focus = session && ["focusing", "away"].includes(session.status) ? session : null;
  const liveSession = session && session.status !== "completed" ? session : null;
  const focusAction = async (action: "leave" | "return" | "end" | "cancelEnd") => {
    if (!focus) return;
    try { if (action === "leave") await api.pauseFocus(focus.id); if (action === "return") await api.resumeFocus(focus.id); if (action === "end") { await api.completeFocus(focus.id); openTimer(); } await changed(); } catch (reason) { setError(String(reason)); }
  };
  const submitRule = async (rule: NewLimitRule, reminders: number[]) => { await api.saveRuleWithReminders(editingRule?.id ?? null, rule, reminders); ruleDialog.current?.close(); setEditingRule(null); refresh(); await refreshCatalog(); };
  const submitClassification = async (rule: NewClassificationRule) => {
    if (editingClassification) await api.updateClassificationRule(editingClassification.id, rule);
    else await api.createClassificationRule(rule);
    classificationDialog.current?.close(); setEditingClassification(null);
    refresh(); await refreshCatalog();
    setCategoryRevision(value => value + 1);
  };
  const toggleRule = async (id: number, enabled: boolean) => { try { const result = await api.setRuleEnabled(id, enabled); if (result.pending) setError("规则将在 30 秒后关闭"); refresh(); } catch (reason) { setError(String(reason)); } };
  const deleteRule = async (id: number) => { if (!window.confirm("删除这条限制规则？历史活动记录会保留。")) return; try { await api.deleteRule(id); refresh(); } catch (reason) { setError(String(reason)); } };
  const toggleClassification = async (id: number, enabled: boolean) => { try { await api.setClassificationRuleEnabled(id, enabled); refresh(); } catch (reason) { setError(String(reason)); } };
  const deleteClassification = async (id: number) => { if (!window.confirm("删除这条统计分类规则？")) return; try { await api.deleteClassificationRule(id); refresh(); } catch (reason) { setError(String(reason)); } };
  const grantEmergency = async (cause: RestrictionCause) => {
    if (!restriction) return;
    await api.grantException(cause.kind, cause.ruleId, 15, restriction.subjectKind ?? "application", restriction.subject ?? restriction.appName);
    setRestriction(current => { const causes = current?.causes.filter(item => !(item.ruleId === cause.ruleId && item.kind === cause.kind)) ?? []; return current && causes.length ? { ...current, causes } : null; });
    await changed();
  };
  const useWrapUp = async () => { if (!focus) return; await api.pauseFocus(focus.id); await changed(); setRestriction(current => { const causes = current?.causes.filter(item => item.kind !== "focus") ?? []; return current && causes.length ? { ...current, causes } : null; }); };
  const openRule = useCallback((rule: LimitRule | null) => { setEditingRule(rule); ruleDialog.current?.showModal(); }, []);

  const commands = useMemo<Command[]>(() => [
    ...pages.map(item => ({ id: `page-${item.id}`, title: item.label, subtitle: "前往页面", icon: item.icon, group: "页面", shortcut: item.shortcut, keywords: item.id, run: () => navigate(item.id) })),
    ...(liveSession ? [{ id: "timer", title: "返回计时", subtitle: liveSession.label, icon: "focus", group: "专注", run: openTimer }] : []),
    ...workspace.tasks.map(task => ({ id: `task-${task.id}`, title: `开始专注：${task.name}`, subtitle: task.timerKind === "countup" ? "正向计时" : `${task.durationMinutes} 分钟${task.mode === "strict" ? " · 严格" : ""}`, icon: "play", group: "专注", keywords: "focus start 开始", run: () => { if (focus) { openTimer(); return; } void api.startFocusTask(task.id).then(async () => { await changed(); openTimer(); }).catch(reason => setError(String(reason))); } })),
    { id: "new-limit", title: "新增每日限制", icon: "limits", group: "操作", keywords: "limit rule 额度", run: () => { navigate("rules"); requestAnimationFrame(() => openRule(null)); } },
    { id: "theme-toggle", title: resolved === "dark" ? "切换到浅色外观" : "切换到深色外观", icon: resolved === "dark" ? "sun" : "moon", group: "外观", shortcut: "Ctrl Shift L", keywords: "theme dark light 主题", run: toggleColorMode },
    { id: "theme-system", title: "外观跟随系统", icon: "palette", group: "外观", keywords: "theme system 主题 自动", run: () => chooseTheme("system") },
    { id: "export-backup", title: "导出完整备份", icon: "download", group: "数据", keywords: "backup 备份", run: () => { void api.exportBackup().then(result => { if (result) setError(`备份已保存：${result}`); }).catch(reason => setError(String(reason))); } },
    { id: "export-csv", title: "导出 CSV 明细", icon: "list", group: "数据", keywords: "csv export 导出", run: () => { void api.exportActivityCsv().then(result => { if (result) setError(`明细已导出：${result}`); }).catch(reason => setError(String(reason))); } },
  ], [navigate, liveSession, workspace.tasks, focus, openTimer, changed, resolved, openRule]);

  if (!dashboard) return <div className="kd-loading"><span className="brand-glyph large">刻</span>{refreshError ? <><p role="alert">{refreshError}</p><button onClick={refresh}>重试</button></> : <p>正在读取本地记录…</p>}</div>;
  const toast = error || refreshError || focusError;
  let lastGroup: string | null = null;
  return <div className="app-shell">
    <aside className="sidebar">
      <div className="sb-brand"><span className="brand-glyph" aria-hidden="true">刻</span><strong>刻度</strong></div>
      <button type="button" className="sb-search" onClick={() => setPaletteOpen(true)}><Icon name="search" size={14}/><span>搜索</span><kbd>Ctrl K</kbd></button>
      <nav aria-label="主要导航">{pages.filter(item => item.id !== "settings").map(item => {
        const header = item.group !== lastGroup ? item.group : null; lastGroup = item.group;
        return <div key={item.id} className="sb-item-wrap">{header && <small className="sb-group">{header}</small>}
          <button type="button" aria-label={item.label} aria-current={page === item.id ? "page" : undefined} className={page === item.id ? "active" : ""} onClick={() => navigate(item.id)}>
            <Icon name={item.icon} size={17}/><span>{item.label}</span>{item.id === "focus" && liveSession && <i className="sb-badge" aria-label="专注进行中"/>}
          </button></div>;
      })}</nav>
      <div className="sb-footer">
        <LiveStatus dashboard={dashboard} session={liveSession} onOpenFocus={openTimer} onOpenToday={() => navigate("today")}/>
        <button type="button" aria-label="设置" aria-current={page === "settings" ? "page" : undefined} className={`sb-settings ${page === "settings" ? "active" : ""}`} onClick={() => navigate("settings")}><Icon name="settings" size={17}/><span>设置</span></button>
      </div>
    </aside>
    <main className="content" ref={content}>
      {toast && <div className="kd-toast" role="alert"><span>{toast}</span><button aria-label="关闭提示" onClick={() => { setError(""); refresh(); reloadFocus(); }}><Icon name="close" size={14}/></button></div>}
      <div className="content-inner" key={page}>
        {page === "today" && <TodayPage dashboard={dashboard} catalog={targetCatalog} discoverApplications={discoverApplications} rules={rules} focus={focus} openFocus={() => { setTimerRequest(liveSession ? timerRequest + 1 : 0); navigate("focus"); }} openRules={() => navigate("rules")} focusAction={focusAction}/>}
        {page === "review" && <ReviewPage catalog={targetCatalog} discoverApplications={discoverApplications}/>}
        {page === "insights" && <InsightsPage/>}
        {page === "focus" && <FocusWorkspace workspace={workspace} catalog={targetCatalog} changed={changed} timerRequest={timerRequest} modalOpen={Boolean(restriction)}/>}
        {page === "rules" && <LimitsWorkspace mode="limits" rules={rules} catalog={targetCatalog} classifications={classifications} openRule={openRule} openClassification={rule => { setEditingClassification(rule); classificationDialog.current?.showModal(); }} toggleRule={toggleRule} deleteRule={deleteRule} toggleClassification={toggleClassification} deleteClassification={deleteClassification}/>}
        {page === "classification" && <><div hidden={categoryRulesOpen}><ClassificationWorkspace revision={categoryRevision} onChanged={() => { setCategoryRevision(value => value + 1); refresh(); }} onRules={() => setCategoryRulesOpen(true)} onExchange={(mode, keys, query) => setExchange({ mode, keys, query })}/></div>{categoryRulesOpen && <><button className="kd-back kd-back-floating" onClick={() => { setCategoryRulesOpen(false); setCategoryRevision(value => value + 1); }}><Icon name="chevronLeft" size={16}/>返回分类</button><LimitsWorkspace mode="categories" rules={rules} catalog={targetCatalog} classifications={classifications} openRule={() => {}} openClassification={rule => { setEditingClassification(rule); classificationDialog.current?.showModal(); }} toggleRule={toggleRule} deleteRule={deleteRule} toggleClassification={toggleClassification} deleteClassification={deleteClassification}/></>}</>}
        {page === "settings" && <SettingsPage onFocusSettingsSaved={changed}/>}
      </div>
    </main>
    <RuleDialog dialogRef={ruleDialog} editing={editingRule} catalog={targetCatalog} onSave={submitRule}/><ClassificationDialog dialogRef={classificationDialog} editing={editingClassification} catalog={targetCatalog} onSave={submitClassification}/>
    {exchange && <ClassificationExchange request={exchange} onClose={() => setExchange(null)} onChanged={() => { setCategoryRevision(value => value + 1); refresh(); }}/>}
    {restriction && <RestrictionScreen notice={restriction} focus={focus} grant={grantEmergency} pause={useWrapUp} close={() => setRestriction(null)} returnToTimer={() => { setRestriction(null); openTimer(); }}/>}
    {paletteOpen && <CommandPalette commands={commands} close={() => setPaletteOpen(false)}/>}
  </div>;
}

function RestrictionScreen({ notice, focus, grant, pause, close, returnToTimer }: { notice: RestrictionNotice; focus: FocusState | null; grant: (cause: RestrictionCause) => Promise<void>; pause: () => Promise<void>; close: () => void; returnToTimer: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const icons = useApplicationIcons(notice.subjectKind === "website" ? [] : [notice.subject ?? notice.appName]);
  const [seconds, setSeconds] = useState(3);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const hasFocus = notice.causes.some(cause => cause.kind === "focus");
  const timerReturn = useRef(returnToTimer); timerReturn.current = returnToTimer;
  useEffect(() => { dialog.current?.showModal(); }, []);
  useEffect(() => { if (!hasFocus || busy || error) return; setSeconds(3); let left = 3; const timer = window.setInterval(() => { left--; setSeconds(left); if (left <= 0) { clearInterval(timer); timerReturn.current(); } }, 1000); return () => clearInterval(timer); }, [notice, hasFocus, busy, error]);
  const act = async (action: () => Promise<void>) => { if (busy) return; setBusy(true); try { await action(); } catch (reason) { setError(String(reason)); } finally { setBusy(false); } };
  return <dialog ref={dialog} className="restriction-screen" aria-label="使用受限" onCancel={event => { event.preventDefault(); if (hasFocus) returnToTimer(); else close(); }}><section>
    <div className="restriction-mark">{notice.subjectKind === "website" ? <span className="restriction-site-mark"><Icon name="globe" size={30}/></span> : <ApplicationIcon name={notice.appName} target={notice.subject ?? notice.appName} icons={icons} className="restriction-app-icon"/>}<span className="restriction-badge"><Icon name={hasFocus ? "focus" : "limits"} size={14} strokeWidth={2.2}/></span></div>
    <h2>{notice.appName}</h2><p>{hasFocus ? `正在专注：${focus?.label ?? notice.causes.find(cause => cause.kind === "focus")?.ruleName ?? "学习"}` : "今日额度已用完"}</p>
    <div className="cause-list">{notice.causes.map(cause => <article key={`${cause.kind}-${cause.ruleId}`}><div><strong>{cause.kind === "focus" ? "不在专注白名单中" : cause.ruleName}</strong><span>{cause.kind === "focus" ? "专注期间暂不可用" : `今日已使用 ${formatDuration(cause.usedSeconds)}`}</span></div>{cause.kind === "daily" && <button disabled={busy} onClick={() => void act(() => grant(cause))}>追加 15 分钟</button>}</article>)}</div>
    {focus?.mode === "normal" && focus.status === "focusing" && focus.leaveRemainingMilliseconds > 0 && hasFocus && <button className="wrap-up" disabled={busy} onClick={() => void act(pause)}>暂停专注 · 可用 {Math.ceil(focus.leaveRemainingMilliseconds / 60000)} 分钟</button>}
    {error && <p role="alert" className="form-error">{error}</p>}<button className="primary kd-button-large restriction-back" onClick={hasFocus ? returnToTimer : close}>{hasFocus ? `${seconds} 秒后返回计时` : "好的"}</button>
  </section></dialog>;
}
export default App;
