import { save } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import { api } from "./tauri";
import { useThreads } from "../store/threads";
import { useApp } from "../store/app";

function safeName(s: string) {
  return (s || "session").replace(/[\\/:*?"<>|\r\n]+/g, " ").trim().slice(0, 60) || "session";
}

export function threadToMarkdown(threadId: string): string {
  const t = useThreads.getState().threads[threadId];
  const summary = useApp.getState().summaries.find((s) => s.id === threadId);
  if (!t) return "";
  const lines: string[] = [];
  lines.push(`# ${summary?.title ?? "Сессия CodeWhale"}`);
  lines.push("");
  lines.push(`- Модель: ${t.thread?.model ?? summary?.model ?? "?"}`);
  lines.push(`- Проект: ${t.thread?.workspace ?? summary?.workspace ?? "?"}`);
  lines.push(`- Экспорт: ${new Date().toLocaleString("ru-RU")}`);
  lines.push("");
  for (const turn of t.turns) {
    if (turn.prompt) {
      lines.push(`## 👤 Вы`);
      lines.push("");
      lines.push(turn.prompt);
      lines.push("");
    }
    lines.push(`## 🐋 CodeWhale`);
    lines.push("");
    for (const id of turn.blocks) {
      if (id.startsWith("approval:")) {
        const a = t.approvals[id.slice(9)];
        if (a) lines.push(`> Подтверждение \`${a.toolName ?? "tool"}\`: ${a.status}`, "");
        continue;
      }
      const it = t.items[id];
      if (!it) continue;
      const text = it.text || it.detail;
      if (it.kind === "agent_message") lines.push(text, "");
      else if (it.kind === "agent_reasoning") lines.push("<details><summary>Размышления</summary>", "", text, "", "</details>", "");
      else if (["tool_call", "command_execution", "file_change"].includes(it.kind) || it.toolName) {
        lines.push(`**🔧 ${it.toolName ?? it.kind}** — ${it.summary}`, "");
        if (it.toolArgs) lines.push("```json", JSON.stringify(it.toolArgs, null, 2), "```", "");
        if (text) lines.push("```", text.length > 4000 ? text.slice(0, 4000) + "\n…" : text, "```", "");
      } else if (text) lines.push(`_${text}_`, "");
    }
    if (turn.error) lines.push(`> ⚠ ${turn.error}`, "");
  }
  return lines.join("\n");
}

export function threadToJson(threadId: string): string {
  const t = useThreads.getState().threads[threadId];
  return JSON.stringify(t ?? {}, null, 2);
}

export async function exportThread(threadId: string, format: "md" | "json") {
  const summary = useApp.getState().summaries.find((s) => s.id === threadId);
  const path = await save({
    title: "Экспорт сессии",
    defaultPath: `${safeName(summary?.title ?? threadId)}.${format}`,
    filters: [format === "md" ? { name: "Markdown", extensions: ["md"] } : { name: "JSON", extensions: ["json"] }],
  });
  if (!path) return;
  try {
    await api.writeExport(path, format === "md" ? threadToMarkdown(threadId) : threadToJson(threadId));
    toast.success("Сессия экспортирована", { description: path });
  } catch (e) {
    toast.error("Экспорт не удался", { description: String(e) });
  }
}

export const exportThreadMarkdown = (threadId: string) => exportThread(threadId, "md");
