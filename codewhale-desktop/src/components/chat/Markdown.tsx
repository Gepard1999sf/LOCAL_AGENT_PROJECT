import { memo, type ReactNode } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { CodeBlock } from "./CodeBlock";
import { api } from "../../lib/tauri";
import { resolvePath } from "../../lib/paths";

const FILE_REF = /^([A-Za-z]:[\\/])?[\w.\-\\/]+\.[A-Za-z0-9]{1,8}(:\d+)?$/;

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
        return <CodeBlock code={text.replace(/\n$/, "")} lang={m?.[1]} streaming={streaming} />;
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
