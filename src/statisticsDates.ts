export type RangeKind = "day" | "week" | "month" | "year" | "custom";
export const dateKey = (date: Date) => `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
export const dateFromKey = (key: string) => { const [year, month, day] = key.split("-").map(Number); return new Date(year, month - 1, day); };
export function shiftDate(key: string, days: number) { const date = dateFromKey(key); date.setDate(date.getDate() + days); return dateKey(date); }
export function statisticalDateKey(hour: number, now = new Date()) { const date = new Date(now); if (date.getHours() < hour) date.setDate(date.getDate() - 1); return dateKey(date); }
export function dayBounds(key: string, hour = 0) { const start = dateFromKey(key); start.setHours(hour, 0, 0, 0); const end = new Date(start); end.setDate(end.getDate() + 1); return { start: start.getTime(), end: end.getTime() }; }
export function rangeBounds(kind: RangeKind, selected: string, customStart: string, customEnd: string, hour: number, today = statisticalDateKey(hour)) {
  const endpoint = selected > today ? today : selected;
  if (kind === "day") return dayBounds(endpoint, hour);
  if (kind === "week" || kind === "month") return { start: dayBounds(shiftDate(endpoint, kind === "week" ? -6 : -29), hour).start, end: dayBounds(endpoint, hour).end };
  if (kind === "year") { const date = dateFromKey(endpoint); const last = `${date.getFullYear()}-12-31`; return { start: new Date(date.getFullYear(), 0, 1, hour).getTime(), end: dayBounds(last > today ? today : last, hour).end }; }
  const last = customEnd > today ? today : customEnd;
  return { start: dayBounds(customStart > last ? last : customStart, hour).start, end: dayBounds(last, hour).end };
}
