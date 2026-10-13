import { Plus, MessageSquare } from "lucide-react";
import { useApp } from "../../store/app";
import { Button } from "../ui/primitives";
import { fmtTime } from "../../lib/format";

const EXAMPLES = [
  "Прочитай структуру проекта и кратко опиши, что где лежит",
  "Найди возможные ошибки в main.rs и предложи исправления",
  "Напиши тесты для функций в этом файле: @",
];

export function EmptyState() {
  const newThread = useApp((s) => s.newThread);
  const summaries = useApp((s) => s.summaries);
  const openThread = useApp((s) => s.openThread);
  const ws = useApp((s) => s.settings?.workspace);
  return (
    <div className="flex min-h-0 flex-1 items-center justify-center overflow-y-auto px-6">
      <div className="anim-msg-in flex max-w-md flex-col items-center text-center">
        <div className="mb-3 text-5xl">🐋</div>
        <h1 className="text-[20px] font-semibold">Добро пожаловать в CodeWhale Desktop</h1>
        <p className="mt-1 text-[13px] text-fg2">Локальный AI-агент для программирования. Всё работает на вашем компьютере, без облака.</p>
        {ws && <p className="mt-1 truncate text-[12px] text-fg3">Проект: {ws}</p>}
        <Button variant="primary" className="mt-5 px-4 py-2" onClick={() => newThread()}>
          <Plus size={15} /> Новая сессия
        </Button>
        <div className="mt-6 w-full space-y-1.5 text-left">
          <div className="text-[11px] uppercase tracking-wide text-fg3">Попробуйте</div>
          {EXAMPLES.map((e) => (
            <button
              key={e}
              onClick={() => window.dispatchEvent(new CustomEvent("composer:set", { detail: e }))}
              className="block w-full truncate rounded-[var(--radius-sm)] border border-line-muted px-3 py-1.5 text-left text-[12.5px] text-fg2 hover:border-line hover:bg-elevated hover:text-fg"
            >
              {e}
            </button>
          ))}
        </div>
        {summaries.length > 0 && (
          <div className="mt-6 w-full text-left">
            <div className="mb-1 text-[11px] uppercase tracking-wide text-fg3">Или откройте недавнюю</div>
            {summaries.slice(0, 4).map((s) => (
              <button
                key={s.id}
                onClick={() => openThread(s.id)}
                className="flex w-full items-center gap-2 rounded-[var(--radius-sm)] px-2 py-1.5 text-left text-[13px] hover:bg-elevated"
              >
                <MessageSquare size={13} className="shrink-0 text-fg3" />
                <span className="min-w-0 flex-1 truncate">{s.title}</span>
                <span className="text-[11px] text-fg3">{fmtTime(s.updated_at)}</span>
              </button>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
