import type { OllamaModel } from "./types";

/** Context the model really runs with: Ollama caps num_ctx at the model's native window. */
export function effectiveCtx(m?: OllamaModel): number | null {
  if (!m?.numCtx) return null;
  return m.nativeCtx ? Math.min(m.numCtx, m.nativeCtx) : m.numCtx;
}
