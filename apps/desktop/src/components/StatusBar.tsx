import { useTranslation } from "react-i18next";
import { useActiveDoc, useApp } from "../store/app";
import { formatZoom } from "../viewer/zoom";

export function StatusBar() {
  const { t } = useTranslation();
  const doc = useActiveDoc();
  const setGoTo = useApp((s) => s.setGoTo);
  return (
    <footer className="statusbar" data-region="status" aria-label={t("status.label")}>
      {doc ? (
        <>
          <button type="button" className="btn" onClick={() => setGoTo(true)} data-testid="status-page" aria-label={t("status.goToPage")}>
            {t("status.page", { page: doc.currentPage + 1, total: doc.info.pageCount })}
          </button>
          <span className="spacer" />
          <span data-testid="status-view">{t(doc.viewMode === "single" ? "cmd.view.single" : "cmd.view.continuous")}</span>
          <span data-testid="status-zoom" aria-label={t("status.zoomLabel")}>
            {formatZoom(doc.zoom)}
          </span>
        </>
      ) : (
        <span>{t("status.ready")}</span>
      )}
    </footer>
  );
}
