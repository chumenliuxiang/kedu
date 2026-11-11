import { useEffect, useMemo, useRef, useState } from "react";
import { Icon } from "./Icon";

export type Command = { id: string; title: string; subtitle?: string; icon: string; group: string; keywords?: string; shortcut?: string; run: () => void };

export function CommandPalette({ commands, close }: { commands: Command[]; close: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  useEffect(() => { const before = document.activeElement; dialog.current?.showModal(); return () => { if (before instanceof HTMLElement && before.isConnected) before.focus(); }; }, []);
  const results = useMemo(() => {
    const terms = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
    return commands.filter(command => { const text = `${command.title} ${command.subtitle ?? ""} ${command.keywords ?? ""} ${command.group}`.toLocaleLowerCase(); return terms.every(term => text.includes(term)); });
  }, [commands, query]);
  useEffect(() => setIndex(0), [query]);
  useEffect(() => { list.current?.querySelector<HTMLElement>(`[data-index="${index}"]`)?.scrollIntoView({ block: "nearest" }); }, [index]);
  const execute = (command: Command | undefined) => { if (!command) return; close(); requestAnimationFrame(() => command.run()); };
  let lastGroup = "";
  return <dialog ref={dialog} className="kd-palette" aria-label="快速操作" onCancel={event => { event.preventDefault(); close(); }} onClick={event => { if (event.target === dialog.current) close(); }}>
    <div className="kd-palette-search"><Icon name="search" size={17}/><input autoFocus value={query} placeholder="搜索页面、任务或操作…" aria-label="搜索命令" onChange={event => setQuery(event.target.value)} onKeyDown={event => {
      if (event.key === "ArrowDown") { event.preventDefault(); setIndex(value => Math.min(results.length - 1, value + 1)); }
      else if (event.key === "ArrowUp") { event.preventDefault(); setIndex(value => Math.max(0, value - 1)); }
      else if (event.key === "Enter") { event.preventDefault(); execute(results[index]); }
    }}/><kbd>Esc</kbd></div>
    <div className="kd-palette-list" ref={list} role="listbox" aria-label="命令">
      {results.length ? results.map((command, position) => {
        const header = command.group !== lastGroup ? command.group : ""; lastGroup = command.group;
        return <div key={command.id}>{header && <small className="kd-palette-group">{header}</small>}
          <button type="button" role="option" data-index={position} aria-selected={position === index} onPointerMove={() => setIndex(position)} onClick={() => execute(command)}>
            <span className="kd-palette-icon"><Icon name={command.icon} size={15}/></span>
            <span className="kd-palette-copy"><b>{command.title}</b>{command.subtitle && <em>{command.subtitle}</em>}</span>
            {command.shortcut && <kbd>{command.shortcut}</kbd>}
          </button></div>;
      }) : <p className="kd-palette-empty">没有找到“{query}”</p>}
    </div>
    <footer className="kd-palette-footer"><span><kbd>↑</kbd><kbd>↓</kbd>选择</span><span><kbd>Enter</kbd>执行</span></footer>
  </dialog>;
}
