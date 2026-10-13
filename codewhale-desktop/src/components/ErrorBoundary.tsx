import { Component, type ReactNode } from "react";

interface State {
  error: Error | null;
}

/** Keeps a rendering bug from blanking the whole window. */
export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error) {
    console.error("UI error:", error);
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div className="flex h-full items-center justify-center bg-base p-6 text-fg">
        <div className="max-w-xl text-center">
          <div className="mb-2 text-4xl">🐋</div>
          <h1 className="text-[18px] font-semibold">Что-то пошло не так в интерфейсе</h1>
          <p className="mt-1 text-[13px] text-fg2">Агент и ваши сессии не пострадали: они хранятся в CodeWhale.</p>
          <pre className="mt-3 max-h-48 overflow-auto rounded border border-line bg-elevated p-2 text-left font-mono text-[11px] text-red">
            {String(this.state.error?.stack ?? this.state.error)}
          </pre>
          <button className="mt-4 rounded bg-blue px-4 py-1.5 text-[13px] text-white" onClick={() => location.reload()}>
            Перезагрузить интерфейс
          </button>
        </div>
      </div>
    );
  }
}
