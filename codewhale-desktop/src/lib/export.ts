import { save } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import { api } from "./tauri";
import { threadViewFromDetail, useThreads, type ThreadView } from "../store/threads";
import { useApp } from "../store/app";
import type { ThreadDetail } from "./types";

function safeName(s: string) {
  return (s || "session").replace(/[\\/:*?"<>|\r\n]+/g, " ").trim().slice(0, 60) || "session";
}

export function threadToMarkdown(threadId: string, view?: ThreadView): string {
  const t = view ?? useThreads.getState().threads[threadId];
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
      if (id.startsWith("input:")) {
        const u = t.userInputs[id.slice(6)];
        if (!u) continue;
        for (const q of u.questions) {
          const ans = u.answers?.filter((a) => a.id === q.id).map((a) => a.value) ?? [];
          lines.push(`> ❓ ${q.question}`, `> → ${ans.length ? ans.join(", ") : u.status}`, "");
        }
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

/**
 * Complete export data for a thread, independent of whether (and how far) its event
 * stream was replayed: the stream replays a bounded tail, GET /v1/threads/{id} does not.
 * The loaded view is preferred when it holds everything, since it also carries
 * decided approvals and answers given in this window.
 */
export async function loadThreadForExport(threadId: string): Promise<{ view?: ThreadView; detail?: ThreadDetail }> {
  const local = useThreads.getState().threads[threadId];
  let detail: ThreadDetail | undefined;
  try {
    detail = await api.request<ThreadDetail>("GET", `/v1/threads/${threadId}`);
  } catch (e) {
    if (!local) throw e;
  }
  if (!detail) return { view: local };
  const complete =
    local?.live && local.turns.length >= detail.turns.length && Object.keys(local.items).length >= detail.items.length;
  return { view: complete ? local : threadViewFromDetail(detail), detail };
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
    const { view, detail } = await loadThreadForExport(threadId);
    const content =
      format === "md" ? threadToMarkdown(threadId, view) : JSON.stringify(detail ?? view ?? {}, null, 2);
    await api.writeExport(path, content);
    toast.success("Сессия экспортирована", { description: path });
  } catch (e) {
    toast.error("Экспорт не удался", { description: String(e) });
  }
}

export const exportThreadMarkdown = (threadId: string) => exportThread(threadId, "md");
