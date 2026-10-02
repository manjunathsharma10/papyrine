import { useTranslation } from "react-i18next";
import { IconButton } from "./ui";
import { Icon } from "../icons/Icon";
import { type DocState, useApp } from "../store/app";

export function Banners({ doc, docId }: { doc: DocState; docId: string }) {
  const { t } = useTranslation();
  const dismiss = useApp((s) => s.dismissBanner);
  const openSources = useApp((s) => s.openSources);
  const closeTab = useApp((s) => s.closeTab);
  if (doc.banners.length === 0) return null;
  return (
    <div role="region" aria-label={t("banner.region")}>
      {doc.banners.map((b) => (
        <div className="banner" key={b} data-testid={`banner-${b}`}>
          <Icon name="warning" />
          <span className="msg">{t(b === "repaired" ? "banner.repaired" : "banner.changed", { name: doc.info.name })}</span>
          {b === "changed-on-disk" && doc.info.path && !doc.info.dirty && (
            <button
              type="button"
              className="btn btn-outline"
              onClick={async () => {
                const path = doc.info.path as string;
                await closeTab(docId, true);
                await openSources([{ kind: "path", path }]);
              }}
            >
              {t("banner.reload")}
            </button>
          )}
          <IconButton icon="close" label={t("banner.dismiss")} onClick={() => dismiss(docId, b)} />
        </div>
      ))}
    </div>
  );
}
