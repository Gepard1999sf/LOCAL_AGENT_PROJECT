# Compiles a Godot 4 .gdshader headlessly and prints COMPILES or the shader errors.
# Usage: powershell -NoProfile -File tools\check-shader.ps1 <path\to\shader.gdshader> [-Godot <godot.exe>]
#
# Uses the check project bundled with the desktop app
# (codewhale-desktop\src-tauri\resources\godot-check: project.godot + check.gd), the same one the
# ai-drive MCP tool `godot_check` runs. check.gd loads the shader from the path given after `--`,
# so nothing is copied into the project folder.
param(
  [Parameter(Mandatory = $true)][string]$Shader,
  [string]$Godot = "F:\PROJECTS\godot-hd\engine\bin\godot.windows.editor.x86_64.exe"
)
$ErrorActionPreference = "Stop"
if ($env:GODOT_BIN -and -not $PSBoundParameters.ContainsKey("Godot")) { $Godot = $env:GODOT_BIN }
$project = Join-Path $PSScriptRoot "..\codewhale-desktop\src-tauri\resources\godot-check"
if (-not (Test-Path $Godot)) { Write-Output "Godot not found at $Godot (pass -Godot or set GODOT_BIN)"; exit 2 }
if (-not (Test-Path (Join-Path $project "check.gd"))) { Write-Output "Check project not found at $project"; exit 2 }
$project = (Resolve-Path $project).Path
$src = (Resolve-Path $Shader).Path
# Godot 4.4+ prints "RESULT: LOADED" and "uniforms=N" from check.gd; shader errors go to stderr as
# "SHADER ERROR: ..." followed by "ERROR: Shader compilation failed.".
$out = cmd /c "`"$Godot`" --headless --path `"$project`" --script res://check.gd -- `"$src`" 2>&1" | Out-String
$errors = ($out -split "`n") | Where-Object { $_ -match "SHADER ERROR|^E\s+\d+->|Expected|ERROR|CANNOT_OPEN" }
$uniforms = if ($out -match "uniforms=(\d+)") { [int]$Matches[1] } else { 0 }
if (-not $errors -and $out -match "RESULT: LOADED") {
  Write-Output "COMPILES: $Shader ($uniforms uniforms)"
  exit 0
}
Write-Output "SHADER ERRORS in ${Shader}:"
$errors | Select-Object -First 12 | ForEach-Object { Write-Output $_.TrimEnd() }
if (-not $errors) { Write-Output ($out.Trim() -split "`n" | Select-Object -Last 5) }
exit 1
