/** Formatting shared by every page. Durations are milliseconds unless named otherwise. */
export const numberFormat = new Intl.NumberFormat("zh-CN");
const timeFormat = new Intl.DateTimeFormat("zh-CN", { hour: "2-digit", minute: "2-digit", hour12: false });
const monthDayFormat = new Intl.DateTimeFormat("zh-CN", { month: "long", day: "numeric" });
const weekdayFormat = new Intl.DateTimeFormat("zh-CN", { weekday: "long" });
const shortWeekdayFormat = new Intl.DateTimeFormat("zh-CN", { weekday: "short" });

/** "3小时 12分", "12分钟", "40秒". */
export function compactDuration(milliseconds: number): string {
  const seconds = Math.max(0, Math.round(milliseconds / 1000));
  const hours = Math.floor(seconds / 3600), minutes = Math.floor(seconds % 3600 / 60);
  if (hours) return minutes ? `${hours}小时 ${minutes}分` : `${hours}小时`;
  if (minutes) return `${minutes}分钟`;
  return `${seconds}秒`;
}
/** Axis-friendly "2.5h", "45m". */
export function shortDuration(milliseconds: number): string {
  const minutes = milliseconds / 60_000;
  if (minutes < 1) return milliseconds > 0 ? "<1m" : "0";
  if (minutes < 60) return `${Math.round(minutes)}m`;
  const hours = minutes / 60;
  return `${Number(hours.toFixed(hours < 10 ? 1 : 0))}h`;
}
export const clockTime = (time: number) => timeFormat.format(time);
export const monthDay = (time: number | Date) => monthDayFormat.format(time);
export const weekday = (time: number | Date) => weekdayFormat.format(time);
export const shortWeekday = (time: number | Date) => shortWeekdayFormat.format(time);
/** Minutes after midnight (may exceed a day) as a wall clock time. */
export function minuteClock(minutes: number): string {
  const value = ((Math.round(minutes) % 1440) + 1440) % 1440;
  return `${String(Math.floor(value / 60)).padStart(2, "0")}:${String(value % 60).padStart(2, "0")}`;
}
export const hourLabel = (hour: number) => `${String(hour).padStart(2, "0")}:00`;
export function hourRange(hour: number) { return `${String(hour).padStart(2, "0")}:00–${String((hour + 1) % 24).padStart(2, "0")}:00`; }

/** Relative change, or null when the baseline is too small to be meaningful. */
export function change(current: number, previous: number): number | null {
  if (previous < 5 * 60_000) return null;
  return (current - previous) / previous;
}
export function percent(value: number, digits = 0) { return `${(value * 100).toFixed(digits)}%`; }

export function greeting(date = new Date()) {
  const hour = date.getHours();
  if (hour < 5) return "夜深了";
  if (hour < 11) return "早上好";
  if (hour < 13) return "中午好";
  if (hour < 18) return "下午好";
  return "晚上好";
}
