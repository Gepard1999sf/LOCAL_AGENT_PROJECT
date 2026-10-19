import { lazy, memo, Suspense, type ReactNode } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { CodeBlock } from "./CodeBlock";
import { api } from "../../lib/tauri";
import { resolvePath } from "../../lib/paths";

// The diagram component and mermaid itself are separate chunks, loaded with the first diagram.
const MermaidDiagram = lazy(() => import("./MermaidDiagram").then((m) => ({ default: m.MermaidDiagram })));

// Unicode letters: Cyrillic user and folder names are common on Russian Windows.
const FILE_REF = /^([A-Za-z]:[\\/])?[\p{L}\p{M}\p{N}_.\-\\/]+\.[A-Za-z0-9]{1,8}(:\d+)?$/u;

function InlineCode({ children }: { children?: ReactNode }) {
  const text = String(children ?? "");
  if (FILE_REF.test(text) && text.length < 200) {
    const [, , line] = /^(.*?)(?::(\d+))?$/.exec(text) ?? [];
    const path = text.replace(/:\d+$/, "");
    return (
      <code
        className="cursor-pointer text-bluefg hover:underline"
        title="Открыть в редакторе"
        onClick={() =>
          api
            .openInEditor(resolvePath(path), line ? Number(line) : undefined)
            .catch((e) => toast.error("Не удалось открыть файл", { description: String(e) }))
        }
      >
        {text}
      </code>
    );
  }
  return <code>{children}</code>;
}

function makeComponents(streaming: boolean): Components {
  return {
    code({ className, children }) {
      const m = /language-([\w+#-]+)/.exec(className || "");
      const text = String(children ?? "");
      // Fenced blocks arrive with a trailing newline and/or a language class.
      if (m || text.includes("\n")) {
        const code = text.replace(/\n$/, "");
        // A diagram is drawn once the answer is complete; while it streams it is plain source.
        if (!streaming && m?.[1].toLowerCase() === "mermaid" && code.trim()) {
          return (
            <Suspense fallback={<CodeBlock code={code} lang={m[1]} />}>
              <MermaidDiagram code={code} />
            </Suspense>
          );
        }
        return <CodeBlock code={code} lang={m?.[1]} streaming={streaming} />;
      }
      return <InlineCode>{children}</InlineCode>;
    },
    pre({ children }) {
      return <>{children}</>;
    },
    a({ href, children }) {
      return (
        <a
          href={href}
          onClick={(e) => {
            e.preventDefault();
            if (href && /^(https?:\/\/|mailto:)/i.test(href)) {
              openUrl(href).catch((err) => toast.error("Не удалось открыть ссылку", { description: String(err) }));
            }
          }}
        >
          {children}
        </a>
      );
    },
  };
}

const staticComponents = makeComponents(false);
const streamingComponents = makeComponents(true);

export const Markdown = memo(function Markdown({ text, streaming = false }: { text: string; streaming?: boolean }) {
  return (
    <div className={`md ${streaming ? "stream-cursor" : ""}`}>
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={streaming ? streamingComponents : staticComponents}>
        {text}
      </ReactMarkdown>
    </div>
  );
});
