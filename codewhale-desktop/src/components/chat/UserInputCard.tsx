import { memo, useState } from "react";
import { AlertTriangle, Check, MessageCircleQuestion, MessageSquareOff, Send } from "lucide-react";
import type { UserInputRequest } from "../../store/threads";
import type { UserInputAnswer, UserInputQuestion } from "../../lib/types";
import { useApp } from "../../store/app";
import { Button } from "../ui/primitives";

/** Selection for one question; `other` marks the free-text row as chosen (single-select). */
interface Pick {
  options: number[];
  other: boolean;
  text: string;
}

const EMPTY_PICK: Pick = { options: [], other: false, text: "" };

// Same answer shape the TUI sends: the option label as both label and value,
// free text under the label "Other".
function answersFor(q: UserInputQuestion, pick: Pick): UserInputAnswer[] {
  const out: UserInputAnswer[] = pick.options
    .filter((i) => q.options[i])
    .map((i) => ({ id: q.id, label: q.options[i].label, value: q.options[i].label }));
  const text = pick.text.trim();
  if (text && (q.multi_select || pick.other)) out.push({ id: q.id, label: "Other", value: text });
  return out;
}

function Question({ q, pick, onChange, disabled }: { q: UserInputQuestion; pick: Pick; onChange: (p: Pick) => void; disabled: boolean }) {
  const multi = !!q.multi_select;
  const toggle = (i: number) => {
    if (multi) {
      onChange({ ...pick, options: pick.options.includes(i) ? pick.options.filter((x) => x !== i) : [...pick.options, i] });
    } else {
      onChange({ ...pick, options: [i], other: false });
    }
  };
  const row = (selected: boolean) =>
    `flex w-full items-start gap-2 rounded-[var(--radius-sm)] border px-2.5 py-1.5 text-left text-[12.5px] transition-colors disabled:opacity-60 ${
      selected ? "border-blue bg-[color-mix(in_srgb,var(--accent-blue)_10%,transparent)]" : "border-line-muted bg-base hover:bg-hover"
    }`;
  const mark = (selected: boolean) => (
    <span
      className={`mt-0.5 inline-flex h-3.5 w-3.5 shrink-0 items-center justify-center border ${multi ? "rounded-[3px]" : "rounded-full"} ${
        selected ? "border-blue bg-blue text-white" : "border-line-strong"
      }`}
    >
      {selected && <Check size={10} />}
    </span>
  );

  return (
    <div className="mx-3 my-2">
      <div className="mb-1 flex items-center gap-2 text-[12px]">
        <span className="rounded-full border border-line px-2 py-0.5 text-[11px] text-fg2">{q.header}</span>
        {multi && <span className="text-[11px] text-fg3">можно выбрать несколько</span>}
      </div>
      <div className="mb-1.5 whitespace-pre-wrap break-words text-[13px] text-fg">{q.question}</div>
      <div className="space-y-1">
        {q.options.map((o, i) => {
          const selected = pick.options.includes(i);
          return (
            <button key={i} type="button" disabled={disabled} className={row(selected)} onClick={() => toggle(i)}>
              {mark(selected)}
              <span className="min-w-0">
                <span className="block font-medium text-fg">{o.label}</span>
                {o.description && <span className="block text-[11.5px] text-fg3">{o.description}</span>}
              </span>
            </button>
          );
        })}
        {/* Always offered, as in the TUI since v0.9.4: `allow_free_text` no longer gates it. */}
        <input
          value={pick.text}
          disabled={disabled}
          placeholder={multi ? "Свой вариант (необязательно)…" : "Свой ответ…"}
          onFocus={() => !multi && onChange({ ...pick, options: [], other: true })}
          onChange={(e) => onChange({ ...pick, text: e.target.value, ...(multi ? {} : { options: [], other: true }) })}
          className={`w-full rounded-[var(--radius-sm)] border bg-base px-2.5 py-1.5 text-[12.5px] text-fg outline-none placeholder:text-fg3 disabled:opacity-60 ${
            !multi && pick.other ? "border-blue" : "border-line-muted focus:border-blue"
          }`}
        />
      </div>
    </div>
  );
}

/** Card for a `request_user_input` call: the agent waits until it is answered. */
export const UserInputCard = memo(function UserInputCard({ threadId, request }: { threadId: string; request: UserInputRequest }) {
  const answer = useApp((s) => s.answerUserInput);
  const [picks, setPicks] = useState<Record<string, Pick>>({});
  const [sending, setSending] = useState(false);
  const questions = request.questions;

  if (request.status !== "pending") {
    const answered = request.status === "answered";
    const I = answered ? Check : MessageSquareOff;
    const given = request.answers?.map((a) => a.value).join(", ");
    return (
      <div className="my-1.5 flex items-center gap-2 text-[11.5px] text-fg3">
        <I size={13} className="shrink-0" /> {answered ? "Ответ отправлен" : "Вопрос отменён"}
        {questions[0] && <span className="truncate">: {questions.map((q) => q.header).join(", ")}</span>}
        {given && <span className="truncate text-fg2">→ {given}</span>}
      </div>
    );
  }

  const pickOf = (q: UserInputQuestion) => picks[q.id] ?? EMPTY_PICK;
  // Multi-select may be confirmed empty (as in the TUI); single-select needs a choice.
  const ready = questions.length > 0 && questions.every((q) => q.multi_select || answersFor(q, pickOf(q)).length > 0);

  const submit = async () => {
    if (!ready || sending) return;
    setSending(true);
    await answer(threadId, request.id, questions.flatMap((q) => answersFor(q, pickOf(q))));
    setSending(false);
  };

  return (
    <div
      className="my-2 overflow-hidden rounded-[var(--radius-md)] border bg-elevated"
      style={{ borderColor: "color-mix(in srgb, var(--accent-blue) 55%, transparent)" }}
    >
      <div className="flex items-center gap-2 px-3 pt-2.5 text-[13px] font-medium">
        <MessageCircleQuestion size={15} className="text-bluefg" />
        Агент задаёт {questions.length > 1 ? "вопросы" : "вопрос"}
        <span className="font-normal text-fg3">· работа продолжится после ответа</span>
      </div>
      {request.deliveryError && (
        <div className="mx-3 mt-2 flex items-center gap-1.5 text-[12px] text-yellow">
          <AlertTriangle size={13} /> Ответ не принят агентом. Попробуйте ещё раз.
        </div>
      )}
      {questions.length ? (
        questions.map((q) => (
          <Question key={q.id} q={q} pick={pickOf(q)} disabled={sending} onChange={(p) => setPicks((s) => ({ ...s, [q.id]: p }))} />
        ))
      ) : (
        <pre className="mx-3 my-2 max-h-56 overflow-auto whitespace-pre-wrap break-words font-mono text-[11.5px] text-fg3">
          {JSON.stringify(request.payload, null, 2)}
        </pre>
      )}
      <div className="flex flex-wrap gap-2 px-3 pb-3 pt-1">
        <Button variant="primary" disabled={!ready || sending} onClick={submit}>
          <Send size={13} /> Ответить
        </Button>
      </div>
    </div>
  );
});
