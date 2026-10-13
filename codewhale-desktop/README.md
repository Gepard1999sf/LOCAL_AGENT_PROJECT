# CodeWhale Desktop

Десктоп-клиент (Tauri 2 + React) для локального агента CodeWhale. Приложение само запускает
`codewhale app-server --http` и работает с моделями Ollama.

## Что нужно

- Node.js и npm, Rust (rustup, toolchain MSVC);
- CodeWhale: `npm i -g codewhale` (или путь к `codewhale.exe` в настройках приложения);
- Ollama с моделью, которая умеет вызывать инструменты.

## Папка сборки Rust

Сборка Rust занимает несколько гигабайт. Без настройки она идёт в `src-tauri\target`.
Чтобы держать её на другом диске, один раз выполните в PowerShell и откройте новое окно терминала:

```powershell
[Environment]::SetEnvironmentVariable('CARGO_TARGET_DIR', 'D:\AI\build\codewhale-desktop-target', 'User')
```

## Разработка

```powershell
npm install
npm run tauri dev
```

В режиме разработки CodeWhale берётся из пути в настройках, из установки npm или из `PATH`.

## Установщик

```powershell
npm run bundle
```

Команда копирует `codewhale.exe` (из установки npm или из `PATH`) в
`src-tauri\binaries\codewhale-app-server-<triple>.exe` и собирает установщики NSIS и MSI.
Они появляются в `release\bundle\` внутри папки сборки Rust.

Если `codewhale.exe` лежит в другом месте, подготовьте его отдельно и соберите без повторного копирования:

```powershell
npm run sidecar -- -Source C:\путь\к\codewhale.exe
npx tauri build --config src-tauri/tauri.bundle.conf.json
```
