import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { initI18n } from "./i18n";
import { trackFocus } from "./components/focus";
import { getHost } from "./ipc";
import { applyTheme, useApp } from "./store/app";
import "./styles/tokens.css";
import "./styles/app.css";

async function boot() {
  const { theme, locale } = useApp.getState();
  applyTheme(theme);
  trackFocus();
  await initI18n(locale);
  if (import.meta.env.DEV) {
    // Test hook: lets E2E drive host events (file changes, prompts) the mock can emit.
    (window as unknown as Record<string, unknown>).__papyrine = { host: getHost(), store: useApp };
  }
  const root = document.getElementById("root");
  if (!root) throw new Error("missing #root");
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
  // Warm the small-dialog chunk and the most likely panel while idle.
  const idle = (window as unknown as { requestIdleCallback?: (cb: () => void) => void }).requestIdleCallback ?? ((cb: () => void) => setTimeout(cb, 500));
  idle(() => {
    void import("./components/Dialogs");
    void import("./panes/panels/SearchPanel");
  });
}

void boot();
