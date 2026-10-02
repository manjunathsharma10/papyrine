import * as Tabs from "@radix-ui/react-tabs";
import * as Tooltip from "@radix-ui/react-tooltip";
import { DirectionProvider } from "@radix-ui/react-direction";
import { lazy, Suspense, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { COMMANDS } from "./commands/registry";
import { isEditableTarget, matchesShortcut } from "./commands/shortcuts";
import CommandPalette from "./components/CommandPalette";
import { Banners } from "./components/Banners";
import { StatusBar } from "./components/StatusBar";
import { Splitter } from "./components/Splitter";
import { TabBar } from "./components/TabBar";
import { Toasts } from "./components/Toasts";
import { Toolbar } from "./components/Toolbar";
import { Welcome } from "./components/Welcome";
import { LOCALES } from "./i18n";
import { getHost } from "./ipc";
import { LeftPane } from "./panes/LeftPane";
import { RightPane } from "./panes/RightPane";
import { useApp } from "./store/app";
import { Viewer } from "./viewer/Viewer";
import { getTileCache } from "./viewer/services";

const EDITABLE_OWNED = new Set(["edit.undo", "edit.redo", "nav.first", "nav.last", "nav.next", "nav.prev"]);

const Dialogs = lazy(() => import("./components/Dialogs"));

export function App() {
  const { t, i18n } = useTranslation();
  const s = useApp();
  const doc = s.activeId ? s.docs[s.activeId] : undefined;
  const [dragging, setDragging] = useState(false);
  const dir = LOCALES.find((l) => l.id === i18n.language)?.dir ?? "ltr";

  useEffect(() => {
    document.title = doc ? `${doc.info.name}${doc.info.dirty ? " *" : ""} - ${t("app.name")}` : t("app.name");
  }, [doc, t, i18n.language]);

  // Host events.
  useEffect(() => {
    const host = getHost();
    const offs = [
      host.on("document-changed", (e) => {
        getTileCache().invalidate(e.info.docId);
        useApp.getState().applyInfo(e.info, e.invalidatedPages);
      }),
      host.on("file-changed-on-disk", (e) => {
        const st = useApp.getState();
        const d = st.docs[e.docId];
        if (d && !d.banners.includes("changed-on-disk")) {
          useApp.setState({ docs: { ...st.docs, [e.docId]: { ...d, banners: [...d.banners, "changed-on-disk"] } } });
        }
      }),
      host.on("action-prompt", (e) => useApp.getState().setPrompt({ docId: e.docId, kind: e.kind, target: e.target })),
      host.on("open-requested", (e) => void useApp.getState().openSources(e.sources)),
    ];
    return () => offs.forEach((off) => off());
  }, []);

  // Global shortcuts.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.isComposing) return;
      const st = useApp.getState();
      const modalOpen = !!document.querySelector('[role="dialog"]');
      const editable = isEditableTarget(e.target);
      for (const cmd of COMMANDS) {
        if (!cmd.shortcut || !matchesShortcut(e, cmd.shortcut)) continue;
        // Text fields keep their own undo/redo and caret movement.
        if (editable && EDITABLE_OWNED.has(cmd.id)) continue;
        if (modalOpen && cmd.id !== "palette.open") continue;
        if (cmd.enabled && !cmd.enabled(st)) continue;
        e.preventDefault();
        void cmd.run(st);
        return;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Drag and drop of files onto the window.
  const onDrop = async (e: React.DragEvent) => {
    e.preventDefault();
    setDragging(false);
    const files = Array.from(e.dataTransfer.files).filter((f) => /\.pdf$/i.test(f.name) || f.type === "application/pdf");
    const sources = await Promise.all(files.map(async (f) => ({ kind: "bytes" as const, name: f.name, data: await f.arrayBuffer() })));
    if (sources.length) await useApp.getState().openSources(sources);
  };

  const showDialogs = s.goToOpen || s.propsOpen || !!s.confirmClose || !!s.prompt;

  return (
    <DirectionProvider dir={dir}>
      <Tooltip.Provider delayDuration={600} skipDelayDuration={200}>
        <Tabs.Root
          className="app"
          value={s.activeId ?? ""}
          onValueChange={s.activate}
          onDragOver={(e) => {
            if (e.dataTransfer.types.includes("Files")) {
              e.preventDefault();
              setDragging(true);
            }
          }}
          onDragLeave={(e) => e.currentTarget === e.target && setDragging(false)}
          onDrop={onDrop}
        >
          <a
            className="skip-link"
            href="#main"
            onClick={(e) => {
              e.preventDefault();
              document.querySelector<HTMLElement>('[data-region="viewer"]')?.focus();
            }}
          >
            {t("a11y.skip")}
          </a>
          <Toolbar />
          <TabBar />
          <div className="body">
            {s.leftPane && doc && s.activeId && (
              <>
                <nav className="left-pane" style={{ inlineSize: s.leftWidth }} aria-label={t("pane.left")}>
                  <LeftPane doc={doc} docId={s.activeId} pane={s.leftPane} />
                </nav>
                <Splitter value={s.leftWidth} min={160} max={480} pane="start" label={t("splitter.left")} onChange={(v) => s.setWidths({ leftWidth: v })} />
              </>
            )}
            <main className="main" id="main">
              {doc && s.activeId ? (
                <Tabs.Content value={s.activeId} className="main" style={{ minBlockSize: 0, flex: 1 }}>
                  <Banners doc={doc} docId={s.activeId} />
                  <Viewer key={s.activeId} doc={doc} docId={s.activeId} />
                </Tabs.Content>
              ) : (
                <Welcome />
              )}
            </main>
            {s.rightTool && (
              <>
                <Splitter value={s.rightWidth} min={240} max={560} pane="end" label={t("splitter.right")} onChange={(v) => s.setWidths({ rightWidth: v })} />
                <div style={{ inlineSize: s.rightWidth, display: "flex", minInlineSize: 0, flex: "none" }}>
                  <RightPane toolId={s.rightTool} />
                </div>
              </>
            )}
          </div>
          <StatusBar />
          <Toasts />
          {dragging && <div className="drop-overlay">{t("drop.hint")}</div>}
          <CommandPalette />
          <Suspense fallback={null}>
            
            {showDialogs && <Dialogs />}
          </Suspense>
        </Tabs.Root>
      </Tooltip.Provider>
    </DirectionProvider>
  );
}
