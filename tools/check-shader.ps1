# Compiles a Godot 4 .gdshader headlessly and prints COMPILES or the shader errors.
# Usage: powershell -NoProfile -File tools\check-shader.ps1 <path\to\shader.gdshader>
param([Parameter(Mandatory = $true)][string]$Shader)
$ErrorActionPreference = "Stop"
$godot = "F:\PROJECTS\godot-hd\engine\bin\godot.windows.editor.x86_64.exe"
$project = "D:\AI\godot-shader-check"
if (-not (Test-Path $godot)) { Write-Output "Godot not found at $godot"; exit 2 }
$src = Resolve-Path $Shader
Copy-Item $src "$project\_under_test.gdshader" -Force
$out = cmd /c "`"$godot`" --headless --path `"$project`" --script res://check.gd -- res://_under_test.gdshader 2>&1" | Out-String
$errors = ($out -split "`n") | Where-Object { $_ -match "SHADER ERROR|^E\s+\d+->|Expected|ERROR" }
$uniforms = if ($out -match "uniforms=(\d+)") { [int]$Matches[1] } else { 0 }
if (-not $errors -and $out -match "RESULT: LOADED") {
  Write-Output "COMPILES: $Shader ($uniforms uniforms)"
  exit 0
}
Write-Output "SHADER ERRORS in ${Shader}:"
$errors | Select-Object -First 12 | ForEach-Object { Write-Output $_.TrimEnd() }
exit 1
