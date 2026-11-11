import { useEffect, useState, type ReactNode } from "react";
import { getVersion } from "@tauri-apps/api/app";
import * as api from "../api";
import type { AppSettings, RuntimeStatus } from "../types";
import { accents, chooseAccent, chooseTheme, modes, refreshTheme, useTheme } from "../theme";
import { FocusPreferencesSettings } from "../components/FocusPreferencesSettings";
import { WebsiteRecordingStatus } from "../components/WebsiteRecordingStatus";
import { PageHeader } from "../components/ui";
import { Icon } from "../components/Icon";
import { shortcutList } from "../shortcuts";

const dateFormat = new Intl.DateTimeFormat("zh-CN", { year: "numeric", month: "long", day: "numeric" });
const timeFormat = new Intl.DateTimeFormat("zh-CN", { hour: "2-digit", minute: "2-digit", hour12: false });

function Group({ id, icon, tone, title, description, children }: { id: string; icon: string; tone: string; title: string; description?: string; children: ReactNode }) {
  return <section className="st-group" id={`settings-${id}`} aria-labelledby={`settings-${id}-title`}>
    <header><span className="st-icon" style={{ background: tone }}><Icon name={icon} size={15} strokeWidth={2}/></span><div><h2 id={`settings-${id}-title`}>{title}</h2>{description && <p>{description}</p>}</div></header>
    <div className="st-box">{children}</div>
  </section>;
}
function Row({ label, hint, children, stacked = false }: { label: ReactNode; hint?: ReactNode; children?: ReactNode; stacked?: boolean }) {
  return <div className={`st-row ${stacked ? "is-stacked" : ""}`}><div className="st-row-copy"><span>{label}</span>{hint && <small>{hint}</small>}</div>{children && <div className="st-row-control">{children}</div>}</div>;
}

function Appearance() {
  const { preference, accent, saving, error } = useTheme();
  return <Group id="appearance" icon="palette" tone="linear-gradient(135deg,#5e5ce6,#bf5af2)" title="外观" description="浅色为默认外观，也可以跟随 Windows 自动切换。">
    <Row label="外观模式" stacked>
      <div className="st-modes" role="radiogroup" aria-label="外观模式">{modes.map(mode => <label key={mode.id} className="st-mode"><input type="radio" name="theme" value={mode.id} checked={preference === mode.id} onChange={() => chooseTheme(mode.id)}/><span className={`st-mode-sample is-${mode.id}`} aria-hidden="true"><i/><b/><em/></span><span>{mode.name}</span></label>)}</div>
    </Row>
    <Row label="强调色" hint={saving ? "保存中…" : "用于按钮、选中状态与图表"}>
      <div className="st-accents" role="radiogroup" aria-label="强调色">{accents.map(item => <label key={item.id} title={item.name} className="st-accent" style={{ ["--swatch" as string]: item.light }}><input type="radio" name="accent" value={item.id} aria-label={item.name} checked={accent === item.id} onChange={() => chooseAccent(item.id)}/><i/></label>)}</div>
    </Row>
    {error && <p role="alert" className="theme-error st-row">{error}</p>}
  </Group>;
}

export function SettingsPage({ onFocusSettingsSaved }: { onFocusSettingsSaved: () => Promise<void> }) {
  const [message, setMessage] = useState("");
  const [settings, setSettings] = useState<AppSettings>({ idleMinutes: 5, countMediaWhileIdle: true, meetingApps: "", titleExclusions: "", dayStartHour: 0 });
  const [saved, setSaved] = useState<AppSettings | null>(null);
  const [autostart, setAutostartState] = useState(false);
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [version, setVersion] = useState("");
  useEffect(() => {
    Promise.all([api.getSettings(), api.getAutostart(), api.getRuntimeStatus()]).then(([value, auto, status]) => { setSettings(value); setSaved(value); setAutostartState(auto); setRuntime(status); }).catch(reason => setMessage(String(reason)));
    if (api.isDesktop()) getVersion().then(setVersion).catch(() => {}); else setVersion("预览");
  }, []);
  const run = async (action: () => Promise<string | null | void>, success: string) => { try { const result = await action(); if (result) setMessage(`${success}：${result}`); else if (result !== null) setMessage(success); } catch (reason) { setMessage(String(reason)); } };
  const save = async () => { try { const value = await api.updateSettings(settings); setSettings(value); setSaved(value); setMessage("记录设置已保存"); } catch (reason) { setMessage(String(reason)); } };
  const toggleAutostart = async (enabled: boolean) => { try { setAutostartState(await api.setAutostart(enabled)); setMessage(enabled ? "已设置为登录 Windows 后在后台启动" : "已关闭开机启动"); } catch (reason) { setMessage(String(reason)); } };
  const dirty = saved !== null && JSON.stringify(saved) !== JSON.stringify(settings);
  const update = (patch: Partial<AppSettings>) => setSettings(current => ({ ...current, ...patch }));
  return <div className="kd-page st-page" data-testid="settings-page">
    <PageHeader title="设置" subtitle="所有记录、规则与分析都只保存在这台电脑上。"/>
    {message && <button className="kd-message st-message" onClick={() => setMessage("")}>{message}<Icon name="close" size={14}/></button>}
    <div className="st-groups">
      <Appearance/>
      <Group id="general" icon="power" tone="linear-gradient(135deg,#8e8e93,#636366)" title="通用">
        <Row label="登录 Windows 后自动运行" hint="在后台静默启动，托盘图标常驻。"><label className="switch"><input aria-label="开机启动" type="checkbox" checked={autostart} onChange={event => void toggleAutostart(event.target.checked)}/><i/></label></Row>
        <Row label="关闭窗口" hint="关闭主窗口后继续在后台记录；从托盘图标重新打开。"/>
      </Group>
      <Group id="recording" icon="eye" tone="linear-gradient(135deg,#30b0c7,#0a84ff)" title="记录与隐私" description="决定什么时间算作有效使用，以及哪些内容不保存标题。">
        <Row label="闲置阈值" hint="超过该时长没有键鼠输入，会回溯扣除离开时间。"><span className="st-number"><input type="number" min="1" max="60" value={settings.idleMinutes} onChange={event => update({ idleMinutes: Number(event.target.value) })}/>分钟</span></Row>
        <Row label="每日换日点" hint="熬夜用户可以把一天的结束推迟到凌晨。"><span className="st-number"><input type="number" min="0" max="23" value={settings.dayStartHour} onChange={event => update({ dayStartHour: Number(event.target.value) })}/>点</span></Row>
        <Row label="播放音视频时继续计时" hint="Edge 中有视频或音频播放时，不按闲置处理。"><label className="switch"><input type="checkbox" aria-label="播放音视频时继续计时" checked={settings.countMediaWhileIdle} onChange={event => update({ countMediaWhileIdle: event.target.checked })}/><i/></label></Row>
        <Row label="闲置例外应用" hint="会议、播放器等无需输入的应用，用分号分隔。" stacked><input className="st-wide" value={settings.meetingApps} onChange={event => update({ meetingApps: event.target.value })} placeholder="zoom;teams;potplayer"/></Row>
        <Row label="不保存标题的目标" hint="这些应用或网站只记录时长，不保存窗口标题。" stacked><input className="st-wide" value={settings.titleExclusions} placeholder="WeChat.exe;bank.example" onChange={event => update({ titleExclusions: event.target.value })}/></Row>
        <div className="st-row st-actions"><button className="primary" disabled={!dirty} onClick={() => void save()}>{dirty ? "保存记录设置" : "已保存"}</button></div>
      </Group>
      <Group id="focus" icon="focus" tone="linear-gradient(135deg,#ff9f0a,#ff6b35)" title="专注" description="时长调整从下一轮开始生效。">
        <FocusPreferencesSettings onSaved={onFocusSettingsSaved}/>
      </Group>
      <Group id="website" icon="globe" tone="linear-gradient(135deg,#34c759,#0fa36b)" title="Edge 网站记录" description="通过本机扩展记录前台网页，只保存域名、标题与页面路径。">
        <Row label="当前状态" stacked><WebsiteRecordingStatus/></Row>
        <Row label="安装扩展" hint={<ol className="st-steps"><li>在 Edge 打开 edge://extensions 并开启“开发人员模式”</li><li>选择“加载解压缩的扩展”，然后选择刻度扩展文件夹</li><li>打开任意网页，确认扩展显示“正在记录”</li></ol>} stacked><button onClick={() => void run(api.openEdgeExtensionFolder, "扩展文件夹已打开")}><Icon name="external" size={13}/>打开扩展文件夹</button></Row>
      </Group>
      <Group id="data" icon="database" tone="linear-gradient(135deg,#636366,#3a3a3c)" title="数据" description="恢复备份会覆盖当前数据，建议先导出一份备份。">
        <Row label="数据位置" hint={runtime ? <code className="st-path">{runtime.dataPath}</code> : "正在读取…"} stacked/>
        {runtime?.dataEarliestAt && runtime.dataLatestAt && <Row label="记录范围" hint={`${dateFormat.format(runtime.dataEarliestAt)} — ${dateFormat.format(runtime.dataLatestAt)}${runtime.lastWriteAt ? ` · 最近写入 ${timeFormat.format(runtime.lastWriteAt)}` : ""}`}/>}
        {runtime?.collectorError && <Row label={<span className="st-danger">采集异常</span>} hint={runtime.collectorError}/>}
        <div className="st-row st-actions">
          <button onClick={() => void run(api.exportBackup, "备份已保存")}><Icon name="download" size={13}/>导出完整备份</button>
          <button onClick={() => void run(() => api.exportActivityCsv(), "明细已导出")}><Icon name="list" size={13}/>导出 CSV 明细</button>
          <button className="danger-subtle" onClick={() => void run(async () => { const result = await api.restoreBackup(); if (result !== null) await refreshTheme(); return result; }, "已恢复备份")}><Icon name="upload" size={13}/>从备份恢复…</button>
        </div>
      </Group>
      <Group id="shortcuts" icon="keyboard" tone="linear-gradient(135deg,#5ac8fa,#007aff)" title="键盘快捷键">
        <ul className="st-shortcuts">{shortcutList.map(item => <li key={item.keys}><span>{item.label}</span><kbd>{item.keys}</kbd></li>)}</ul>
      </Group>
      <footer className="st-about"><span className="brand-glyph small" aria-hidden="true">刻</span><div><strong>刻度 Kedu</strong><small>版本 {version || "—"} · 本地优先的时间记录与专注工具</small></div></footer>
    </div>
  </div>;
}
