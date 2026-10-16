import { Tooltip } from "radix-ui";
import { Toaster } from "sonner";
import { useApp } from "./store/app";
import { useBootstrap } from "./hooks/useBootstrap";
import { useShortcuts } from "./hooks/useShortcuts";
import { Toolbar } from "./components/layout/Toolbar";
import { Sidebar } from "./components/layout/Sidebar";
import { Inspector } from "./components/layout/Inspector";
import { StatusBar } from "./components/layout/StatusBar";
import { ChatArea } from "./components/chat/ChatArea";
import { OllamaOffline, RuntimeFailed } from "./components/screens/StatusScreens";
import { SettingsPanel } from "./components/overlays/SettingsPanel";
import { CommandPalette } from "./components/overlays/CommandPalette";
import { AboutPanel, LogsPanel, ModelsPanel, ShortcutsPanel, UsagePanel } from "./components/overlays/SidePanels";
import { KnowledgeDrivePanel } from "./components/drive/KnowledgeDrivePanel";

function Main() {
  const runtime = useApp((s) => s.runtime.state);
  const ollamaOnline = useApp((s) => s.ollama?.online);
  if (ollamaOnline === false) return <div className="flex-1"><OllamaOffline /></div>;
  if (runtime === "failed") return <div className="flex-1"><RuntimeFailed /></div>;
  return <ChatArea />;
}

function Overlays() {
  const overlay = useApp((s) => s.overlay);
  switch (overlay) {
    case "settings":
      return <SettingsPanel />;
    case "palette":
      return <CommandPalette />;
    case "shortcuts":
      return <ShortcutsPanel />;
    case "about":
      return <AboutPanel />;
    case "logs":
      return <LogsPanel />;
    case "models":
      return <ModelsPanel />;
    case "usage":
      return <UsagePanel />;
    case "drive":
      return <KnowledgeDrivePanel />;
    default:
      return null;
  }
}

export default function App() {
  useBootstrap();
  useShortcuts();
  const theme = useApp((s) => s.settings?.theme);
  const width = useApp((s) => s.windowWidth);
  const narrow = width < 700;
  const sidebarCollapsed = useApp((s) => s.sidebarCollapsed);
  return (
    <Tooltip.Provider delayDuration={400}>
      <div className="relative flex h-full flex-col">
        <Toolbar />
        <div className="relative flex min-h-0 flex-1">
          {narrow ? (
            // Under 700 px only the chat stays; the sidebar opens as an overlay.
            !sidebarCollapsed && (
              <div className="absolute inset-y-0 left-0 z-20 shadow-[var(--shadow-lg)]">
                <Sidebar />
              </div>
            )
          ) : (
            <Sidebar />
          )}
          <Main />
          {!narrow && <Inspector />}
        </div>
        <StatusBar />
        <Overlays />
      </div>
      <Toaster
        position="bottom-right"
        offset={40}
        theme={theme === "light" ? "light" : theme === "auto" ? "system" : "dark"}
        toastOptions={{
          style: {
            background: "var(--bg-overlay)",
            border: "1px solid var(--border-default)",
            color: "var(--text-primary)",
            fontSize: 13,
          },
        }}
      />
    </Tooltip.Provider>
  );
}
