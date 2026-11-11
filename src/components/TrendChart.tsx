import { memo, useMemo } from "react";
import type { TrendPoint } from "../types";
import { StackedColumns, type Column } from "./charts";

export function duration(seconds: number, exact = false) {
  const value = Math.max(0, Math.round(seconds));
  const h = Math.floor(value / 3600), m = Math.floor(value % 3600 / 60), s = value % 60;
  if (exact) return [h ? `${h}小时` : "", m ? `${m}分` : "", s || (!h && !m) ? `${s}秒` : ""].filter(Boolean).join(" ");
  return h ? `${h}小时 ${m}分` : m ? `${m}分 ${s}秒` : `${s}秒`;
}
export function DurationValue({ seconds, exact = false }: { seconds: number; exact?: boolean }) {
  return <span className="kd-duration">{duration(seconds, exact).split(" ").map((part, index) => {
    const [, value, unit] = /^(\d+)(.*)$/.exec(part) ?? ["", part, ""];
    return <span className="kd-duration-part" key={index}><span className="kd-duration-number">{value}</span><span className="kd-duration-unit">{unit}</span></span>;
  })}</span>;
}

const full = {
  hour: new Intl.DateTimeFormat("zh-CN", { month: "long", day: "numeric", hour: "2-digit", minute: "2-digit", hour12: false }),
  day: new Intl.DateTimeFormat("zh-CN", { month: "long", day: "numeric", weekday: "short" }),
  month: new Intl.DateTimeFormat("zh-CN", { year: "numeric", month: "long" }),
};
export type Bucket = "hour" | "day" | "month";
/** Infer the bucket from the first point's span when the caller does not say. */
export const bucketOf = (points: TrendPoint[]): Bucket => {
  const first = points[0];
  const span = first ? (first.endedAt ?? first.startedAt) - first.startedAt : 0;
  return span > 0 && span <= 3_700_000 ? "hour" : span > 26 * 3_600_000 ? "month" : "day";
};
function shortLabel(point: TrendPoint, bucket: Bucket) {
  const date = new Date(point.startedAt);
  if (bucket === "hour") return String(date.getHours());
  if (bucket === "month") return `${date.getMonth() + 1}月`;
  return date.getDate() === 1 ? `${date.getMonth() + 1}/1` : String(date.getDate());
}

/** Time series of one measure as columns; a click drills into that period. */
export const TrendColumns = memo(function TrendColumns({ points, bucket: suppliedBucket, color = "var(--kd-accent)", name = "有效使用", onPoint, height = 180, showAverage = true }: {
  points: TrendPoint[]; bucket?: Bucket; color?: string; name?: string; onPoint?: (point: TrendPoint) => void; height?: number; showAverage?: boolean;
}) {
  const bucket = suppliedBucket ?? bucketOf(points);
  const now = Date.now();
  const columns = useMemo<Column[]>(() => points.map((point, index) => ({
    key: `${point.startedAt}-${index}`,
    label: shortLabel(point, bucket),
    tooltipTitle: bucket === "hour" ? `${full.hour.format(point.startedAt)}–${String(new Date(point.endedAt ?? point.startedAt + 3_600_000).getHours()).padStart(2, "0")}:00` : full[bucket].format(point.startedAt),
    segments: [{ key: "value", name, color, milliseconds: point.milliseconds ?? point.seconds * 1000 }],
    highlight: point.startedAt <= now && now < (point.endedAt ?? point.startedAt),
    muted: point.startedAt > now,
    onSelect: onPoint ? () => onPoint(point) : undefined,
  })), [points, bucket, name, color, onPoint, now]);
  const values = columns.filter(column => !column.muted).map(column => column.segments[0].milliseconds);
  const average = showAverage && bucket !== "hour" && values.length > 1 ? values.reduce((sum, value) => sum + value, 0) / values.length : undefined;
  if (!points.length) return <div className="kd-empty">没有记录</div>;
  return <StackedColumns columns={columns} caption={`${name}趋势`} average={average} fixedMax={bucket === "hour" ? 3_600_000 : undefined} labelEvery={bucket === "hour" ? 3 : points.length > 16 ? Math.ceil(points.length / 10) : 1} height={height}/>;
});
