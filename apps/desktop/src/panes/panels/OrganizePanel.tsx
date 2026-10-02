import { useTranslation } from "react-i18next";
import { Icon } from "../../icons/Icon";
import { useActiveDoc, useApp } from "../../store/app";

/** Organize Pages (shell): drives the engine command IPC on the current page. */
export default function OrganizePanel() {
  const { t } = useTranslation();
  const doc = useActiveDoc();
  const run = useApp((s) => s.run);
  const undo = useApp((s) => s.undo);
  const redo = useApp((s) => s.redo);
  if (!doc) return <p className="empty-note">{t("panel.needsDoc")}</p>;
  const page = doc.currentPage;
  return (
    <div className="panel">
      <p>{t("organize.intro", { page: page + 1 })}</p>
      <button type="button" className="btn btn-outline" onClick={() => void run({ type: "rotate-pages", pages: [page], degrees: 90 })}>
        <Icon name="rotate" /> {t("organize.rotate")}
      </button>
      <button type="button" className="btn btn-outline" disabled={doc.info.pageCount < 2} onClick={() => void run({ type: "delete-pages", pages: [page] })}>
        <Icon name="close" /> {t("organize.delete")}
      </button>
      <button type="button" className="btn btn-outline" disabled={page === 0} onClick={() => void run({ type: "move-pages", pages: [page], to: page - 1 })}>
        <Icon name="chevronup" /> {t("organize.moveUp")}
      </button>
      <button type="button" className="btn btn-outline" disabled={!doc.info.canUndo} onClick={() => void undo()}>
        <Icon name="undo" /> {t("cmd.edit.undo")}
      </button>
      <button type="button" className="btn btn-outline" disabled={!doc.info.canRedo} onClick={() => void redo()}>
        <Icon name="redo" /> {t("cmd.edit.redo")}
      </button>
    </div>
  );
}
