#!/usr/bin/env python3
"""build-godot-docset.py - builds a Dash/Zeal docset of the Godot 4 class reference from a
godot-docs checkout (classes/class_*.rst). Standard library only, Python 3.11+.

    py -3 -I -X utf8 build-godot-docset.py --docs F:\\AI_Drive\\git_vault\\gamedev\\godot-docs --out F:\\AI_Drive\\docsets

Writes <out>/<name>.docset (default name: Godot), replacing an existing one only after the new
build succeeded:

    Contents/Info.plist
    Contents/Resources/docSet.dsidx          SQLite searchIndex(id, name, type, path)
    Contents/Resources/Documents/
        index.html                           list of classes
        style.css
        class_<x>.html                       full class page; every member has an id anchor
        class_<x>/<member>.html              one short page per member (method, property, signal,
                                             constant, enum, theme item, operator, constructor,
                                             annotation)

Search rows for members point at the short per-member pages, so a reader that ignores the
'#anchor' part of a path (the ai-drive docset_read tool does) still gets just that member instead
of the whole class page. Each member page links back to its anchor on the class page.

Index names: classes "Node"; members "Node.add_child", "Node.ready", "Node.ProcessMode",
"Node.PROCESS_MODE_INHERIT"; @GlobalScope/@GDScript functions, constants and enums are global in
GDScript and are indexed bare ("lerp", "KEY_A", "Error", "@export_range").
"""
from __future__ import annotations

import argparse
import hashlib
import html
import json
import os
import posixpath
import re
import shutil
import sqlite3
import subprocess
import sys
import textwrap
import time
import unicodedata
from dataclasses import dataclass, field
from pathlib import Path
from urllib.parse import quote

DOCS_URL = "https://docs.godotengine.org/en/latest/"
GLOBAL_CLASSES = {"@GlobalScope", "@GDScript"}
ITEM_KINDS = {
    "method",
    "property",
    "signal",
    "enumeration",
    "enumeration-constant",
    "constant",
    "themeproperty",
    "operator",
    "constructor",
    "annotation",
}
# Index type -> word used on the member page ("Method of Node").
TYPE_WORDS = {"Style": "Theme property"}
ADMONITIONS = {
    "note": "Note",
    "warning": "Warning",
    "tip": "Tip",
    "hint": "Hint",
    "important": "Important",
    "caution": "Caution",
    "danger": "Danger",
    "attention": "Attention",
    "error": "Error",
    "seealso": "See also",
}
LANG_NAMES = {
    "gdscript": "GDScript",
    "csharp": "C#",
    "cpp": "C++",
    "c++": "C++",
    "glsl": "GLSL",
    "python": "Python",
    "text": "Text",
    "shell": "Shell",
    "bash": "Shell",
    "json": "JSON",
    "ini": "INI",
}
# Used when a class file lacks the substitution definitions at its end.
DEFAULT_SUBS = {
    "virtual": ":abbr:`virtual (This method should typically be overridden by the user to have any effect.)`",
    "required": ":abbr:`required (This method is required to be overridden when extending its base class.)`",
    "const": ":abbr:`const (This method has no side effects. It doesn't modify any of the instance's member variables.)`",
    "vararg": ":abbr:`vararg (This method accepts any number of arguments after the ones described here.)`",
    "constructor": ":abbr:`constructor (This method is used to construct a type.)`",
    "static": ":abbr:`static (This method doesn't need an instance to be called, so it can be called directly using the class name.)`",
    "operator": ":abbr:`operator (This method describes a valid operator to use with this type as left-hand operand.)`",
    "bitfield": ":abbr:`BitField (This value is an integer composed as a bitmask of the following flags.)`",
    "void": ":abbr:`void (No return value.)`",
}

LABEL_RE = re.compile(r"^\.\. _([^:`]+):\s*$")
SUBST_DEF_RE = re.compile(r"^\.\. \|([^|]+)\| replace:: (.*)$")
DIRECTIVE_RE = re.compile(r"^\.\. ([A-Za-z][\w-]*)::\s*(.*)$")
RSTCLASS_RE = re.compile(r"^\.\. rst-class:: (\S+)\s*$")
OPTION_RE = re.compile(r"^\s*:[\w-]+:")
BULLET_RE = re.compile(r"^[-*+]\s+")
ENUM_LIST_RE = re.compile(r"^(?:\d+|#)[.)]\s+")
UNDERLINE_RE = re.compile(r"^([=\-~^\"'`#*+:.])\1{2,}$")
BOLD_RE = re.compile(r"\*\*(.+?)\*\*(?!\*)")
EM_RE = re.compile(r"(?<![\w*])\*(?=\S)([^*]+?)(?<=\S)\*(?![\w*])")
INLINE_RE = re.compile(
    r"``(?P<lit>.+?)``(?!`)"
    r"|:(?P<role>[a-z]+):`(?P<rbody>(?:[^`\\]|\\.)*)`"
    r"|`(?P<ltext>[^`<]*?)\s*<(?P<url>[^<>`]+)>`__?"
    r"|\|(?P<sub>[A-Za-z0-9_-]+)\|"
)
TARGET_RE = re.compile(r"^(.*?)\s*<([^<>]+)>$", re.S)
ABBR_RE = re.compile(r"^(.*?)\s*\((.*)\)\s*$", re.S)
PERMALINK_RE = re.compile(r"\s*:ref:`🔗<[^`>]*>`\s*$")
SAFE_RE = re.compile(r"[^A-Za-z0-9_.@-]+")
HREF_PREFIX = "\x01"  # "" on class pages, "../" on member pages

CSS = """:root{color-scheme:light dark;--fg:#1f2328;--bg:#ffffff;--muted:#59636e;--code:#f3f5f7;--link:#0969da;--border:#d1d9e0;--note:#eef6ff}
@media (prefers-color-scheme:dark){:root{--fg:#e6edf3;--bg:#0d1117;--muted:#9198a1;--code:#161b22;--link:#4493f8;--border:#3d444d;--note:#10213a}}
body{font:15px/1.55 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif;color:var(--fg);background:var(--bg);max-width:980px;margin:0 auto;padding:16px}
a{color:var(--link);text-decoration:none}a:hover{text-decoration:underline}
h1{font-size:1.7em;margin:.2em 0 .4em}h2{font-size:1.3em;border-bottom:1px solid var(--border);padding-bottom:.2em;margin-top:1.6em}
code,kbd,pre{font-family:ui-monospace,"Cascadia Code",Consolas,monospace;font-size:.92em}
code{background:var(--code);padding:.1em .3em;border-radius:4px}
pre{background:var(--code);padding:10px 12px;border-radius:6px;overflow-x:auto;line-height:1.4}pre code{background:none;padding:0}
kbd{border:1px solid var(--border);border-radius:4px;padding:0 .3em}
abbr{text-decoration:none;color:var(--muted)}
table.ref{border-collapse:collapse;width:100%;margin:.5em 0}table.ref td,table.ref th{border:1px solid var(--border);padding:3px 8px;vertical-align:top;text-align:left}
.item{border-top:1px solid var(--border);padding:.6em 0}.sig{font-family:ui-monospace,"Cascadia Code",Consolas,monospace;font-size:.95em;font-weight:500}
.kind{color:var(--muted);margin-top:0}.tab-label{font-size:.8em;color:var(--muted);margin:.6em 0 .1em}
.admonition{background:var(--note);border-radius:6px;padding:.2em .9em;margin:.6em 0}.admonition-title{font-weight:600}
.contribute{color:var(--muted);font-style:italic}ul.setget{color:var(--muted);font-family:ui-monospace,Consolas,monospace;font-size:.9em}
dl.constants dt{font-family:ui-monospace,Consolas,monospace;margin-top:.6em}dl.constants dd{margin-left:1.4em}
ul.classes{columns:2;padding-left:1.2em}@media (max-width:640px){ul.classes{columns:1}}
"""

PLIST = """<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleIdentifier</key>
	<string>{ident}</string>
	<key>CFBundleName</key>
	<string>{title}</string>
	<key>DocSetPlatformFamily</key>
	<string>{ident}</string>
	<key>DashDocSetFamily</key>
	<string>dashtoc</string>
	<key>dashIndexFilePath</key>
	<string>index.html</string>
	<key>isDashDocset</key>
	<true/>
	<key>isJavaScriptEnabled</key>
	<false/>
</dict>
</plist>
"""


def esc(s: str) -> str:
    return html.escape(s, quote=False)


def attr(s: str) -> str:
    return html.escape(s, quote=True)


def make_id(label: str) -> str:
    """docutils' make_id: the id Sphinx gives a `.. _label:` target."""
    s = unicodedata.normalize("NFKD", label).encode("ascii", "ignore").decode("ascii").lower()
    s = re.sub(r"[^a-z0-9]+", "-", s)
    return re.sub(r"^[-0-9]+|-+$", "", s)


def indent_of(line: str) -> int:
    return len(line) - len(line.lstrip(" \t"))


def dedent_lines(lines: list[str]) -> list[str]:
    return textwrap.dedent("\n".join(lines)).split("\n") if lines else []


def strip_options(lines: list[str]) -> list[str]:
    i = 0
    while i < len(lines) and OPTION_RE.match(lines[i]):
        i += 1
    return lines[i:]


def take_block(lines: list[str], i: int, ind: int) -> tuple[list[str], int]:
    """Lines from i on that are blank or indented deeper than `ind` (a directive's body)."""
    j = i
    while j < len(lines) and (not lines[j].strip() or indent_of(lines[j]) > ind):
        j += 1
    block = lines[i:j]
    while block and not block[-1].strip():
        block.pop()
    return block, j


def unescape(s: str) -> str:
    return re.sub(r"\\(.)", r"\1", s.replace("\\ ", ""))


# ---------------------------------------------------------------------------------------------
# Inline markup
# ---------------------------------------------------------------------------------------------


class Ctx:
    def __init__(self) -> None:
        # lower-cased label -> (page stem, anchor or None for the page itself)
        self.labels: dict[str, tuple[str, str | None]] = {}
        # lower-cased label of the narrative docs -> (absolute url, title)
        self.docs: dict[str, tuple[str, str]] = {}
        self.subs: dict[str, str] = dict(DEFAULT_SUBS)

    def resolve_ref(self, target: str) -> tuple[str | None, str | None]:
        key = target.strip().lower()
        if key in self.labels:
            stem, anchor = self.labels[key]
            return f"{HREF_PREFIX}{stem}.html" + (f"#{anchor}" if anchor else ""), None
        if key in self.docs:
            return self.docs[key]
        return None, None


def plain(s: str, as_html: bool) -> str:
    if not s:
        return ""
    s = s.replace("\\ ", "").replace("\\*", "\x00")
    s = re.sub(r"\\(.)", r"\1", s)
    if as_html:
        s = esc(s)
        s = BOLD_RE.sub(r"<strong>\1</strong>", s)
        s = EM_RE.sub(r"<em>\1</em>", s)
    else:
        s = BOLD_RE.sub(r"\1", s)
        s = EM_RE.sub(r"\1", s)
    return s.replace("\x00", "*")


def doc_url(target: str) -> str:
    target = target.strip()
    if target.startswith("/"):
        path = target.lstrip("/")
    else:
        path = posixpath.normpath(posixpath.join("classes", target))
    return DOCS_URL + path + ".html"


def role(ctx: Ctx, name: str, body: str, as_html: bool) -> str:
    if name in ("ref", "doc"):
        m = TARGET_RE.match(body)
        text, target = (m[1], m[2]) if m else (None, body)
        if text is not None and text.strip() == "🔗":
            return ""  # Sphinx permalink marker
        if name == "ref":
            href, title = ctx.resolve_ref(target)
        else:
            href, title = doc_url(target), None
        disp = unescape(text).strip() if text else (title or target)
        if not as_html:
            return disp
        return f'<a href="{attr(href)}">{esc(disp)}</a>' if href else esc(disp)
    if name == "abbr":
        m = ABBR_RE.match(body)
        if m:
            if not as_html:
                return unescape(m[1])
            return f'<abbr title="{attr(unescape(m[2]))}">{esc(unescape(m[1]))}</abbr>'
        return plain(body, as_html)
    if name == "kbd":
        return f"<kbd>{esc(unescape(body))}</kbd>" if as_html else unescape(body)
    if name in ("code", "math", "samp", "file", "command"):
        return f"<code>{esc(unescape(body))}</code>" if as_html else unescape(body)
    return plain(body, as_html)


def inline(ctx: Ctx, text: str, as_html: bool = True) -> str:
    out: list[str] = []
    pos = 0
    for m in INLINE_RE.finditer(text):
        out.append(plain(text[pos : m.start()], as_html))
        if m["lit"] is not None:
            out.append(f"<code>{esc(m['lit'])}</code>" if as_html else m["lit"])
        elif m["role"] is not None:
            out.append(role(ctx, m["role"], m["rbody"], as_html))
        elif m["url"] is not None:
            url = m["url"].strip()
            label = unescape(m["ltext"]).strip() or url
            if not re.match(r"^[A-Za-z][A-Za-z0-9+.-]*:", url) and not url.startswith("#"):
                url = DOCS_URL + posixpath.normpath(posixpath.join("classes", url))  # relative to classes/
            out.append(f'<a href="{attr(url)}">{esc(label)}</a>' if as_html else label)
        elif m["sub"] in ctx.subs:
            out.append(inline(ctx, ctx.subs[m["sub"]], as_html))
        else:
            out.append(plain(m[0], as_html))
        pos = m.end()
    out.append(plain(text[pos:], as_html))
    return "".join(out)


# ---------------------------------------------------------------------------------------------
# Block markup
# ---------------------------------------------------------------------------------------------


def render_code(lines: list[str], lang: str) -> str:
    text = "\n".join(strip_options(lines)).strip("\n")
    lang = lang.strip().split()[0].lower() if lang.strip() else ""
    return f'<pre class="code" data-lang="{attr(lang)}"><code>{esc(text)}</code></pre>'


def render_tabs(ctx: Ctx, body: list[str]) -> str:
    out = []
    i = 0
    while i < len(body):
        m = DIRECTIVE_RE.match(body[i].strip())
        if m and m[1] in ("code-tab", "tab", "group-tab"):
            block, i = take_block(body, i + 1, indent_of(body[i]))
            words = m[2].split()
            lang = words[0] if words else ""
            label = " ".join(words[1:]) if len(words) > 1 else LANG_NAMES.get(lang.lower(), lang or "Code")
            if m[1] == "code-tab":
                out.append(f'<p class="tab-label">{esc(label)}</p>{render_code(dedent_lines(block), lang)}')
            else:
                out.append(f'<p class="tab-label">{esc(label)}</p>{render_blocks(ctx, dedent_lines(block))}')
            continue
        i += 1
    return "\n".join(out)


def render_grid(ctx: Ctx, rows: list[str]) -> str:
    """Grid table: column borders are the '+' positions of the first border line."""
    border = rows[0]
    cols = [k for k, ch in enumerate(border) if ch == "+"]
    parsed: list[tuple[list[str], bool]] = []
    cur: list[str] | None = None
    header_until = 0
    for line in rows:
        if line.startswith("+"):
            if cur is not None:
                parsed.append((cur, False))
                cur = None
            if "=" in line:
                header_until = len(parsed)
            continue
        if len(line) == len(border) and len(cols) > 1:
            cells = [line[a + 1 : b].strip() for a, b in zip(cols, cols[1:])]
        else:
            cells = [c.strip() for c in line.strip("|").split(" | ")]
        if cur is None:
            cur = cells
        else:
            cur = [f"{a} {b}".strip() for a, b in zip(cur, cells)]
    if cur is not None:
        parsed.append((cur, False))
    out = ['<table class="ref">']
    for k, (cells, _) in enumerate(parsed):
        tag = "th" if k < header_until else "td"
        out.append("<tr>" + "".join(f"<{tag}>{inline(ctx, c)}</{tag}>" for c in cells) + "</tr>")
    out.append("</table>")
    # No newlines between rows: text extractors that end a line at </tr> would add blank lines.
    return "".join(out)


def take_list(lines: list[str], i: int, ind: int, rx: re.Pattern[str]) -> tuple[list[list[str]], int]:
    items: list[list[str]] = []
    n = len(lines)
    while i < n:
        s = lines[i].strip()
        if not s:
            j = i
            while j < n and not lines[j].strip():
                j += 1
            if j < n and indent_of(lines[j]) == ind and rx.match(lines[j].strip()):
                i = j
                continue
            break
        m = rx.match(s)
        if indent_of(lines[i]) != ind or not m:
            break
        content = [s[m.end() :]]
        i += 1
        while i < n:
            if lines[i].strip() and indent_of(lines[i]) > ind:
                content.append(lines[i])
                i += 1
            elif not lines[i].strip():
                j = i
                while j < n and not lines[j].strip():
                    j += 1
                if j < n and indent_of(lines[j]) > ind:
                    content.extend(lines[i:j])
                    i = j
                else:
                    break
            else:
                break
        items.append(content)
    return items, i


def render_list_item(ctx: Ctx, content: list[str]) -> str:
    body = render_blocks(ctx, [content[0]] + dedent_lines(content[1:]))
    if body.startswith("<p>") and body.endswith("</p>") and body.count("<p>") == 1:
        body = body[3:-4]
    return body


def render_blocks(ctx: Ctx, lines: list[str]) -> str:
    out: list[str] = []
    i, n = 0, len(lines)
    while i < n:
        line = lines[i]
        s = line.strip()
        if not s:
            i += 1
            continue
        ind = indent_of(line)
        if s.startswith(".. "):
            block, i = take_block(lines, i + 1, ind)
            m = DIRECTIVE_RE.match(s)
            if not m:
                continue  # comment, label or substitution definition
            name, arg = m[1], m[2].strip()
            body = dedent_lines(block)
            if name == "tabs":
                out.append(render_tabs(ctx, body))
            elif name in ("code", "code-block", "sourcecode", "code-tab"):
                out.append(render_code(body, arg))
            elif name in ADMONITIONS:
                inner = render_blocks(ctx, ([arg] if arg else []) + body)
                out.append(
                    f'<div class="admonition"><p class="admonition-title">{ADMONITIONS[name]}</p>{inner}</div>'
                )
            elif name == "container":
                out.append(f'<div class="{attr(arg or "container")}">{render_blocks(ctx, body)}</div>')
            elif name == "table":
                out.append(render_blocks(ctx, strip_options(body)))
            continue  # rst-class, meta, image, only, ...: dropped
        if s.startswith("+") and s.endswith("+") and set(s) <= set("+-=:"):
            j = i
            while j < n and lines[j].strip()[:1] in ("+", "|"):
                j += 1
            out.append(render_grid(ctx, [x.strip() for x in lines[i:j]]))
            i = j
            continue
        if len(s) >= 4 and set(s) == {"-"}:
            i += 1  # transition
            continue
        if s == "::":  # literal block ([codeblock] in the engine XML; highlight_language is gdscript)
            block, i = take_block(lines, i + 1, ind)
            out.append(render_code(dedent_lines(block), "gdscript"))
            continue
        rx = BULLET_RE if BULLET_RE.match(s) else ENUM_LIST_RE if ENUM_LIST_RE.match(s) else None
        if rx is not None:
            items, i = take_list(lines, i, ind, rx)
            tag = "ul" if rx is BULLET_RE else "ol"
            out.append(f"<{tag}>" + "".join(f"<li>{render_list_item(ctx, it)}</li>" for it in items) + f"</{tag}>")
            continue
        para = [s]
        i += 1
        while i < n and lines[i].strip() and not lines[i].strip().startswith(".. "):
            para.append(lines[i].strip())
            i += 1
        literal = para[-1].endswith("::")
        if literal:  # "text::" = "text:" followed by a literal block
            para[-1] = para[-1][:-2].rstrip() + (":" if not para[-1][:-2].endswith(" ") else "")
        out.append(f"<p>{inline(ctx, ' '.join(para))}</p>")
        if literal:
            block, i = take_block(lines, i, ind)
            out.append(render_code(dedent_lines(block), "gdscript"))
    return "\n".join(out)


# ---------------------------------------------------------------------------------------------
# Class files
# ---------------------------------------------------------------------------------------------


@dataclass
class Item:
    kind: str
    label: str | None
    sig: str
    name: str = ""
    anchor: str = ""
    sig_html: str = ""
    sig_text: str = ""
    setget: list[str] = field(default_factory=list)
    setget_html: str = ""
    body: list[str] = field(default_factory=list)
    body_html: str = ""
    enum: Item | None = None
    constants: list[Item] = field(default_factory=list)
    index_type: str = ""
    index_name: str = ""
    page: str = ""


@dataclass
class Section:
    title: str
    html: str
    items: list[Item]


@dataclass
class ClassDoc:
    name: str
    stem: str
    anchor: str
    preamble_html: str
    brief: str
    sections: list[Section]


def is_heading(lines: list[str], i: int) -> bool:
    s, u = lines[i], lines[i + 1]
    t = s.strip()
    return bool(
        t
        and not s[0].isspace()
        and not t.startswith(("..", "+", "|"))
        and len(u) >= max(3, len(t))
        and set(u) == {"-"}
    )


def parse_items(lines: list[str]) -> tuple[list[Item], list[str]]:
    """Splits a section into member items (marked by `.. rst-class:: classref-<kind>`) and the
    remaining lines (descriptions, summary tables)."""
    items: list[Item] = []
    other: list[str] = []
    cur: Item | None = None
    enum: Item | None = None
    label: str | None = None
    i, n = 0, len(lines)
    while i < n:
        line = lines[i]
        m = LABEL_RE.match(line)
        if m:
            cur, label = None, m[1]
            i += 1
            continue
        m = RSTCLASS_RE.match(line)
        if m:
            kind = m[1][len("classref-") :] if m[1].startswith("classref-") else ""
            if kind in ITEM_KINDS:
                j = i + 1
                while j < n and not lines[j].strip():
                    j += 1
                cur = Item(kind, label, lines[j].strip() if j < n else "")
                label = None
                if kind == "enumeration":
                    enum = cur
                elif kind == "enumeration-constant":
                    cur.enum = enum
                    if enum is not None:
                        enum.constants.append(cur)
                else:
                    enum = None
                items.append(cur)
                i = j + 1
                continue
            if kind == "property-setget" and cur is not None:
                j = i + 1
                while j < n and not lines[j].strip():
                    j += 1
                while j < n and indent_of(lines[j]) == 0 and BULLET_RE.match(lines[j]):
                    cur.setget.append(BULLET_RE.sub("", lines[j].strip(), count=1))
                    j += 1
                i = j
                continue
            cur = None  # item/section separators, group markers
            if kind == "section-separator":
                enum = None
            i += 1
            continue
        if cur is not None:
            cur.body.append(line)
        else:
            other.append(line)
        i += 1
    return items, other


def index_entry(cls: str, it: Item) -> tuple[str, str]:
    glob = cls in GLOBAL_CLASSES
    k, nm = it.kind, it.name
    q = f"{cls}.{nm}"
    if k == "method":
        return ("Function", nm) if glob else ("Method", q)
    if k == "property":
        return "Property", q
    if k == "signal":
        return "Signal", q
    if k == "enumeration":
        return "Enum", nm if glob else q
    if k in ("enumeration-constant", "constant"):
        return "Constant", nm if glob else q
    if k == "themeproperty":
        return "Style", q
    if k == "annotation":
        return "Annotation", nm
    if k == "operator":
        m = re.search(r"\(\s*right:\s*(.*?)\s*\)\s*$", it.sig_text)
        return "Operator", f"{q}({m[1] if m else ''})"
    if k == "constructor":
        p = it.sig_text.find(f"{nm}(")
        return "Constructor", it.sig_text[p:] if p >= 0 else f"{nm}()"
    return "Entry", q


def member_file(cls: str, it: Item, used: set[str]) -> str:
    lab = it.label or ""
    if lab.startswith(f"class_{cls}_"):
        base = lab[len(f"class_{cls}_") :]
    elif lab.startswith(f"enum_{cls}_"):
        base = "enum_" + lab[len(f"enum_{cls}_") :]
    else:
        base = f"{it.kind}_{it.name}"
    base = SAFE_RE.sub("_", base).strip("._ ") or it.kind
    if len(base) > 80:  # keep full paths under Windows MAX_PATH
        base = base[:70] + "_" + hashlib.sha1(base.encode("utf-8")).hexdigest()[:8]
    name, k = base, 2
    while name.lower() in used:
        name, k = f"{base}_{k}", k + 1
    used.add(name.lower())
    return name


def first_paragraph(lines: list[str]) -> list[str]:
    para: list[str] = []
    for line in lines:
        s = line.strip()
        if not s:
            if para:
                if not para[0].startswith("**"):
                    return para
                para = []
            continue
        if s.startswith(".. ") or (not para and indent_of(line) > 0):
            if para and not para[0].startswith("**"):
                return para
            para = []
            continue
        para.append(s)
    return para if para and not para[0].startswith("**") else []


def parse_class(ctx: Ctx, path: Path) -> ClassDoc:
    lines: list[str] = []
    for line in path.read_text(encoding="utf-8-sig").splitlines():
        m = SUBST_DEF_RE.match(line)
        if m:
            ctx.subs[m[1]] = m[2].strip()
            continue
        stripped = line.rstrip()
        if stripped.endswith("\\") and len(stripped) < len(line):
            stripped = stripped[:-1]  # an escaped space ("\ ") at the end of the line
        lines.append(stripped)
    title = next(
        (
            i
            for i in range(len(lines) - 1)
            if lines[i].strip()
            and re.fullmatch(r"=+", lines[i + 1])
            and len(lines[i + 1]) >= len(lines[i].strip())
        ),
        None,
    )
    if title is None:
        raise ValueError("no class title")
    name = unescape(lines[title].strip())
    stem = path.stem
    heads = [i for i in range(title + 2, len(lines) - 1) if is_heading(lines, i)]
    pre = lines[title + 2 : heads[0] if heads else len(lines)]
    sections: list[Section] = []
    used_anchor: set[str] = set()
    used_file: set[str] = set()
    for k, h in enumerate(heads):
        end = heads[k + 1] if k + 1 < len(heads) else len(lines)
        items, other = parse_items(lines[h + 2 : end])
        for it in items:
            sig = PERMALINK_RE.sub("", it.sig)
            it.sig_html = inline(ctx, sig)
            it.sig_text = inline(ctx, sig, as_html=False).strip()
            bm = BOLD_RE.search(sig.replace("\\ ", ""))
            it.name = unescape(bm[1]).strip() if bm else it.sig_text
            base = make_id(it.label) if it.label else make_id(f"class_{name}_{it.kind}_{it.name}")
            anchor, n = base, 2
            while anchor in used_anchor:
                anchor, n = f"{base}-{n}", n + 1
            used_anchor.add(anchor)
            it.anchor = anchor
            it.index_type, it.index_name = index_entry(name, it)
            it.page = f"{stem}/{member_file(name, it, used_file)}.html"
            it.body_html = render_blocks(ctx, it.body)
            if it.setget:
                it.setget_html = (
                    '<ul class="setget">' + "".join(f"<li>{inline(ctx, s)}</li>" for s in it.setget) + "</ul>"
                )
        sections.append(Section(lines[h].strip(), render_blocks(ctx, other), items))
    brief = inline(ctx, " ".join(first_paragraph(pre)), as_html=False)
    return ClassDoc(name, stem, make_id(f"class_{name}"), render_blocks(ctx, pre), brief, sections)


# ---------------------------------------------------------------------------------------------
# Pages
# ---------------------------------------------------------------------------------------------


def page_head(title: str, css: str, body_class: str) -> str:
    return (
        '<!DOCTYPE html>\n<html lang="en"><head><meta charset="utf-8">'
        f"<title>{esc(title)}</title>"
        f'<link rel="stylesheet" href="{css}"></head>\n<body class="{body_class}">\n'
    )


def dash_anchor(it: Item) -> str:
    return f'<a name="//apple_ref/cpp/{it.index_type}/{quote(it.index_name, safe="")}" class="dashAnchor"></a>'


def item_block(it: Item, toc: bool) -> str:
    out = [f'<div class="item {it.kind}" id="{it.anchor}">' + (dash_anchor(it) if toc else "")]
    out.append(f'<p class="sig">{it.sig_html}</p>')
    if it.setget_html:
        out.append(it.setget_html)
    if it.body_html:
        out.append(it.body_html)
    if it.constants:
        out.append('<dl class="constants">')
        for c in it.constants:
            out.append(f'<dt id="{c.anchor}">' + (dash_anchor(c) if toc else "") + f"{c.sig_html}</dt>")
            out.append(f"<dd>{c.body_html}</dd>")
        out.append("</dl>")
    out.append("</div>")
    return "\n".join(out)


def class_page(cd: ClassDoc) -> str:
    out = [page_head(cd.name, "style.css", "class"), f'<h1 id="{cd.anchor}">{esc(cd.name)}</h1>', cd.preamble_html]
    for sec in cd.sections:
        out.append(f'<h2 id="{make_id(cd.name + "-" + sec.title)}">{esc(sec.title)}</h2>')
        if sec.html:
            out.append(sec.html)
        for it in sec.items:
            if it.kind == "enumeration-constant" and it.enum is not None:
                continue  # rendered inside its enum
            out.append(item_block(it, toc=True))
    out.append("</body></html>\n")
    return "\n".join(out).replace(HREF_PREFIX, "")


def member_page(cd: ClassDoc, it: Item) -> str:
    word = TYPE_WORDS.get(it.index_type, it.index_type)
    owner = f'<a href="../{cd.stem}.html#{it.anchor}">{esc(cd.name)}</a>'
    if it.enum is not None:
        owner += f', enum <a href="../{cd.stem}.html#{it.enum.anchor}">{esc(it.enum.name)}</a>'
    out = [
        page_head(it.index_name, "../style.css", "member"),
        f"<h1>{esc(it.index_name)}</h1>",
        f'<p class="kind">{esc(word)} of {owner}</p>',
        item_block(it, toc=False),
        "</body></html>\n",
    ]
    return "\n".join(out).replace(HREF_PREFIX, "../")


def index_page(title: str, revision: str, classes: list[ClassDoc]) -> str:
    out = [page_head(title, "style.css", "index"), f"<h1>{esc(title)} class reference</h1>"]
    src = "godot-docs" + (f" @ {esc(revision)}" if revision else "")
    out.append(f'<p class="kind">Built from {src} (classes/*.rst). {len(classes)} classes.</p>')
    out.append('<ul class="classes">')
    for cd in sorted(classes, key=lambda c: c.name.lower()):
        brief = f" — {esc(cd.brief)}" if cd.brief else ""
        out.append(f'<li><a href="{cd.stem}.html">{esc(cd.name)}</a>{brief}</li>')
    out.append("</ul></body></html>\n")
    return "\n".join(out)


# ---------------------------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------------------------


def scan_class_labels(ctx: Ctx, files: list[Path]) -> None:
    for path in files:
        stem = path.stem
        for line in path.read_text(encoding="utf-8-sig").splitlines():
            m = LABEL_RE.match(line)
            if not m:
                continue
            label = m[1]
            is_page = label.lower() == stem.lower() or (label.startswith("class_") and "_" not in label[6:])
            ctx.labels.setdefault(label.lower(), (stem, None if is_page else make_id(label)))


def scan_doc_labels(ctx: Ctx, root: Path) -> None:
    """Labels of the narrative docs (tutorials etc.), so :ref:`doc_x` becomes an online link."""
    for dirpath, dirnames, filenames in os.walk(root):
        rel = Path(dirpath).relative_to(root)
        dirnames[:] = [
            d for d in dirnames if not d.startswith(("_", ".")) and not (rel == Path(".") and d == "classes")
        ]
        for fn in filenames:
            if not fn.endswith(".rst"):
                continue
            try:
                lines = (Path(dirpath) / fn).read_text(encoding="utf-8-sig", errors="replace").splitlines()
            except OSError:
                continue
            page = (rel / fn[:-4]).as_posix()
            for i, line in enumerate(lines):
                m = LABEL_RE.match(line)
                if not m or m[1].lower() in ctx.labels:
                    continue
                title = None
                j = i + 1
                while j < len(lines) and (not lines[j].strip() or LABEL_RE.match(lines[j])):
                    j += 1
                if j + 1 < len(lines):
                    a, b = lines[j].strip(), lines[j + 1].strip()
                    if UNDERLINE_RE.match(a) and b and j + 2 < len(lines) and UNDERLINE_RE.match(lines[j + 2].strip()):
                        title = b
                    elif a and UNDERLINE_RE.match(b) and len(b) >= len(a):
                        title = a
                url = DOCS_URL + page + ".html#" + make_id(m[1])
                ctx.docs.setdefault(m[1].lower(), (url, unescape(title) if title else m[1]))


def godot_version(docs: Path) -> str:
    try:
        conf = (docs / "conf.py").read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""
    m = re.search(r"[\"']godot_version[\"']\s*:\s*[\"']([^\"']+)[\"']", conf)
    return m[1] if m else ""


def git_revision(docs: Path) -> str:
    try:
        r = subprocess.run(
            ["git", "-C", str(docs), "log", "-1", "--format=%h %cs"],
            capture_output=True,
            text=True,
            timeout=30,
        )
        return r.stdout.strip() if r.returncode == 0 else ""
    except (OSError, subprocess.SubprocessError):
        return ""


def dir_size(path: Path) -> tuple[int, int]:
    total = files = 0
    for dirpath, _, filenames in os.walk(path):
        for fn in filenames:
            files += 1
            total += os.path.getsize(os.path.join(dirpath, fn))
    return total, files


def build(docs: Path, out_dir: Path, name: str) -> int:
    started = time.monotonic()
    classes_dir = docs / "classes"
    files = sorted(classes_dir.glob("class_*.rst"))
    if not files:
        print(f"error: no classes/class_*.rst in {docs} (clone https://github.com/godotengine/godot-docs)", file=sys.stderr)
        return 2
    version = godot_version(docs)
    revision = git_revision(docs)
    if version and not version.startswith("4"):
        print(f"warning: godot-docs is for Godot {version}, not 4.x", file=sys.stderr)
    title = f"Godot {version}" if version else "Godot"
    print(f"== {title}: {len(files)} class files in {classes_dir}" + (f" ({revision})" if revision else ""), flush=True)

    ctx = Ctx()
    scan_class_labels(ctx, files)
    scan_doc_labels(ctx, docs)
    print(f"   labels: {len(ctx.labels)} class reference, {len(ctx.docs)} narrative docs", flush=True)

    out_dir.mkdir(parents=True, exist_ok=True)
    final = out_dir / f"{name}.docset"
    tmp = out_dir / f".{name}.docset.building"
    if tmp.exists():
        shutil.rmtree(tmp)
    res = tmp / "Contents" / "Resources"
    documents = res / "Documents"
    documents.mkdir(parents=True)

    rows: list[tuple[str, str, str]] = []
    classes: list[ClassDoc] = []
    failed: list[str] = []
    pages = 0
    for n, path in enumerate(files, 1):
        try:
            cd = parse_class(ctx, path)
        except (ValueError, UnicodeDecodeError) as e:
            failed.append(f"{path.name}: {e}")
            continue
        classes.append(cd)
        (documents / f"{cd.stem}.html").write_text(class_page(cd), encoding="utf-8", newline="\n")
        rows.append((cd.name, "Class", f"{cd.stem}.html"))
        members = [it for sec in cd.sections for it in sec.items]
        if members:
            (documents / cd.stem).mkdir(exist_ok=True)
        for it in members:
            (documents / it.page).write_text(member_page(cd, it), encoding="utf-8", newline="\n")
            rows.append((it.index_name, it.index_type, it.page))
        pages += 1 + len(members)
        if n % 200 == 0:
            print(f"   {n}/{len(files)} classes, {pages} pages", flush=True)

    (documents / "index.html").write_text(index_page(title, revision, classes), encoding="utf-8", newline="\n")
    (documents / "style.css").write_text(CSS, encoding="utf-8", newline="\n")
    ident = name.lower()
    (tmp / "Contents" / "Info.plist").write_text(
        PLIST.format(ident=attr(ident), title=attr(title)), encoding="utf-8", newline="\n"
    )
    meta = {"name": name, "title": title, "version": version or "4", "revision": revision}
    (tmp / "meta.json").write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8", newline="\n")

    con = sqlite3.connect(res / "docSet.dsidx")
    try:
        con.execute("CREATE TABLE searchIndex(id INTEGER PRIMARY KEY, name TEXT, type TEXT, path TEXT)")
        con.execute("CREATE UNIQUE INDEX anchor ON searchIndex (name, type, path)")
        con.executemany("INSERT OR IGNORE INTO searchIndex(name, type, path) VALUES (?, ?, ?)", rows)
        con.commit()
        counts = con.execute("SELECT type, COUNT(*) FROM searchIndex GROUP BY type ORDER BY 2 DESC").fetchall()
        total_rows = con.execute("SELECT COUNT(*) FROM searchIndex").fetchone()[0]
    finally:
        con.close()

    old = out_dir / f".{name}.docset.old"
    if old.exists():
        shutil.rmtree(old)
    try:
        if final.exists():
            final.rename(old)
        tmp.rename(final)
    except OSError as e:
        print(f"error: cannot replace {final}: {e} (is it open in Zeal?). New build left in {tmp}", file=sys.stderr)
        return 1
    if old.exists():
        shutil.rmtree(old, ignore_errors=True)

    size, nfiles = dir_size(final)
    print(f"== {final}")
    print(f"   classes {len(classes)}, pages {pages}, files {nfiles}, size {size / 1048576:.1f} MB")
    print(f"   index rows {total_rows}: " + ", ".join(f"{t} {c}" for t, c in counts))
    for f in failed:
        print(f"   skipped {f}", file=sys.stderr)
    print(f"   done in {time.monotonic() - started:.1f} s", flush=True)
    return 0


def main() -> int:
    if sys.version_info < (3, 11):
        print("error: Python 3.11+ required", file=sys.stderr)
        return 2
    ap = argparse.ArgumentParser(description="Build a Dash/Zeal docset of the Godot 4 class reference.")
    ap.add_argument("--docs", required=True, type=Path, help="godot-docs checkout (contains classes/)")
    ap.add_argument("--out", required=True, type=Path, help="docsets folder; writes <out>/<name>.docset")
    ap.add_argument("--name", default="Godot", help="docset name (default: Godot)")
    a = ap.parse_args()
    if not re.fullmatch(r"[A-Za-z0-9_+-]+", a.name):
        print(f"error: bad docset name {a.name!r}", file=sys.stderr)
        return 2
    return build(a.docs.resolve(), a.out.resolve(), a.name)


if __name__ == "__main__":
    sys.exit(main())
