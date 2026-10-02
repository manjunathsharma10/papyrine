import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { Icon } from "../icons/Icon";
import { useApp } from "../store/app";

export function Welcome() {
  const { t } = useTranslation();
  const open = useApp((s) => s.openDialog);
  const openSources = useApp((s) => s.openSources);
  const recents = useApp((s) => s.recents);
  const load = useApp((s) => s.loadRecents);
  useEffect(() => void load(), [load]);
  return (
    <div className="welcome" data-testid="welcome">
      <h1>{t("welcome.title")}</h1>
      <p>{t("welcome.body")}</p>
      <button type="button" className="btn btn-primary" onClick={() => void open()} data-testid="welcome-open">
        <Icon name="open" /> {t("toolbar.open")}
      </button>
      {recents.length > 0 && (
        <>
          <p>{t("welcome.recent")}</p>
          <ul className="recent-list" aria-label={t("welcome.recent")}>
            {recents.map((r) => (
              <li key={r.path}>
                <button type="button" className="btn" onClick={() => void openSources([{ kind: "path", path: r.path }])}>
                  <Icon name="file" /> {r.name}
                </button>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}
