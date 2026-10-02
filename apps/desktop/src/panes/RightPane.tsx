import { Suspense } from "react";
import { useTranslation } from "react-i18next";
import { ErrorBoundary } from "../components/ErrorBoundary";
import { IconButton } from "../components/ui";
import { Scroll } from "../components/Scroll";
import { useApp } from "../store/app";
import { findTool } from "../tools/registry";
import { PANELS } from "./panels";

export function RightPane({ toolId }: { toolId: string }) {
  const { t } = useTranslation();
  const close = useApp((s) => s.setRightTool);
  const tool = findTool(toolId);
  const Panel = PANELS[toolId];
  const title = toolId === "all-tools" ? t("toolbar.allTools") : tool ? t(tool.titleKey) : toolId;
  return (
    <aside className="right-pane" aria-label={title} data-region="right" data-testid="right-pane">
      <div className="pane-header">
        <h2 className="pane-title">{title}</h2>
        <IconButton icon="close" label={t("pane.close")} onClick={() => close(null)} />
      </div>
      <Scroll>
        <ErrorBoundary>
          <Suspense fallback={<p className="empty-note">{t("common.loading")}</p>}>
            {Panel ? <Panel toolId={toolId} /> : null}
          </Suspense>
        </ErrorBoundary>
      </Scroll>
    </aside>
  );
}
