import { useEffect, useState, type RefObject } from "react";
import type { ClassificationRule, NewClassificationRule, TargetCatalog } from "../types";
import { TargetPicker } from "./TargetPicker";

export function ClassificationDialog({ dialogRef, editing, catalog, onSave }: {
  dialogRef: RefObject<HTMLDialogElement | null>;
  editing: ClassificationRule | null;
  catalog: TargetCatalog;
  onSave: (rule: NewClassificationRule) => Promise<void>;
}) {
  const [kind, setKind] = useState<NewClassificationRule["targetKind"]>("application");
  const [target, setTarget] = useState("");
  const [name, setName] = useState("");
  const [category, setCategory] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const reset = () => {
    setKind(editing?.targetKind ?? "application");
    setTarget(editing?.targetPattern ?? "");
    setName(editing?.name ?? "");
    setCategory(editing?.category ?? "");
    setError("");
  };
  useEffect(reset, [editing]);
  const title = editing ? "编辑分类规则" : "新增分类规则";

  return <dialog ref={dialogRef} className="modal target-modal lw-editor lw-classification-editor" aria-label={title} onClose={reset}>
    <form onSubmit={async event => {
      event.preventDefault();
      if (busy) return;
      setBusy(true); setError("");
      try { await onSave({ name: name.trim(), targetKind: kind, targetPattern: target.trim(), category: category.trim() }); }
      catch (reason) { setError(reason instanceof Error ? reason.message : String(reason)); }
      finally { setBusy(false); }
    }}>
      <header><h2>{title}</h2><button type="button" className="icon-button" aria-label="关闭" onClick={() => dialogRef.current?.close()}>×</button></header>
      <fieldset className="lw-editor-section"><legend className="visually-hidden">分类目标</legend><div className="lw-editor-target">
        <label>匹配层级<select aria-label="匹配层级" value={kind} onChange={event => { setKind(event.target.value as typeof kind); setTarget(""); setName(""); }}>
          <option value="application">应用</option><option value="website">网站</option><option value="title">窗口标题</option>
        </select></label>
        {kind === "title" ? <label>窗口标题包含<input aria-label="窗口标题包含" value={target} onChange={event => { setTarget(event.target.value); if (!name) setName(event.target.value); }} placeholder="输入标题关键词" required/></label>
          : <div className="form-field"><span>选择目标</span><TargetPicker kind={kind} value={target} catalog={catalog} onChange={(value, displayName) => { setTarget(value); setName(displayName); }}/></div>}
      </div></fieldset>
      <fieldset className="lw-editor-section"><legend className="visually-hidden">规则名称与分类</legend><div className="lw-editor-grid">
        <label>规则名称<input aria-label="规则名称" value={name} onChange={event => setName(event.target.value)} required/></label>
        <label>归入分类<input aria-label="归入分类" value={category} onChange={event => setCategory(event.target.value)} placeholder="学习、工作、娱乐" required/></label>
      </div></fieldset>
      {error && <p role="alert" className="form-error">{error}</p>}
      <footer><button type="button" className="secondary" onClick={() => dialogRef.current?.close()}>取消</button><button type="submit" className="primary" disabled={busy || !target.trim() || !name.trim() || !category.trim()}>{busy ? "保存中…" : "保存规则"}</button></footer>
    </form>
  </dialog>;
}
