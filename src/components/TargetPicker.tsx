import { useEffect, useMemo, useRef, useState } from "react";
import * as api from "../api";
import type { TargetApplication, TargetCatalog } from "../types";
import { ApplicationIcon, useApplicationIcons } from "./ApplicationIcon";

type TargetKind="application"|"website"|"group";

export function TargetPicker({kind,value,catalog,onChange,label="选择目标"}:{kind:TargetKind;value:string;catalog:TargetCatalog;onChange:(value:string,displayName:string,path?:string)=>void;label?:string}){
  const[open,setOpen]=useState(false);const[query,setQuery]=useState("");const[extras,setExtras]=useState<TargetApplication[]>([]);const root=useRef<HTMLDivElement>(null);const searchRef=useRef<HTMLInputElement>(null);
  const applications=useMemo(()=>{const seen=new Set<string>();return[...extras,...catalog.applications].filter(app=>{const key=app.executableName.toLowerCase();if(seen.has(key))return false;seen.add(key);return true})},[catalog.applications,extras]);
  const selected=applications.find(app=>app.executableName.toLowerCase()===value.toLowerCase());
  const normalized=query.trim().toLowerCase();
  const filteredApps=applications.filter(app=>`${app.displayName} ${app.executableName}`.toLowerCase().includes(normalized)).slice(0,80);
  const options=(kind==="website"?catalog.websites:catalog.groups).filter(item=>item.toLowerCase().includes(normalized));
  const icons=useApplicationIcons(kind === "application" ? [selected?.path || "", ...(open ? filteredApps.slice(0,63).map(app=>app.path||app.executableName) : [])] : []);
  useEffect(()=>{if(!open)return;const close=(event:PointerEvent)=>{if(!root.current?.contains(event.target as Node))setOpen(false)};document.addEventListener("pointerdown",close);requestAnimationFrame(()=>searchRef.current?.focus());return()=>document.removeEventListener("pointerdown",close)},[open]);
  const choose=(next:string,name:string,path?:string)=>{onChange(next,name,path);setOpen(false);setQuery("")};
  const browse=async()=>{const app=await api.pickApplicationExecutable();if(!app)return;setExtras(current=>[app,...current]);choose(app.executableName,app.displayName,app.path)};
  const currentLabel=selected?.displayName||value||label;
  return <div className="target-picker" ref={root}>
    <button type="button" className={`target-trigger ${value?"selected":""}`} aria-haspopup="listbox" aria-expanded={open} onClick={()=>setOpen(current=>!current)}>
      {kind==="application"&&value?<ApplicationIcon name={currentLabel} target={selected?.path||value} icons={icons} className="picker-trigger-icon"/>:<span className="target-kind-mark" aria-hidden="true">{kind==="website"?"◎":kind==="group"?"◇":""}</span>}
      <span>{currentLabel}</span><b aria-hidden="true">⌄</b>
    </button>
    {open&&<div className="target-menu">
      <label className="target-search"><span className="visually-hidden">搜索目标</span><input ref={searchRef} value={query} onChange={event=>setQuery(event.target.value)} onKeyDown={event=>{if(event.key==="Escape")setOpen(false)}} placeholder={kind==="application"?"搜索应用或 EXE":kind==="website"?"搜索或输入网站":"搜索限制组"}/></label>
      <div className="target-options" role="listbox" aria-label={label}>
        {kind==="application"?<>
          {filteredApps.some(app=>app.lastSeenAt!==null)&&<small>最近使用</small>}
          {filteredApps.filter(app=>app.lastSeenAt!==null).map(app=><TargetApplicationOption key={`recent-${app.executableName}`} app={app} icons={icons} selected={value.toLowerCase()===app.executableName.toLowerCase()} choose={choose}/>)}
          {filteredApps.some(app=>app.lastSeenAt===null)&&<small>本机应用</small>}
          {filteredApps.filter(app=>app.lastSeenAt===null).map(app=><TargetApplicationOption key={`installed-${app.executableName}`} app={app} icons={icons} selected={value.toLowerCase()===app.executableName.toLowerCase()} choose={choose}/>)}
          {!filteredApps.length&&<p>没有找到应用</p>}
        </>:<>
          {options.map(item=><button type="button" role="option" aria-selected={value===item} key={item} onClick={()=>choose(item,item)}><span className="target-kind-mark" aria-hidden="true">{kind==="website"?"◎":"◇"}</span><span><b>{item}</b></span></button>)}
          {kind==="website"&&query.trim()&&!catalog.websites.some(item=>item.toLowerCase()===query.trim().toLowerCase())&&<button type="button" className="custom-target" onClick={()=>choose(query.trim(),query.trim())}>使用“{query.trim()}”</button>}
          {!options.length&&!(kind==="website"&&query.trim())&&<p>没有可选目标</p>}
        </>}
      </div>
      {kind==="application"&&<button type="button" className="browse-application" onClick={()=>void browse()}>从电脑选择应用…</button>}
    </div>}
  </div>;
}

function TargetApplicationOption({app,icons,selected,choose}:{app:TargetApplication;icons:ReturnType<typeof useApplicationIcons>;selected:boolean;choose:(value:string,name:string,path?:string)=>void}){
  return <button type="button" role="option" aria-selected={selected} onClick={()=>choose(app.executableName,app.displayName,app.path)}><ApplicationIcon name={app.displayName} target={app.path||app.executableName} icons={icons} className="picker-option-icon"/><span><b>{app.displayName}</b><em>{app.executableName}</em></span>{selected&&<i aria-hidden="true">✓</i>}</button>;
}
