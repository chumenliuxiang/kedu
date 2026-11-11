import { useState } from "react";
import type { FocusTask } from "./types";

/** A valid example: taskId is either an existing task's numeric ID or null. */
export function createScheduleExample(tasks: FocusTask[], task?: FocusTask): string {
  return JSON.stringify({
    name: "学习日",
    availableTasks: tasks.map(({ id, name }) => ({ id, name })),
    items: [
      { label: task?.name ?? "英语精读", start: "09:00", durationMinutes: task?.durationMinutes ?? 50, mode: task?.mode ?? "normal", taskId: task?.id ?? null },
      { label: "复盘整理", start: "20:30", durationMinutes: 30, mode: "strict", taskId: null },
    ],
  }, null, 2);
}

export function ScheduleImportGuide({ tasks, setDraft }: { tasks: FocusTask[]; setDraft: (draft: string) => void }) {
  const [exampleTask, setExampleTask] = useState("");
  return <details>
    <summary>示例和任务关联说明</summary>
    <p className="fw-field-note">示例中的 availableTasks 列出当前可关联的任务，仅供参考。每个时段的 taskId 填对应任务编号；填 null 或省略 taskId 表示不关联。时段名称、时间、时长和模式仍可自行修改。预览后也可以选择关联任务。</p>
    {tasks.length ? <ul>{tasks.map(task => <li key={task.id}>{task.name}：<code>{task.id}</code></li>)}</ul> : <p className="fw-field-note">目前没有可关联的任务，可先在任务页添加任务，或选择不关联。</p>}
    <label>示例中第一个时段关联的任务<select value={exampleTask} onChange={event => setExampleTask(event.target.value)}><option value="">不关联任务</option>{tasks.map(task => <option key={task.id} value={task.id}>{task.name}（编号 {task.id}）</option>)}</select></label>
    <p className="fw-field-note">生成示例会替换当前 JSON 内容。</p>
    <button type="button" className="secondary" onClick={() => setDraft(createScheduleExample(tasks, tasks.find(task => String(task.id) === exampleTask)))}>生成示例 JSON</button>
  </details>;
}
