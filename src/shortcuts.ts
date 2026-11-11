export type Page = "today" | "review" | "insights" | "focus" | "rules" | "classification" | "settings";
export const pages: { id: Page; label: string; icon: string; group: string; shortcut: string }[] = [
  { id: "today", label: "今日", icon: "today", group: "概览", shortcut: "Ctrl 1" },
  { id: "review", label: "回顾", icon: "review", group: "概览", shortcut: "Ctrl 2" },
  { id: "insights", label: "洞察", icon: "insights", group: "概览", shortcut: "Ctrl 3" },
  { id: "focus", label: "专注", icon: "focus", group: "习惯", shortcut: "Ctrl 4" },
  { id: "rules", label: "限制", icon: "limits", group: "习惯", shortcut: "Ctrl 5" },
  { id: "classification", label: "分类", icon: "classification", group: "整理", shortcut: "Ctrl 6" },
  { id: "settings", label: "设置", icon: "settings", group: "", shortcut: "Ctrl ," },
];
export const shortcutList = [
  { label: "搜索与快速操作", keys: "Ctrl K" },
  ...pages.map(page => ({ label: `前往${page.label}`, keys: page.shortcut })),
  { label: "切换浅色 / 深色", keys: "Ctrl Shift L" },
  { label: "关闭对话框", keys: "Esc" },
];
