import { useTranslation } from "react-i18next";
import { findTool } from "../../tools/registry";

/** Options pane for tools whose engine support lands with the host (annotate, fill & sign). */
export default function PendingPanel({ toolId }: { toolId: string }) {
  const { t } = useTranslation();
  const tool = findTool(toolId);
  return (
    <div className="panel">
      <p>{tool ? t(tool.descriptionKey) : null}</p>
      <p>{t("panel.pendingEngine")}</p>
    </div>
  );
}
