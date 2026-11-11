import { useMemo, useRef, useState, type KeyboardEvent } from "react";
import type { ClassificationRule, LimitRule, TargetCatalog } from "./types";
import { ApplicationIcon, useApplicationIcons } from "./components/ApplicationIcon";
import "./LimitsWorkspace.css";

type LimitsWorkspaceProps = {
  mode?: "limits" | "categories";
  rules: LimitRule[];
  catalog: TargetCatalog;
  classifications: ClassificationRule[];
  openRule: (rule: LimitRule | null) => void;
  openClassification: (rule: ClassificationRule | null) => void;
  toggleRule: (id: number, enabled: boolean) => void;
  deleteRule: (id: number) => void;
  toggleClassification: (id: number, enabled: boolean) => void;
  deleteClassification: (id: number) => void;
};
type Tab = "limits" | "categories";
type StatusFilter = "all" | "enabled" | "disabled";
const tabs: { id: Tab; label: string }[] = [{ id: "limits", label: "限制与额度" }, { id: "categories", label: "统计分类" }];
const filters: { id: StatusFilter; label: string }[] = [{ id: "all", label: "全部" }, { id: "enabled", label: "启用" }, { id: "disabled", label: "停用" }];
const kindLabel = (kind: string) => kind === "application" ? "应用" : kind === "website" ? "网站" : kind === "title" ? "窗口标题" : "限制组";
const duration = (seconds: number) => {
  const value = Math.max(0, Math.floor(seconds));
  const hours = Math.floor(value / 3600), minutes = Math.floor((value % 3600) / 60);
  return hours ? `${hours}小时${minutes ? ` ${minutes}分` : ""}` : minutes ? `${minutes}分钟` : `${value}秒`;
};

export function LimitsWorkspace({ mode, rules, catalog, classifications, openRule, openClassification, toggleRule, deleteRule, toggleClassification, deleteClassification }: LimitsWorkspaceProps) {
  const [localTab, setTab] = useState<Tab>("limits");
  const tab = mode ?? localTab;
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<StatusFilter>("all");
  const tabButtons = useRef<(HTMLButtonElement | null)[]>([]);
  const applications = useMemo(() => new Map(catalog.applications.flatMap(app => [
    [app.executableName.toLocaleLowerCase(), app] as const,
    [app.path.toLocaleLowerCase(), app] as const,
  ])), [catalog.applications]);
  // Installed apps can have no activity history yet. Resolve their local path
  // for icons without changing the stable executable stored in the rule.
  const iconTarget = (target: string) => applications.get(target.toLocaleLowerCase())?.path || target;
  const icons = useApplicationIcons([...rules, ...classifications].filter(rule => rule.targetKind === "application").map(rule => iconTarget(rule.targetPattern)));
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const matches = (rule: LimitRule | ClassificationRule) => (filter === "all" || rule.enabled === (filter === "enabled")) &&
    `${rule.name} ${rule.targetPattern} ${"groupName" in rule ? rule.groupName : rule.category} ${kindLabel(rule.targetKind)}`.toLocaleLowerCase().includes(normalizedQuery);
  const visibleRules = rules.filter(matches);
  const visibleClassifications = classifications.filter(matches);
  const source = tab === "limits" ? rules : classifications;
  const visibleCount = tab === "limits" ? visibleRules.length : visibleClassifications.length;
  const filtered = Boolean(query.trim()) || filter !== "all";
  const add = () => tab === "limits" ? openRule(null) : openClassification(null);
  const changeTab = (next: Tab) => { setTab(next); setQuery(""); setFilter("all"); };
  const navigateTabs = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    let next = index;
    if (event.key === "ArrowRight" || event.key === "ArrowLeft") next = 1 - index;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = tabs.length - 1;
    else return;
    event.preventDefault(); changeTab(tabs[next].id); tabButtons.current[next]?.focus();
  };

  return <div className="kd-page lw-workspace">
    {!mode && <h1 className="visually-hidden">限制</h1>}
    <header className="lw-header">
      {!mode ? <div className="lw-tabs" role="tablist" aria-label="限制设置">
        {tabs.map((item, index) => <button key={item.id} ref={node => { tabButtons.current[index] = node; }} type="button" role="tab" id={`lw-tab-${item.id}`} aria-selected={tab === item.id} aria-controls={`lw-panel-${item.id}`} tabIndex={tab === item.id ? 0 : -1} onKeyDown={event => navigateTabs(event, index)} onClick={() => changeTab(item.id)}>{item.label}<span>{item.id === "limits" ? rules.length : classifications.length}</span></button>)}
      </div> : <div className="kd-page-heading"><h1 id={`lw-tab-${tab}`}>{tab === "limits" ? "限制" : "自动分类规则"}</h1><p>{tab === "limits" ? "为容易分心的应用设置每日额度：接近用完时提醒，用完后拦截。可重叠的限制组会对同一时间去重。" : "按应用、网站或窗口标题自动归类。标题规则优先于网站，网站优先于应用；单段手动纠正始终优先。"}</p></div>}
      <button type="button" className="lw-add primary" onClick={add}><span aria-hidden="true">＋</span>{tab === "limits" ? "新增限制" : "新增规则"}</button>
    </header>

    <section id={`lw-panel-${tab}`} role="tabpanel" aria-labelledby={`lw-tab-${tab}`} tabIndex={0} className="lw-panel">
      <div className="lw-tools">
        <label className="lw-search"><svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7"><circle cx="10.5" cy="10.5" r="6.5"/><path d="m16 16 4 4"/></svg><span className="visually-hidden">{tab === "limits" ? "搜索限制" : "搜索分类"}</span><input type="search" value={query} onChange={event => setQuery(event.target.value)} placeholder={tab === "limits" ? "搜索名称或限制组" : "搜索名称或分类"}/></label>
        <div className="lw-filters" role="group" aria-label="按状态筛选">{filters.map(item => <button key={item.id} type="button" aria-pressed={filter === item.id} onClick={() => setFilter(item.id)}>{item.label}</button>)}</div>
        <span className="lw-count" aria-live="polite">{filtered ? `${visibleCount} / ${source.length}` : source.length} 项</span>
      </div>

      {visibleCount > 0 ? <ul className={`lw-list ${tab === "categories" ? "lw-classifications" : ""}`}>
        {tab === "limits" ? visibleRules.map(rule => {
          const limitSeconds = Math.max(1, rule.dailyLimitMinutes * 60);
          const used = Math.max(0, rule.usedSeconds);
          const remaining = Math.max(0, rule.remainingSeconds);
          const exhausted = remaining <= 0;
          return <li key={rule.id} className={`lw-row ${!rule.enabled ? "is-disabled" : exhausted ? "is-exhausted" : ""}`}>
            <div className="lw-identity">
              {rule.targetKind === "application" ? <ApplicationIcon name={rule.name} target={iconTarget(rule.targetPattern)} icons={icons}/> : <span className="lw-target-mark" aria-hidden="true">{rule.targetKind === "group" ? "◇" : "◎"}</span>}
              <div><strong title={rule.name}>{rule.name}</strong><p title={rule.groupName || kindLabel(rule.targetKind)}><span>{kindLabel(rule.targetKind)}</span>{rule.groupName && <><i aria-hidden="true">·</i><span>{rule.groupName}</span></>}{rule.targetKind === "website" && <><i aria-hidden="true">·</i><span>{rule.targetPattern}</span></>}</p></div>
            </div>
            <div className="lw-quota">
              <div className="lw-quota-copy"><span>已用 <b>{duration(used)}</b></span><span>每天 {rule.dailyLimitMinutes} 分钟</span></div>
              <progress max={limitSeconds} value={Math.min(used, limitSeconds)} aria-label={`${rule.name}今日用量`} aria-valuetext={`今日已用${duration(used)}，每日额度${rule.dailyLimitMinutes}分钟`}/>
              <small>{!rule.enabled ? "已关闭限制" : exhausted ? "今日额度已用尽" : `剩余 ${duration(remaining)}`}</small>
            </div>
            <RowActions name={rule.name} enabled={rule.enabled} onToggle={enabled => toggleRule(rule.id, enabled)} onEdit={() => openRule(rule)} onDelete={() => deleteRule(rule.id)}/>
          </li>;
        }) : visibleClassifications.map(rule => {
          const app = rule.targetKind === "application" ? applications.get(rule.targetPattern.toLocaleLowerCase()) : undefined;
          const target = rule.targetKind === "application" ? app?.displayName ?? rule.name : rule.targetPattern;
          const showTarget = target.toLocaleLowerCase() !== rule.name.toLocaleLowerCase();
          return <li key={rule.id} className={`lw-row ${!rule.enabled ? "is-disabled" : ""}`}>
            <div className="lw-identity">
              {rule.targetKind === "application" ? <ApplicationIcon name={rule.name} target={iconTarget(rule.targetPattern)} icons={icons}/> : <span className="lw-target-mark" aria-hidden="true">{rule.targetKind === "title" ? "T" : "◎"}</span>}
              <div><strong title={rule.name}>{rule.name}</strong><p title={target}><span>{kindLabel(rule.targetKind)}</span>{showTarget && <><i aria-hidden="true">·</i><span>{target}</span></>}</p></div>
            </div>
            <div className="lw-category"><span title={rule.category}>{rule.category}</span><small>{rule.enabled ? "参与统计" : "已停用"}</small></div>
            <RowActions name={rule.name} enabled={rule.enabled} onToggle={enabled => toggleClassification(rule.id, enabled)} onEdit={() => openClassification(rule)} onDelete={() => deleteClassification(rule.id)}/>
          </li>;
        })}
      </ul> : <div className="lw-empty">
        <h2>{source.length ? "没有符合条件的规则" : tab === "limits" ? "还没有限制" : "还没有分类规则"}</h2>
        {source.length ? <button type="button" className="lw-empty-action" onClick={() => { setQuery(""); setFilter("all"); }}>清除筛选</button> : <button type="button" className="lw-add primary" onClick={add}>{tab === "limits" ? "新增限制" : "新增规则"}</button>}
      </div>}
    </section>
  </div>;
}

function RowActions({ name, enabled, onToggle, onEdit, onDelete }: { name: string; enabled: boolean; onToggle: (enabled: boolean) => void; onEdit: () => void; onDelete: () => void }) {
  return <div className="lw-actions">
    <label className="lw-switch"><input type="checkbox" aria-label={`${name}状态`} checked={enabled} onChange={event => onToggle(event.target.checked)}/><i aria-hidden="true"/></label>
    <div><button type="button" aria-label={`编辑${name}`} onClick={onEdit}>编辑</button><button type="button" className="lw-delete" aria-label={`删除${name}`} onClick={onDelete}>删除</button></div>
  </div>;
}
