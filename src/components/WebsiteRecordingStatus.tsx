import { useCallback } from "react";
import * as api from "../api";
import type { WebsiteRecordingStatus as Status } from "../types";
import { usePolledQuery } from "../hooks/usePolledQuery";

const timestamp = (value: number) => new Intl.DateTimeFormat("zh-CN", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false }).format(value);
export function WebsiteRecordingStatus() {
  const query = useCallback(api.getWebsiteRecordingStatus, []);
  const { data, error, reload } = usePolledQuery<Status | null>(query, () => null, { intervalMs: 5000 });
  if (error) return <button type="button" className="collector-error" onClick={reload}>网站记录状态读取失败 · 重试</button>;
  if (!data) return <p className="data-status">正在读取网站记录状态…</p>;
  return <div className="website-recording-status" role="status"><p>{data.message || (!data.bridgeOnline ? "网站记录服务未启动" : !data.extensionOnline ? "未收到扩展数据" : !data.pageConfirmed ? "等待当前页面确认" : "当前页面已确认")}</p><p className="data-status">{data.lastReceivedAt ? `最近接收 ${timestamp(data.lastReceivedAt)}` : "尚未收到页面记录"}{data.lastWrittenAt ? ` · 最近写入 ${timestamp(data.lastWrittenAt)}` : " · 尚未写入网站记录"}</p></div>;
}
