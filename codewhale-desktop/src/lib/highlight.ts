import type { HighlighterCore } from "shiki/core";

// Fine-grained Shiki: only the core, the JS regex engine (no WASM, CSP-friendly),
// two themes and languages loaded on demand.
const LANGS: Record<string, () => Promise<any>> = {
  rust: () => import("shiki/langs/rust.mjs"),
  typescript: () => import("shiki/langs/typescript.mjs"),
  tsx: () => import("shiki/langs/tsx.mjs"),
  javascript: () => import("shiki/langs/javascript.mjs"),
  jsx: () => import("shiki/langs/jsx.mjs"),
  json: () => import("shiki/langs/json.mjs"),
  toml: () => import("shiki/langs/toml.mjs"),
  yaml: () => import("shiki/langs/yaml.mjs"),
  python: () => import("shiki/langs/python.mjs"),
  powershell: () => import("shiki/langs/powershell.mjs"),
  bash: () => import("shiki/langs/bash.mjs"),
  html: () => import("shiki/langs/html.mjs"),
  css: () => import("shiki/langs/css.mjs"),
  markdown: () => import("shiki/langs/markdown.mjs"),
  diff: () => import("shiki/langs/diff.mjs"),
  csharp: () => import("shiki/langs/csharp.mjs"),
  cpp: () => import("shiki/langs/cpp.mjs"),
  c: () => import("shiki/langs/c.mjs"),
  go: () => import("shiki/langs/go.mjs"),
  java: () => import("shiki/langs/java.mjs"),
  sql: () => import("shiki/langs/sql.mjs"),
  gdscript: () => import("shiki/langs/gdscript.mjs"),
  glsl: () => import("shiki/langs/glsl.mjs"),
  lua: () => import("shiki/langs/lua.mjs"),
  xml: () => import("shiki/langs/xml.mjs"),
};

const ALIASES: Record<string, string> = {
  rs: "rust",
  ts: "typescript",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  py: "python",
  ps1: "powershell",
  ps: "powershell",
  pwsh: "powershell",
  sh: "bash",
  shell: "bash",
  zsh: "bash",
  console: "bash",
  yml: "yaml",
  md: "markdown",
  cs: "csharp",
  "c#": "csharp",
  "c++": "cpp",
  hpp: "cpp",
  h: "c",
  gd: "gdscript",
  gdshader: "glsl",
  shader: "glsl",
  hlsl: "glsl",
  jsonc: "json",
  htm: "html",
  svg: "xml",
  patch: "diff",
};

export function normalizeLang(lang?: string | null): string | null {
  if (!lang) return null;
  const l = lang.toLowerCase().trim();
  const name = ALIASES[l] ?? l;
  return LANGS[name] ? name : null;
}

let hlPromise: Promise<HighlighterCore> | null = null;
const loaded = new Set<string>();

async function highlighter(): Promise<HighlighterCore> {
  if (!hlPromise) {
    hlPromise = (async () => {
      const [{ createHighlighterCore }, { createJavaScriptRegexEngine }, dark, light] = await Promise.all([
        import("shiki/core"),
        import("shiki/engine/javascript"),
        import("shiki/themes/github-dark-default.mjs"),
        import("shiki/themes/github-light-default.mjs"),
      ]);
      return createHighlighterCore({
        themes: [dark.default, light.default],
        langs: [],
        engine: createJavaScriptRegexEngine({ forgiving: true }),
      });
    })();
  }
  return hlPromise;
}

const cache = new Map<string, string>();

export async function highlight(code: string, lang?: string | null): Promise<string | null> {
  const name = normalizeLang(lang);
  if (!name) return null;
  const key = name + "\u0000" + code;
  const hit = cache.get(key);
  if (hit) return hit;
  const hl = await highlighter();
  if (!loaded.has(name)) {
    const mod = await LANGS[name]();
    await hl.loadLanguage(mod.default);
    loaded.add(name);
  }
  const html = hl.codeToHtml(code, {
    lang: name,
    themes: { dark: "github-dark-default", light: "github-light-default" },
    defaultColor: false,
  });
  if (cache.size > 300) cache.clear();
  cache.set(key, html);
  return html;
}
