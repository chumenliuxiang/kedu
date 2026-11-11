import { invoke } from "@tauri-apps/api/core";
import * as api from "./api";
import { dateFromKey, statisticalDateKey } from "./statisticsDates";

export type FocusStatisticsQuery = { start: number; end: number };
export type FocusDistribution = { key: string; taskId: number | null; label: string; milliseconds: number; completedCount: number };
export type FocusRecord = { id: number; taskKey: string; taskId: number | null; label: string; startedAt: number; completedAt: number; milliseconds: number; rangeMilliseconds: number; pauseMilliseconds: number; timerKind: string; mode: string; completionReason: string };
export type FocusRecordsPage = { items: FocusRecord[]; total: number; offset: number; limit: number };
export type FocusRecordsQuery = FocusStatisticsQuery & { offset?: number; limit?: number; taskKey?: string; search?: string };
export type FocusStatistics = { start: number; end: number; generatedAt: number; total: { completedCount: number; milliseconds: number; averageMilliseconds: number; daysCount: number; firstStartedAt: number | null }; range: { completedCount: number; milliseconds: number }; distribution: FocusDistribution[]; records: FocusRecordsPage };
export type PreviewFocusHistory = { id: number; taskId: number | null; label: string; taskName?: string; startedAt: number; completedAt: number; elapsedMilliseconds: number; pauses: { startedAt: number; endedAt: number }[]; timerKind: string; mode: string; completionReason: string };

const taskKey = (record: PreviewFocusHistory) => record.taskId === null ? `label:${record.label}` : `task:${record.taskId}`;
const overlap = (start: number, end: number, query: FocusStatisticsQuery) => Math.max(0, Math.min(end, query.end) - Math.max(start, query.start));

// Browser preview follows the same completed-learning semantics as the desktop.
// Raw task configuration is never used as the recorded learning duration.
function learningIntervals(record: PreviewFocusHistory) {
  const intervals: { start: number; end: number }[] = [];
  let cursor = record.startedAt;
  let available = Math.max(0, record.elapsedMilliseconds);
  const append = (end: number) => {
    const counted = Math.min(available, Math.max(0, end - cursor));
    if (counted > 0) intervals.push({ start: cursor, end: cursor + counted });
    available -= counted;
  };
  for (const pause of [...record.pauses].sort((a, b) => a.startedAt - b.startedAt)) {
    const start = Math.max(cursor, Math.min(record.completedAt, pause.startedAt));
    append(start);
    cursor = Math.max(cursor, Math.min(record.completedAt, pause.endedAt));
  }
  append(record.completedAt);
  return intervals;
}

function previewRecords(history: PreviewFocusHistory[], query: FocusRecordsQuery): FocusRecordsPage {
  const search = query.search?.trim().toLocaleLowerCase() ?? "";
  const rows = history.flatMap(record => {
    const key = taskKey(record);
    if ((query.taskKey && query.taskKey !== key) || (search && !`${record.label} ${record.taskName ?? ""}`.toLocaleLowerCase().includes(search))) return [];
    const rangeMilliseconds = learningIntervals(record).reduce((sum, interval) => sum + overlap(interval.start, interval.end, query), 0);
    if (rangeMilliseconds === 0 && !(record.completedAt >= query.start && record.completedAt < query.end)) return [];
    return [{ id: record.id, taskKey: key, taskId: record.taskId, label: record.label, startedAt: record.startedAt, completedAt: record.completedAt, milliseconds: record.elapsedMilliseconds, rangeMilliseconds, pauseMilliseconds: record.pauses.reduce((sum, pause) => sum + Math.max(0, pause.endedAt - pause.startedAt), 0), timerKind: record.timerKind, mode: record.mode, completionReason: record.completionReason }];
  }).sort((a, b) => b.completedAt - a.completedAt || b.id - a.id);
  const offset = Math.max(0, query.offset ?? 0), limit = Math.max(1, Math.min(100, query.limit ?? 20));
  return { items: rows.slice(offset, offset + limit), total: rows.length, offset, limit };
}

export async function queryFocusStatistics(query: FocusStatisticsQuery): Promise<FocusStatistics> {
  if (api.isDesktop()) return invoke("query_focus_statistics", { query });
  const history = api.getPreviewFocusHistory();
  const { dayStartHour } = await api.getSettings();
  const distribution = new Map<string, FocusDistribution>();
  let milliseconds = 0, completedCount = 0;
  for (const record of history) {
    const duration = learningIntervals(record).reduce((sum, interval) => sum + overlap(interval.start, interval.end, query), 0);
    const count = Number(record.completedAt >= query.start && record.completedAt < query.end);
    milliseconds += duration; completedCount += count;
    if (!duration && !count) continue;
    const key = taskKey(record);
    const item = distribution.get(key) ?? { key, taskId: record.taskId, label: record.taskName ?? record.label, milliseconds: 0, completedCount: 0 };
    item.milliseconds += duration; item.completedCount += count; distribution.set(key, item);
  }
  const totalMilliseconds = history.reduce((sum, record) => sum + record.elapsedMilliseconds, 0);
  const firstStartedAt = history.length ? Math.min(...history.map(record => record.elapsedMilliseconds > 0 ? record.startedAt : record.completedAt)) : null;
  const calendarOrdinal = (key: string) => { const date = dateFromKey(key); return Date.UTC(date.getFullYear(), date.getMonth(), date.getDate()) / 86_400_000; };
  const daysCount = firstStartedAt === null ? 0 : Math.max(1, calendarOrdinal(statisticalDateKey(dayStartHour)) - calendarOrdinal(statisticalDateKey(dayStartHour, new Date(firstStartedAt))) + 1);
  return { ...query, generatedAt: Date.now(), total: { completedCount: history.length, milliseconds: totalMilliseconds, averageMilliseconds: daysCount ? Math.floor(totalMilliseconds / daysCount) : 0, daysCount, firstStartedAt }, range: { completedCount, milliseconds }, distribution: [...distribution.values()].sort((a, b) => b.milliseconds - a.milliseconds || a.key.localeCompare(b.key)), records: previewRecords(history, query) };
}

export async function listFocusRecords(query: FocusRecordsQuery): Promise<FocusRecordsPage> {
  if (api.isDesktop()) return invoke("list_focus_records", { query });
  return previewRecords(api.getPreviewFocusHistory(), query);
}
