import { invoke } from "@tauri-apps/api/core";
import { openPath } from "@tauri-apps/plugin-opener";
import type { WebsitePages, WebsiteRecordingStatus, TimelineItem, UsageInsights } from "./types";
import type { FocusAppearance, FocusPreferences, FocusTask, FocusTaskInput, FocusWhitelistEntry, FocusWhitelistInput, FocusWorkspaceData } from "./types";
import type { ActivityDetails, ActivityDetailsQuery, ActivityStatistics, ApplicationIconMap, AppSettings, ClassificationRule, Dashboard, ExceptionUsage, FocusState, LimitRule, NewClassificationRule, NewLimitRule, RankingPage, RuleChangeResult, RuntimeStatus, ScheduleImport, ScheduleImportItem, ScheduleItem, StatisticsExclusion, StatisticsQuery, StatisticsKind, TargetApplication, TargetCatalog } from "./types";

export const isDesktop = () => "__TAURI_INTERNALS__" in window;
const testBackend = () => new URLSearchParams(window.location.search).get("testBackend");
const now = Date.now();
const mockDashboard: Dashboard = {
  generatedAt: now, dayStartedAt:new Date(new Date().setHours(0,0,0,0)).getTime(), effectiveSeconds: 16680, trackedSeconds: 18720,
  currentActivity: { appName: "Microsoft Edge", processPath: "msedge.exe", title: "刻度 · 产品设计", idle: false,idleSeconds:0,idleExempt:false },
  appUsage: [
    { appName: "Microsoft Edge", seconds: 7720, percent: 46, color: "#2f6d73" },
    { appName: "Visual Studio Code", seconds: 4890, percent: 29, color: "#d9963d" },
    { appName: "微信", seconds: 1760, percent: 11, color: "#536b8e" },
    { appName: "Obsidian", seconds: 1380, percent: 8, color: "#9b6b78" },
    { appName: "其他", seconds: 950, percent: 6, color: "#78945c" },
  ],
  siteUsage: [
    { appName: "Microsoft", target: "microsoft.com", seconds: 3240, percent: 19, color: "#536b8e" },
    { appName: "github.com", seconds: 2380, percent: 14, color: "#9b6b78" },
    { appName: "哔哩哔哩", target: "bilibili.com", seconds: 870, percent: 5, color: "#78945c" },
  ],
  timeline: [
    { id: 1, appName: "Microsoft Edge", title: "刻度 · 产品设计", startedAt: now - 42 * 60000, endedAt: now, seconds: 2520, idle: false },
    { id: 2, appName: "Visual Studio Code", title: "storage.rs — Time", startedAt: now - 88 * 60000, endedAt: now - 44 * 60000, seconds: 2640, idle: false },
    { id: 3, appName: "微信", title: "微信", startedAt: now - 99 * 60000, endedAt: now - 90 * 60000, seconds: 540, idle: false },
    { id: 4, appName: "Obsidian", title: "今日学习安排", startedAt: now - 140 * 60000, endedAt: now - 103 * 60000, seconds: 2220, idle: false },
    { id: 5, appName: "离开", title: "无键鼠输入", startedAt: now - 158 * 60000, endedAt: now - 143 * 60000, seconds: 900, idle: true },
  ], focus: null,
};
let mockRules: LimitRule[] = [
  { id: 1, name: "微信", targetKind: "application", targetPattern: "WeChat.exe", groupName: "即时通讯", dailyLimitMinutes: 25, enabled: true, usedSeconds: 1020, remainingSeconds: 480 },
  { id: 2, name: "Steam 浏览", targetKind: "application", targetPattern: "steam.exe", groupName: "游戏诱因", dailyLimitMinutes: 10, enabled: true, usedSeconds: 180, remainingSeconds: 420 },
  { id: 3, name: "全部游戏", targetKind: "group", targetPattern: "games", groupName: "游戏", dailyLimitMinutes: 60, enabled: true, usedSeconds: 2520, remainingSeconds: 1080 },
];
let mockClassifications: ClassificationRule[] = [
  { id: 1, name: "学习网站", targetKind: "website", targetPattern: "learn.microsoft.com", category: "学习", enabled: true },
  { id: 2, name: "代码编辑", targetKind: "application", targetPattern: "Code.exe", category: "工作", enabled: true },
];
let mockSettings: AppSettings = { idleMinutes: 5, countMediaWhileIdle: true, meetingApps: "zoom;teams;webex;wemeet;vlc;potplayer;mpv",titleExclusions:"",dayStartHour:0 };
let mockFocus: FocusState | null = null;
const mockSchedules: Record<string, ScheduleItem[]> = {};
async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> { return invoke<T>(command, args); }
export async function getDashboard(): Promise<Dashboard> { if(isDesktop())return call("get_dashboard", { includeStatistics: false }); advancePreviewFocus(); return { ...mockDashboard, generatedAt: Date.now(), dayStartedAt: new Date(new Date().setHours(mockSettings.dayStartHour,0,0,0)).getTime() - (new Date().getHours() < mockSettings.dayStartHour ? 86_400_000 : 0), focus: mockFocus?.status === "completed" ? null : mockFocus }; }
export async function getRuntimeStatus():Promise<RuntimeStatus>{return isDesktop()?call("get_runtime_status"):{dataPath:"%USERPROFILE%\\刻度数据\\kedu.sqlite",dataEarliestAt:Date.now()-12*86_400_000,dataLatestAt:Date.now(),lastSampleAt:Date.now(),lastWriteAt:Date.now(),collectorError:null};}
export async function getStatistics(start: number, end: number): Promise<ActivityStatistics> {
  if (isDesktop()) return call("get_statistics", { start, end });
  return {
    start, end, effectiveSeconds: mockDashboard.effectiveSeconds, trackedSeconds: mockDashboard.trackedSeconds,
    appUsage: mockDashboard.appUsage, siteUsage: mockDashboard.siteUsage,
    categoryUsage: [
      { appName: "学习", seconds: 7010, percent: 42, color: "#2f6d73" },
      { appName: "工作", seconds: 4890, percent: 29, color: "#d9963d" },
      { appName: "社交", seconds: 1760, percent: 11, color: "#536b8e" },
    ],
    trend: Array.from({ length: 12 }, (_, index) => ({ startedAt: start + index * ((end-start)/12), label: `${index*2}:00`, seconds: [0,0,600,1200,2100,900,1800,2600,1900,1500,800,300][index] })),
    trendSeries: [
      {kind:"application",name:"Microsoft Edge",color:"#2f6d73",points:Array.from({length:12},(_,index)=>({startedAt:start+index*((end-start)/12),label:`${index*2}:00`,seconds:[0,0,300,700,1000,400,1000,1200,900,700,400,100][index]}))},
      {kind:"website",name:"learn.microsoft.com",color:"#536b8e",points:Array.from({length:12},(_,index)=>({startedAt:start+index*((end-start)/12),label:`${index*2}:00`,seconds:[0,0,0,300,600,200,500,700,400,300,100,0][index]}))},
      {kind:"category",name:"学习",color:"#2f6d73",points:Array.from({length:12},(_,index)=>({startedAt:start+index*((end-start)/12),label:`${index*2}:00`,seconds:[0,0,200,500,900,300,800,1100,700,500,200,100][index]}))},
    ],
    timeline: mockDashboard.timeline,
  };
}

// Browser preview data is for layout exploration only. Installed validation uses these commands.
let previewExclusions: StatisticsExclusion[] = [];
const previewWebsiteVisits: TimelineItem[] = [
  { id: 101, appName: "哔哩哔哩", target: "bilibili.com", domain: "www.bilibili.com", pageKey: "www.bilibili.com/video/BV1Learning", title: "高等数学 · 导数与微分", startedAt: now - 50 * 60000, endedAt: now - 42 * 60000, seconds: 480, idle: false, sourceKind: "website", category: "学习" },
  { id: 102, appName: "哔哩哔哩", target: "bilibili.com", domain: "www.bilibili.com", pageKey: "www.bilibili.com/video/BV1Learning", title: "高等数学 · 导数与微分", startedAt: now - 35 * 60000, endedAt: now - 30 * 60000, seconds: 300, idle: false, sourceKind: "website", category: "学习" },
  { id: 103, appName: "哔哩哔哩", target: "bilibili.com", domain: "www.bilibili.com", pageKey: "www.bilibili.com/", title: "哔哩哔哩首页", startedAt: now - 20 * 60000, endedAt: now - 18.5 * 60000, seconds: 90, idle: false, sourceKind: "website", category: "未分类" },
];
export async function getWebsiteRecordingStatus(): Promise<WebsiteRecordingStatus> {
  return isDesktop() ? call("get_website_recording_status") : { bridgeOnline: false, extensionOnline: false, pageConfirmed: false, state: "preview", lastReceivedAt: null, lastWrittenAt: null, message: "浏览器预览不采集网站" };
}
export async function queryWebsitePages(query: StatisticsQuery, site: string, offset = 0, limit = 30, search = ""): Promise<WebsitePages> {
  if (isDesktop()) return call("query_website_pages", { query, site, offset, limit, search });
  const groups = new Map<string, WebsitePages["items"][number]>();
  for (const visit of previewWebsiteVisits.filter(item => item.target === site && item.endedAt > query.start && item.startedAt < query.end)) {
    const key = visit.pageKey!;
    const milliseconds = Math.max(0, Math.min(query.end, visit.endedAt) - Math.max(query.start, visit.startedAt));
    const previous = groups.get(key);
    groups.set(key, { site, pageKey: key, title: visit.title, domain: visit.domain!, milliseconds: (previous?.milliseconds ?? 0) + milliseconds, seconds: (previous?.seconds ?? 0) + milliseconds / 1000, visits: (previous?.visits ?? 0) + 1, lastSeenAt: Math.max(previous?.lastSeenAt ?? 0, visit.endedAt) });
  }
  const items = [...groups.values()].filter(item => `${item.title} ${item.domain}`.toLocaleLowerCase().includes(search.toLocaleLowerCase())).sort((a, b) => b.milliseconds - a.milliseconds);
  return { items: items.slice(offset, offset + limit), total: items.length, offset, limit };
}
export async function queryStatistics(query: StatisticsQuery, includeTimeline = true): Promise<ActivityStatistics> {
  if (isDesktop()) return call("query_statistics", { query, includeTimeline, includeTrendSeries: false });
  const backend=testBackend();if(backend){const response=await fetch(`${backend}/v1/test/statistics`,{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify(query)});if(!response.ok)throw new Error(await response.text());return response.json() as Promise<ActivityStatistics>;}
  const data = await getStatistics(query.start, query.end);
  if (query.targetKind === "website") {
    const visits = await queryActivityDetails({ ...query, limit: 1000 });
    const milliseconds = visits.items.reduce((sum, item) => sum + (item.milliseconds ?? item.seconds * 1000), 0);
    return { ...data, effectiveSeconds: milliseconds / 1000, effectiveMilliseconds: milliseconds, trend: data.trend.map((point, index) => ({ ...point, seconds: index === 0 ? milliseconds / 1000 : 0, milliseconds: index === 0 ? milliseconds : 0 })) };
  }
  const series = query.target && data.trendSeries.find(item => item.kind === query.targetKind && item.name === query.target);
  const trend = series ? series.points : data.trend;
  return { ...data, trend, recordsCount: data.timeline.length, effectiveMilliseconds: (series ? trend.reduce((sum, point) => sum + point.seconds, 0) : data.effectiveSeconds) * 1000 };
}
export async function queryActivityDetails(query: ActivityDetailsQuery): Promise<ActivityDetails> {
  if (isDesktop()) return call("query_activity_details", { query });
  const source = query.targetKind === "website" ? previewWebsiteVisits : mockDashboard.timeline;
  const items = source.filter(item => (!query.target || (item.target || item.appName) === query.target) && (!query.pageKey || item.pageKey === query.pageKey) && (!query.search || `${item.appName} ${item.title}`.toLocaleLowerCase().includes(query.search.toLocaleLowerCase())) && item.endedAt > query.start && item.startedAt < query.end).map(item => { const startedAt = Math.max(item.startedAt, query.start), endedAt = Math.min(item.endedAt, query.end); return { ...item, startedAt, endedAt, milliseconds: endedAt - startedAt, seconds: (endedAt - startedAt) / 1000 }; });
  const offset = query.offset ?? 0, limit = query.limit ?? 40;
  return { items: items.slice(offset, offset + limit), total: items.length, offset, limit };
}
export async function queryRanking(query: StatisticsQuery, kind: StatisticsKind, offset=0, limit=50): Promise<RankingPage> {
  if (isDesktop()) return call("query_ranking", { query, kind, offset, limit });
  const statistics = await queryStatistics(query);
  const rows = kind === "application" ? statistics.appUsage : kind === "website" ? statistics.siteUsage : statistics.categoryUsage;
  return { items: rows.slice(offset, offset + limit), total: rows.length, offset, limit };
}
export async function setDefaultCategory(targetKind: "application" | "website", target: string, category: string): Promise<void> {
  if (isDesktop()) return call("set_default_category", { targetKind, target, category });
}
export async function setActivityCategory(sourceKind: "application" | "website", sourceId: number, category: string | null, sourceIds?: number[] | null): Promise<void> {
  if (isDesktop()) return call("set_activity_category", { sourceKind, sourceId, category, sourceIds });
}
export async function setStatisticsExcluded(targetKind: "application" | "website", target: string, excluded: boolean): Promise<void> {
  if (isDesktop()) return call("set_statistics_excluded", { targetKind, target, excluded });
  previewExclusions = previewExclusions.filter(item => item.targetKind !== targetKind || item.target !== target);
  if (excluded) previewExclusions.push({ targetKind, target, displayName: target });
}
export async function listStatisticsExclusions(): Promise<StatisticsExclusion[]> {
  return isDesktop() ? call("list_statistics_exclusions") : previewExclusions;
}
export async function getApplicationIcons(targets: string[]): Promise<ApplicationIconMap> {
  const unique = [...new Set(targets.map(target => target.trim()).filter(Boolean))];
  if (!unique.length || !isDesktop()) return {};
  return call("get_application_icons", { targets: unique });
}
export async function getTargetCatalog():Promise<TargetCatalog>{
  if(isDesktop())return call("get_target_catalog");
  return {applications:[
    {displayName:"Microsoft Edge",executableName:"msedge.exe",path:"C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",source:"history",lastSeenAt:now},
    {displayName:"Visual Studio Code",executableName:"Code.exe",path:"C:\\Program Files\\Microsoft VS Code\\Code.exe",source:"history",lastSeenAt:now-1000},
    {displayName:"微信",executableName:"Weixin.exe",path:"E:\\Weixin\\Weixin.exe",source:"history",lastSeenAt:now-2000},
    {displayName:"Steam",executableName:"steam.exe",path:"C:\\Program Files (x86)\\Steam\\steam.exe",source:"installed",lastSeenAt:null},
  ],websites:mockDashboard.siteUsage.map(item=>item.target || item.appName),groups:["即时通讯","游戏诱因","游戏"]};
}
export async function pickApplicationExecutable():Promise<TargetApplication|null>{
  return isDesktop()?call("pick_application_executable"):null;
}
export async function listRules(): Promise<LimitRule[]> { return isDesktop() ? call("list_rules") : [...mockRules]; }
export async function startFocus(label: string, minutes: number, mode: string): Promise<FocusState> {
  if (isDesktop()) return call("start_focus", { label, minutes, mode });
  const task = await saveFocusTask({ name: label, durationMinutes: minutes, mode: mode as "normal" | "strict", timerKind: "countdown" }); return startFocusTask(task.id);
}
export async function startFocusLeave(): Promise<FocusState> { if (isDesktop()) return call("start_focus_leave"); if (!mockFocus) throw new Error("当前没有进行中的专注"); return pauseFocus(mockFocus.id); }
export async function returnToFocus(): Promise<FocusState> { if (isDesktop()) return call("return_to_focus"); if (!mockFocus) throw new Error("当前没有进行中的专注"); return resumeFocus(mockFocus.id); }
export async function endFocus(): Promise<FocusState> { if (isDesktop()) return call("end_focus"); if(!mockFocus)throw new Error("当前没有进行中的专注"); return completeFocus(mockFocus.id); }
export async function cancelFocusEnd():Promise<FocusState>{if(isDesktop())return call("cancel_focus_end");if(!mockFocus)throw new Error("当前没有进行中的专注");mockFocus={...mockFocus,pendingEndAt:null};return mockFocus;}
export async function createRule(rule: NewLimitRule): Promise<LimitRule> { if (isDesktop()) return call("create_rule", { rule }); const created = { ...rule, id: Date.now(), enabled: true, usedSeconds: 0, remainingSeconds: rule.dailyLimitMinutes * 60 }; mockRules = [...mockRules, created]; return created; }
export async function updateRule(id: number, rule: NewLimitRule): Promise<LimitRule> { if (isDesktop()) return call("update_rule", { id, rule }); const index=mockRules.findIndex(item=>item.id===id);if(index<0)throw new Error("限制规则不存在");mockRules[index]={...mockRules[index],...rule,remainingSeconds:Math.max(0,rule.dailyLimitMinutes*60-mockRules[index].usedSeconds)};return mockRules[index]; }
const previewReminders = new Map<number, number[]>();
export async function getLimitReminders(ruleId: number): Promise<number[]> {
  if (isDesktop()) return call("get_limit_reminders", { ruleId });
  return previewReminders.get(ruleId) ?? [5, 1].filter(value => value < (mockRules.find(rule => rule.id === ruleId)?.dailyLimitMinutes ?? 30));
}
export async function saveRuleWithReminders(id: number | null, rule: NewLimitRule, minutes: number[]): Promise<LimitRule> {
  if (isDesktop()) return call("save_rule_with_reminders", { id, rule, minutes });
  if (minutes.length > 32 || minutes.some(value => !Number.isInteger(value) || value < 1 || value >= rule.dailyLimitMinutes)) throw new Error("提醒时间须为小于每日额度的正整数，最多 32 项");
  if (id === null && rule.targetKind === "website") throw new Error("暂不支持新建网站限制");
  const saved = id === null ? await createRule(rule) : await updateRule(id, rule);
  previewReminders.set(saved.id, [...new Set(minutes)].sort((a, b) => b - a));
  return saved;
}
export async function deleteRule(id: number): Promise<void> { if (isDesktop()) return call("delete_rule", { id }); mockRules=mockRules.filter(item=>item.id!==id); }
export async function setRuleEnabled(id: number, enabled: boolean): Promise<RuleChangeResult> { if (isDesktop()) return call("set_rule_enabled", { id, enabled }); const rule=mockRules.find(item=>item.id===id);if(rule)rule.enabled=enabled;return {pending:false,applyAt:null}; }
export async function listClassificationRules(): Promise<ClassificationRule[]> { return isDesktop()?call("list_classification_rules"):[...mockClassifications]; }
export async function createClassificationRule(rule:NewClassificationRule):Promise<ClassificationRule>{if(isDesktop())return call("create_classification_rule",{rule});const created={...rule,id:Date.now(),enabled:true};mockClassifications=[...mockClassifications,created];return created;}
export async function updateClassificationRule(id:number,rule:NewClassificationRule):Promise<ClassificationRule>{if(isDesktop())return call("update_classification_rule",{id,rule});const index=mockClassifications.findIndex(item=>item.id===id);if(index<0)throw new Error("分类规则不存在");mockClassifications[index]={...mockClassifications[index],...rule};return mockClassifications[index];}
export async function setClassificationRuleEnabled(id:number,enabled:boolean):Promise<void>{if(isDesktop())return call("set_classification_rule_enabled",{id,enabled});const rule=mockClassifications.find(item=>item.id===id);if(rule)rule.enabled=enabled;}
export async function deleteClassificationRule(id:number):Promise<void>{if(isDesktop())return call("delete_classification_rule",{id});mockClassifications=mockClassifications.filter(item=>item.id!==id);}
export async function getSettings():Promise<AppSettings>{return isDesktop()?call("get_settings"):{...mockSettings};}
export async function updateSettings(settings:AppSettings):Promise<AppSettings>{if(isDesktop())return call("update_settings",{settings});mockSettings={...settings};return {...mockSettings};}
export async function grantException(kind: "daily" | "focus", ruleId: number, minutes = 15, subjectKind = "", subject = ""): Promise<void> {
  if (isDesktop()) await call("grant_exception", { kind, target: String(ruleId), subjectKind, subject, minutes, reason: "紧急使用" });
}
export async function listExceptionHistory(limit=20):Promise<ExceptionUsage[]>{return isDesktop()?call("list_exception_history",{limit}):[];}
export async function endException(id:number):Promise<void>{if(isDesktop())await call("end_exception",{id});}
export async function openEdgeExtensionFolder(): Promise<void> {
  if (!isDesktop()) return;
  const path = await call<string>("edge_extension_path");
  await openPath(path);
}
export async function getAutostart():Promise<boolean>{return isDesktop()?call("get_autostart"):false;}
export async function setAutostart(enabled:boolean):Promise<boolean>{return isDesktop()?call("set_autostart",{enabled}):enabled;}
export async function listSchedule(date: string): Promise<ScheduleItem[]> {
  return isDesktop() ? call("list_schedule", { date }) : [...(mockSchedules[date] ?? [])];
}
export async function importSchedule(date: string, schedule: ScheduleImport): Promise<ScheduleItem[]> {
  if (isDesktop()) return call("import_schedule", { date, schedule });
  mockSchedules[date] = schedule.items.map((item, index) => ({ ...item, id: Date.now() + index, date, sourceName: schedule.name, enabled: true, started: false }));
  return [...mockSchedules[date]];
}
export async function setScheduleItemEnabled(id: number, enabled: boolean): Promise<void> {
  if (isDesktop()) return call("set_schedule_item_enabled", { id, enabled });
  for (const items of Object.values(mockSchedules)) { const item = items.find(entry => entry.id === id); if (item) item.enabled = enabled; }
}
export async function setScheduleDayEnabled(date: string, enabled: boolean): Promise<void> {
  if (isDesktop()) return call("set_schedule_day_enabled", { date, enabled });
  for (const item of mockSchedules[date] ?? []) if (!item.started) item.enabled = enabled;
}
export async function saveScheduleItem(id:number|null,date:string,item:ScheduleImportItem):Promise<ScheduleItem>{
  if(isDesktop())return call("save_schedule_item",{id,date,item});
  const existing=id?mockSchedules[date]?.find(entry=>entry.id===id):undefined;
  const saved:ScheduleItem={...item,id:id??Date.now(),date,sourceName:"手动安排",enabled:existing?.enabled??true,started:false};
  mockSchedules[date]=[...(mockSchedules[date]??[]).filter(entry=>entry.id!==id),saved].sort((a,b)=>a.start.localeCompare(b.start));return saved;
}
export async function deleteScheduleItem(id:number):Promise<void>{if(isDesktop())return call("delete_schedule_item",{id});for(const date of Object.keys(mockSchedules))mockSchedules[date]=mockSchedules[date].filter(item=>item.id!==id);}
export async function exportBackup(): Promise<string | null> { return isDesktop() ? call("export_backup") : "浏览器预览未写入文件"; }
export async function restoreBackup(): Promise<string | null> { return isDesktop() ? call("restore_backup") : "浏览器预览未读取文件"; }
export async function exportActivityCsv(start = 0, end = Date.now()): Promise<string | null> { return isDesktop() ? call("export_activity_csv", { start, end }) : "浏览器预览未写入文件"; }

// Browser preview state is intentionally separate from desktop persistence.
let previewTasks: FocusTask[] = [];
let previewWhitelist: FocusWhitelistEntry[] = [];
let previewAppearance: FocusAppearance = { style: "normal", size: "normal", autoImmerse: false };
export const defaultFocusPreferences: FocusPreferences = { windowGuardMode: "reminder", pauseMinutes: 5, restMinutes: 5, notificationsEnabled: true, soundEnabled: true, scheduleStartNotificationsEnabled: true, scheduleStartSoundEnabled: true, scheduleEndNotificationsEnabled: true, scheduleEndSoundEnabled: true };
let previewPreferences = { ...defaultFocusPreferences };
type PreviewFocusHistory = { id: number; taskId: number | null; label: string; taskName?: string; startedAt: number; completedAt: number; elapsedMilliseconds: number; pauses: { startedAt: number; endedAt: number }[]; timerKind: "countdown" | "countup"; mode: "normal" | "strict"; completionReason: string };
const previewHistory: PreviewFocusHistory[] = [];
let previewPauses: PreviewFocusHistory["pauses"] = [];
let previewId = Date.now();
function closePreviewPause(at: number) {
  if (mockFocus?.activeLeaveStartedAt != null) {
    previewPauses.push({ startedAt: mockFocus.activeLeaveStartedAt, endedAt: Math.max(mockFocus.activeLeaveStartedAt, at) });
    mockFocus.activeLeaveStartedAt = null;
  }
}
function finishPreviewFocus(at: number, reason: string) {
  if (!mockFocus || mockFocus.completedAt !== null) return;
  closePreviewPause(at);
  mockFocus = { ...mockFocus, status: mockFocus.restDurationSeconds ? "resting" : "completed", completedAt: at, completionReason: reason, breakEndsAt: at + mockFocus.restDurationSeconds * 1000, activeLeaveStartedAt: null };
  previewHistory.push({ id: mockFocus.id, taskId: mockFocus.taskId, label: mockFocus.label, startedAt: mockFocus.startedAt, completedAt: at, elapsedMilliseconds: mockFocus.elapsedMilliseconds, pauses: previewPauses.map(pause => ({ ...pause })), timerKind: mockFocus.timerKind, mode: mockFocus.mode, completionReason: reason });
  const task = previewTasks.find(item => item.id === mockFocus?.taskId);
  if (task) { task.todayCompleted++; task.todayMilliseconds += mockFocus.elapsedMilliseconds; }
}
function advancePreviewFocus() {
  if (!mockFocus) return;
  const now = Date.now();
  let cursor = mockFocus.observedAt;
  if (mockFocus.status === "away") {
    const spent = Math.min(Math.max(0, now - cursor), mockFocus.leaveRemainingMilliseconds);
    mockFocus.leaveRemainingMilliseconds -= spent;
    mockFocus.leaveRemainingSeconds = Math.ceil(mockFocus.leaveRemainingMilliseconds / 1000);
    mockFocus.leaveUsedSeconds = mockFocus.leaveBudgetSeconds - mockFocus.leaveRemainingSeconds;
    cursor += spent;
    if (!mockFocus.leaveRemainingMilliseconds) { closePreviewPause(cursor); mockFocus.status = "focusing"; }
  }
  if (mockFocus.status === "focusing") {
    const elapsed = Math.max(0, now - cursor);
    const counted = mockFocus.remainingMilliseconds === null ? elapsed : Math.min(mockFocus.remainingMilliseconds, elapsed);
    mockFocus.elapsedMilliseconds += counted;
    if (mockFocus.remainingMilliseconds !== null) {
      mockFocus.remainingMilliseconds -= counted;
      mockFocus.plannedEnd = now + mockFocus.remainingMilliseconds;
      if (mockFocus.remainingMilliseconds <= 0) finishPreviewFocus(cursor + counted, "elapsed");
    }
  }
  if (mockFocus.status === "resting" && (mockFocus.breakEndsAt ?? Infinity) <= now) mockFocus.status = "completed";
  mockFocus.observedAt = now;
}
export async function getFocusWorkspace(): Promise<FocusWorkspaceData> {
  if (isDesktop()) return call("get_focus_workspace");
  advancePreviewFocus();
  return { tasks: previewTasks.map(task => ({ ...task })), session: mockFocus ? { ...mockFocus } : null, whitelist: previewWhitelist.map(item => ({ ...item })), appearance: { ...previewAppearance }, preferences: { ...previewPreferences } };
}
export async function getFocusPreferences(): Promise<FocusPreferences> {
  return isDesktop() ? call("get_focus_preferences") : { ...previewPreferences };
}
export async function saveFocusPreferences(preferences: FocusPreferences): Promise<FocusPreferences> {
  if (isDesktop()) return call("save_focus_preferences", { preferences });
  if (![preferences.pauseMinutes, preferences.restMinutes].every(value => Number.isInteger(value) && value >= 0 && value <= 60)) throw new Error("暂停和休息时长需为 0–60 分钟的整数");
  previewPreferences = { ...preferences }; return { ...previewPreferences };
}
export function getPreviewFocusHistory(): PreviewFocusHistory[] {
  advancePreviewFocus();
  return previewHistory.map(item => ({ ...item, taskName: previewTasks.find(task => task.id === item.taskId)?.name, pauses: item.pauses.map(pause => ({ ...pause })) }));
}
export async function saveFocusTask(input: FocusTaskInput): Promise<FocusTask> {
  if (isDesktop()) return call("save_focus_task", { input });
  if (!input.name.trim() || input.durationMinutes < 1 || input.durationMinutes > 720) throw new Error("请填写任务名称及 1–720 分钟时长");
  const existing = previewTasks.find(task => task.id === input.id);
  const task = { ...input, id: input.id ?? ++previewId, todayCompleted: existing?.todayCompleted ?? 0, todayMilliseconds: existing?.todayMilliseconds ?? 0 };
  previewTasks = [...previewTasks.filter(item => item.id !== task.id), task]; return task;
}
export async function deleteFocusTask(id: number): Promise<void> {
  if (isDesktop()) return call("delete_focus_task", { id });
  previewTasks = previewTasks.filter(task => task.id !== id);
}
export async function startFocusTask(taskId: number): Promise<FocusState> {
  if (isDesktop()) return call("start_focus_task", { taskId });
  advancePreviewFocus();
  if (mockFocus && ["focusing", "away"].includes(mockFocus.status)) return { ...mockFocus };
  const task = previewTasks.find(item => item.id === taskId); if (!task) throw new Error("任务不存在");
  const at = Date.now(); const countdown = task.timerKind === "countdown";
  const pauseSeconds = previewPreferences.pauseMinutes * 60;
  previewPauses = [];
  mockFocus = { id: ++previewId, taskId, label: task.name, mode: task.mode, timerKind: task.timerKind, startedAt: at, plannedEnd: countdown ? at + task.durationMinutes * 60000 : 0, durationSeconds: countdown ? task.durationMinutes * 60 : 0, elapsedMilliseconds: 0, remainingMilliseconds: countdown ? task.durationMinutes * 60000 : null, observedAt: at, leaveBudgetSeconds: pauseSeconds, leaveUsedSeconds: 0, leaveRemainingSeconds: pauseSeconds, leaveRemainingMilliseconds: pauseSeconds * 1000, activeLeaveStartedAt: null, pendingEndAt: null, breakEndsAt: null, restDurationSeconds: previewPreferences.restMinutes * 60, completedAt: null, completionReason: null, status: "focusing" };
  return { ...mockFocus };
}
function previewSession(sessionId: number): FocusState {
  advancePreviewFocus(); if (!mockFocus || mockFocus.id !== sessionId) throw new Error("本轮专注已变化，请重试"); return mockFocus;
}
export async function pauseFocus(sessionId: number): Promise<FocusState> {
  if (isDesktop()) return call("pause_focus", { sessionId });
  const session = previewSession(sessionId);
  if (session.mode === "strict") throw new Error("严格模式不能暂停");
  if (session.status === "focusing" && session.leaveRemainingMilliseconds > 0) { session.status = "away"; session.activeLeaveStartedAt = Date.now(); }
  return { ...session };
}
export async function resumeFocus(sessionId: number): Promise<FocusState> {
  if (isDesktop()) return call("resume_focus", { sessionId });
  const session = previewSession(sessionId); if (session.status === "away") { closePreviewPause(session.observedAt); session.status = "focusing"; } return { ...session };
}
export async function completeFocus(sessionId: number): Promise<FocusState> {
  if (isDesktop()) return call("complete_focus", { sessionId });
  previewSession(sessionId); finishPreviewFocus(Date.now(), "manual"); return { ...mockFocus! };
}
export async function skipFocusBreak(sessionId: number): Promise<FocusState> {
  if (isDesktop()) return call("skip_focus_break", { sessionId });
  const session = previewSession(sessionId); if (session.status === "resting") session.status = "completed"; return { ...session };
}
export async function saveFocusWhitelist(input: FocusWhitelistInput): Promise<FocusWhitelistEntry> {
  if (isDesktop()) return call("save_focus_whitelist", { input });
  const current = previewWhitelist.find(item => item.targetKind === input.targetKind && item.target === input.target);
  if (current) return current;
  const entry = { ...input, id: input.id ?? ++previewId, availableAt: Date.now() + (mockFocus?.status === "focusing" ? 30_000 : 0) };
  previewWhitelist.push(entry); return entry;
}
export async function removeFocusWhitelist(id: number): Promise<void> {
  if (isDesktop()) return call("remove_focus_whitelist", { id });
  previewWhitelist = previewWhitelist.filter(entry => entry.id !== id);
}
export async function saveFocusAppearance(appearance: FocusAppearance): Promise<FocusAppearance> {
  if (isDesktop()) return call("save_focus_appearance", { appearance });
  previewAppearance = { ...appearance }; return previewAppearance;
}
export async function launchFocusApplication(id: number): Promise<void> {
  if (isDesktop()) return call("launch_focus_application", { id });
}

export async function getBrowserProtectionStatus(): Promise<import("./types").BrowserProtectionStatus> {
  if (isDesktop()) return call("get_browser_protection_status");
  return { extensionOnline: false, pageConfirmed: false, state: "offline", domain: null, message: "网站保护未连接", lastSeenAt: null };
}
export async function takeRestrictionNotice(): Promise<import("./types").RestrictionNotice | null> {
  if (isDesktop()) return call("take_restriction_notice");
  return null;
}

// Deterministic preview data: shapes resemble a real week of desk work.
function previewInsights(start: number, end: number): UsageInsights {
  const day = 86_400_000, hourMs = 3_600_000;
  const profile = [0, 0, 0, 0, 0, 0, 0, .1, .35, .8, .95, .85, .4, .55, .9, 1, .9, .7, .45, .5, .7, .6, .3, .05];
  const days: UsageInsights["days"] = [];
  const heatmap = Array.from({ length: 7 }, () => Array<number>(24).fill(0));
  const hourly = Array<number>(24).fill(0);
  for (let cursor = start; cursor < end; cursor += day) {
    const date = new Date(cursor), weekday = (date.getDay() + 6) % 7, seed = (date.getDate() * 37 + date.getMonth() * 11) % 17;
    const scale = weekday >= 5 ? .45 : .75 + seed / 40;
    let total = 0;
    profile.forEach((weight, hour) => {
      const value = cursor + hour * hourMs > Date.now() ? 0 : Math.round(weight * scale * (0.7 + ((seed + hour) % 5) / 10) * hourMs * .82);
      heatmap[weekday][hour] += value; hourly[hour] += value; total += value;
    });
    const label = `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
    days.push({ startedAt: cursor, endedAt: Math.min(end, cursor + day), label, milliseconds: total, firstActivityAt: total ? cursor + (8 * 60 + seed * 3) * 60_000 : null, lastActivityAt: total ? cursor + (22 * 60 + seed * 2) * 60_000 : null });
  }
  const effective = days.reduce((sum, item) => sum + item.milliseconds, 0);
  const peak = hourly.reduce((best, value, hour) => value > hourly[best] ? hour : best, 0);
  const active = days.filter(item => item.milliseconds >= 60_000);
  return {
    start, end, effectiveMilliseconds: effective, idleMilliseconds: Math.round(effective * .08),
    previousStart: start - (end - start), previousEnd: start - (end - start) + Math.max(0, Math.min(Date.now(), end) - start), previousEffectiveMilliseconds: Math.round(effective * .88),
    dayCount: days.length, activeDays: active.length, days, heatmap, hourly, peakHour: effective ? peak : null,
    longestStretch: effective ? { startedAt: start + 14 * hourMs, endedAt: start + 16.4 * hourMs, milliseconds: 2.4 * hourMs, name: null, target: null } : null,
    longestSession: effective ? { startedAt: start + 14.2 * hourMs, endedAt: start + 15.7 * hourMs, milliseconds: 1.5 * hourMs, name: "Visual Studio Code", target: "code.exe" } : null,
    switches: Math.round(effective / 3_600_000 * 24), switchesPerHour: effective ? 24 : 0,
    averageFirstMinute: active.length ? 8 * 60 + 40 : null, averageLastMinute: active.length ? 22 * 60 + 15 : null,
  };
}
export async function queryInsights(start: number, end: number): Promise<UsageInsights> {
  return isDesktop() ? call("query_insights", { start, end }) : previewInsights(start, end);
}
