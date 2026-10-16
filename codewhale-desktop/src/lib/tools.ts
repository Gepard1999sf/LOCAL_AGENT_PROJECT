import {
  Brain,
  FileEdit,
  FilePlus,
  FileText,
  FolderTree,
  Globe,
  ListTodo,
  Search,
  SquareTerminal,
  Wrench,
  GitBranch,
  Bot,
  type LucideIcon,
} from "lucide-react";

export interface ToolMeta {
  icon: LucideIcon;
  label: string;
  risk: "safe" | "write" | "exec" | "network";
}

const TOOLS: Record<string, ToolMeta> = {
  read: { icon: FileText, label: "Чтение файла", risk: "safe" },
  read_file: { icon: FileText, label: "Чтение файла", risk: "safe" },
  handle_read: { icon: FileText, label: "Чтение", risk: "safe" },
  list_dir: { icon: FolderTree, label: "Список файлов", risk: "safe" },
  file_search: { icon: Search, label: "Поиск файлов", risk: "safe" },
  grep_files: { icon: Search, label: "Поиск по коду", risk: "safe" },
  tool_search: { icon: Search, label: "Поиск инструментов", risk: "safe" },
  project_map: { icon: FolderTree, label: "Карта проекта", risk: "safe" },
  write: { icon: FilePlus, label: "Запись файла", risk: "write" },
  edit: { icon: FileEdit, label: "Правка файла", risk: "write" },
  apply_patch: { icon: FileEdit, label: "Патч", risk: "write" },
  fim_edit: { icon: FileEdit, label: "Правка", risk: "write" },
  bash: { icon: SquareTerminal, label: "Команда", risk: "exec" },
  Run: { icon: SquareTerminal, label: "Запуск", risk: "exec" },
  execute_tools: { icon: SquareTerminal, label: "Скрипт инструментов", risk: "exec" },
  task_shell_start: { icon: SquareTerminal, label: "Фоновая команда", risk: "exec" },
  Web: { icon: Globe, label: "Веб", risk: "network" },
  web_search: { icon: Globe, label: "Веб-поиск", risk: "network" },
  fetch_url: { icon: Globe, label: "Загрузка URL", risk: "network" },
  Git: { icon: GitBranch, label: "Git", risk: "exec" },
  todo_write: { icon: ListTodo, label: "План задач", risk: "safe" },
  agent: { icon: Bot, label: "Субагент", risk: "exec" },
  think: { icon: Brain, label: "Размышление", risk: "safe" },
};

export function toolMeta(name?: string | null): ToolMeta {
  if (!name) return { icon: Wrench, label: "Инструмент", risk: "exec" };
  return TOOLS[name] ?? { icon: Wrench, label: name, risk: "exec" };
}

/** Tools listed in Settings → auto-approve. */
export const APPROVABLE_TOOLS: Array<{ name: string; label: string; risk: ToolMeta["risk"] }> = [
  { name: "read", label: "read: чтение файлов", risk: "safe" },
  { name: "grep_files", label: "grep_files: поиск по коду", risk: "safe" },
  { name: "list_dir", label: "list_dir: список файлов", risk: "safe" },
  { name: "file_search", label: "file_search: поиск файлов", risk: "safe" },
  { name: "edit", label: "edit: правка файлов", risk: "write" },
  { name: "write", label: "write: запись файлов", risk: "write" },
  { name: "apply_patch", label: "apply_patch: патчи", risk: "write" },
  { name: "bash", label: "bash: команды PowerShell", risk: "exec" },
  { name: "Web", label: "Web: доступ в интернет", risk: "network" },
  { name: "ai_drive", label: "ai-drive: диск знаний (поиск, документация, проверка Godot)", risk: "safe" },
];

/** Tools given to local models in the "lite" profile: fewer schemas, ~6k tokens less prompt. */
export const LITE_TOOLS = ["read", "write", "edit", "bash", "todo_write"];

/** Best-effort extraction of a tool name from an approval or item payload. */
export function findToolName(obj: any): string | undefined {
  if (!obj || typeof obj !== "object") return undefined;
  const direct =
    obj.tool_name ?? obj.toolName ?? obj.tool ?? obj.name ?? obj.function?.name ?? obj.request?.tool_name;
  if (typeof direct === "string" && direct) return direct;
  for (const key of ["request", "call", "tool_call", "metadata", "payload", "action"]) {
    const v = obj[key];
    if (v && typeof v === "object") {
      const n = findToolName(v);
      if (n) return n;
    }
  }
  return undefined;
}

/** Best-effort extraction of arguments (command / path) for display. */
export function findToolArgs(obj: any): Record<string, unknown> | undefined {
  if (!obj || typeof obj !== "object") return undefined;
  const raw = obj.tool_input ?? obj.arguments ?? obj.args ?? obj.input ?? obj.params ?? obj.request?.arguments;
  if (typeof raw === "string") {
    try {
      return JSON.parse(raw);
    } catch {
      return { value: raw };
    }
  }
  if (raw && typeof raw === "object") return raw as Record<string, unknown>;
  for (const key of ["request", "call", "tool_call", "metadata", "payload"]) {
    const v = obj[key];
    if (v && typeof v === "object") {
      const a = findToolArgs(v);
      if (a) return a;
    }
  }
  return undefined;
}

export function primaryArg(args?: Record<string, unknown>): string | undefined {
  if (!args) return undefined;
  for (const k of ["command", "path", "file_path", "file", "pattern", "query", "url", "value"]) {
    const v = args[k];
    if (typeof v === "string" && v) return v;
  }
  return undefined;
}
