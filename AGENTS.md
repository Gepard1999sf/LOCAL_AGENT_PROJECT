# Instructions for the CodeWhale agent

## Language
- Always answer the user in Russian.
- Code, identifiers, file names and code comments stay in English.

## How to work
- Read the relevant files before changing them. Keep changes small and focused.
- After editing code, verify it (build, test or a check script) before saying it works.
- The shell is Windows PowerShell 5.1: no `&&`, no Unix tools like grep/sed/head.

## Godot
- Godot 4 binary: `F:\PROJECTS\godot-hd\engine\bin\godot.windows.editor.x86_64.exe` (custom 4.8 build).
- For any `.gdshader` work, load the `godot-shaders` skill first, then compile-check every shader:
  `powershell -NoProfile -File tools\check-shader.ps1 <path>` and fix errors until it prints `COMPILES`.
