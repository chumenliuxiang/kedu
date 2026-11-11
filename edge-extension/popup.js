const status = document.getElementById("status");
const refresh = document.getElementById("refresh");
const time = value => value ? new Intl.DateTimeFormat("zh-CN", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false }).format(new Date(value)) : "暂无";

async function update() {
  refresh.disabled = true;
  let result;
  try { result = await chrome.runtime.sendMessage({ type: "kedu:status" }); }
  catch { result = null; }
  const online = Boolean(result?.bridgeOnline);
  document.getElementById("connection").textContent = online ? "已连接" : "未连接";
  document.getElementById("received").textContent = time(result?.lastReceivedAt);
  document.getElementById("written").textContent = time(result?.lastWrittenAt);
  status.className = result?.state === "recording" ? "recording" : "waiting";
  const labels = { recording: "正在记录前台网页", idle: "闲置中", inactive: "浏览器未在前台", unsupported_page: "当前页面不记录", stale: "网页采集已中断" };
  status.textContent = !online ? "刻度未连接" : !result.extensionOnline ? "尚未收到网页记录" : labels[result.state] || "等待确认当前网页";
  document.getElementById("message").textContent = !online ? "请先打开刻度，再刷新。" : result.message || "打开普通网页后检查最近收到与写入时间。";
  refresh.disabled = false;
}
refresh.addEventListener("click", update);
void update();
