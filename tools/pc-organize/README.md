# pc-organize — аудит и наведение порядка на ПК

Три скрипта для Windows PowerShell 5.1 (запускать из корня репозитория, без прав администратора).
Все сохранены в UTF-8 с BOM, чтобы PowerShell 5.1 читал русский текст.

| Скрипт | Что делает | Меняет диск? |
| --- | --- | --- |
| `Invoke-PcAudit.ps1` | Полный аудит: диски, папки по размерам, AppData и кэши, программы, автозагрузка, игры/движки, Docker/WSL, «Загрузки», рабочий стол, документы, проекты, большие файлы, рекомендации | нет |
| `Invoke-PcOrganize.ps1` | Раскладывает файлы по фиксированной структуре (ниже). По умолчанию — только план; `-Apply` выполняет, `-Undo` откатывает | только с `-Apply` |
| `Invoke-PcRecycle.ps1` | Отправляет кандидатов «DELETE?» из плана сортировки в Корзину (не удаляет насовсем; слишком большие для Корзины элементы пропускает и называет) | только с `-Apply` |
| `Invoke-PcCleanup.ps1` | Удаляет пересоздаваемые кэши (шейдеры NVIDIA, дампы WSL, кэш npm/pip, загрузки Unity Hub, старый Temp) | только с `-Apply` |

Отчёты, планы и журналы складываются в `%USERPROFILE%\Desktop\PC-Audit`.

## 1. Аудит

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools\pc-organize\Invoke-PcAudit.ps1
```

Появятся `PC-Audit-<дата>.md` (читать) и `.json` (сырые данные). Полный проход с подсчётом
размеров и поиском больших файлов занимает 5–15 минут; `-Quick` пропускает медленные части.
Ключ `-ProjectRoots F:\PROJECTS,D:\MY_UNITY_PROJECTS` добавляет папки для поиска проектов.

## 2. Сортировка

```powershell
# план (ничего не перемещает), все группы правил
powershell -NoProfile -ExecutionPolicy Bypass -File tools\pc-organize\Invoke-PcOrganize.ps1 -Root F:\ -All
# выполнить
powershell -NoProfile -ExecutionPolicy Bypass -File tools\pc-organize\Invoke-PcOrganize.ps1 -Root F:\ -All -Apply
# выполнить отредактированный план (лишние строки удалить из CSV)
powershell -NoProfile -ExecutionPolicy Bypass -File tools\pc-organize\Invoke-PcOrganize.ps1 -PlanFile "$env:USERPROFILE\Desktop\PC-Audit\PC-Organize-Plan-<дата>.csv" -Apply
# откат по журналу
powershell -NoProfile -ExecutionPolicy Bypass -File tools\pc-organize\Invoke-PcOrganize.ps1 -Undo "$env:USERPROFILE\Desktop\PC-Audit\PC-Organize-Journal-<дата>.csv"
```

Группы правил включаются ключами (или все сразу `-All`): `-SortDownloads`, `-SortDesktop`,
`-SortDocuments`, `-MoveProjects`, `-MoveInstallers`, `-MovePortable`, `-MoveWorkFolders`,
`-CleanRoots` (лишнее в корнях `C:\` и `F:\`), `-ArchiveDuplicates` (копии и бэкапы в
`F:\PROJECTS`). Репозиторий CodeWhale переносится отдельно ключом `-MoveRepo` — последним шагом,
при закрытом приложении и закрытой сессии Claude (потом `claude` запускать из новой папки).

Скрипт **ничего не удаляет**: то, что выглядит лишним (ярлыки-дубликаты, пустые папки,
распакованные установщики, старый образ Docker), попадает в раздел «Кандидаты на удаление»
плана — решает пользователь. Секреты (SSH-ключи в `Desktop\Octagon-Servers`) помечены как
«вручную» и не трогаются.

Как перемещает: файлы и папки на том же диске — `Move-Item` (мгновенно), папки между дисками —
`robocopy /E /MOVE` (длинные пути `node_modules` не мешают). Занятые файлы остаются на месте, в
журнале такая строка помечена `PARTIAL`. При совпадении имени в назначении добавляется ` (2)`
(`-OnConflict Skip` — пропускать).

### Структура после сортировки

```
C:\Users\<вы>\
  Desktop\          только ярлыки и PC-Audit
  Documents\
    Work\Salary       зарплатные таблицы, Сводные, Export_*, html-инструменты
    Work\CS-System    документы CS System / Crystall / chlite / ТЗ
    Work\Other
    Personal          билеты, визы, квитанции, резюме, выписки
    Personal\Security коды восстановления (лучше перенести в менеджер паролей)
    Guides\Godot      руководства и планы по Godot
    Guides\AI-agents  документы по CodeWhale и локальным агентам
    Guides\Art, Guides\Other
    Unsorted          то, что не распознано
  Pictures\Downloads, Pictures\GIF, Pictures\Screenshots
  Videos\Downloads
  Downloads\Torrents, Downloads\Unsorted
D:\MY_UNITY_PROJECTS\   проекты Unity (в Unity Hub добавить заново через Add)
F:\
  AI_Drive\           диск знаний (раздел 4 хэндоффа, создаётся позже)
  AI\CodeWhale\       всё, с чем работаем вместе: LOCAL_AGENT_PROJECT (репозиторий), portable\, releases\, ollama\
  PROJECTS\           все проекты с кодом (+ _godot-sandbox для тестовых проектов Godot)
  Software\
    Installers\<категория>   Drivers / Development / AI-assistants / Graphics-3D / Communication-Browsers / Audio-Video / Games-Mods / Other
    Portable\<программа>     Godot-4.7.2, DBeaver, chlite, rclone
    Drivers\
    Unreal-Assets\           плагины и паки Unreal
  Archive\Backups, Archive\DB-dumps, Archive\Old-Projects, Archive\Misc
  ART BASE\Artbooks, Reference, Drawings, 3D
```

## 3. Кандидаты на удаление → Корзина

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools\pc-organize\Invoke-PcRecycle.ps1 -PlanFile "$env:USERPROFILE\Desktop\PC-Audit\PC-Organize-Plan-<дата>.csv"
powershell -NoProfile -ExecutionPolicy Bypass -File tools\pc-organize\Invoke-PcRecycle.ps1 -PlanFile "..." -Apply
```

Берёт строки `DELETE?` из плана и отправляет их в Корзину (журнал `PC-Recycle-Journal-<дата>.csv`).
Элементы крупнее лимита Корзины своего диска (например образ Docker на 53 ГБ) не трогает и
перечисляет: их удаляют вручную. Место освобождается после очистки Корзины.

## 4. Чистка кэшей

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools\pc-organize\Invoke-PcCleanup.ps1          # показать
powershell -NoProfile -ExecutionPolicy Bypass -File tools\pc-organize\Invoke-PcCleanup.ps1 -Apply   # удалить
```

Удаляются только кэши, которые программы пересоздают сами. Что требует решения пользователя
и не делается скриптом: удаление программ (`winget uninstall` или «Параметры → Приложения»),
перенос игр Steam/Epic на другой диск, лишние редакторы Unity (через Unity Hub), образ Docker
(Docker Desktop → Settings → Resources → Disk image location), `powercfg /h off` для hiberfil.sys.
