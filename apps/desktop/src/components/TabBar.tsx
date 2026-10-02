import * as Tabs from "@radix-ui/react-tabs";
import { useTranslation } from "react-i18next";
import { Icon } from "../icons/Icon";
import { useApp } from "../store/app";

/** Document tabs. The close glyph is pointer-only; keyboard users press Delete or Mod+W. */
export function TabBar() {
  const { t } = useTranslation();
  const tabs = useApp((s) => s.tabs);
  const docs = useApp((s) => s.docs);
  const closeTab = useApp((s) => s.closeTab);
  if (tabs.length === 0) return null;
  return (
    <Tabs.List className="tabbar" aria-label={t("tabs.label")} data-region="tabs">
      {tabs.map((id) => {
        const d = docs[id];
        if (!d) return null;
        return (
          <Tabs.Trigger
            key={id}
            value={id}
            className="doc-tab"
            data-testid={`doc-tab`}
            title={d.info.path ?? d.info.name}
            aria-keyshortcuts="Delete"
            onKeyDown={(e) => {
              if (e.key === "Delete" || e.key === "Backspace") {
                e.preventDefault();
                void closeTab(id);
              }
            }}
            onAuxClick={(e) => {
              if (e.button === 1) void closeTab(id);
            }}
          >
            <Icon name="file" size={16} />
            <span className="name">{d.info.name}</span>
            {d.info.dirty && (
              <>
                <span className="dirty" aria-hidden="true" />
                <span className="sr-only">{t("tabs.unsaved")}</span>
              </>
            )}
            <span
              className="close"
              aria-hidden="true"
              data-testid="tab-close"
              onPointerDown={(e) => e.stopPropagation()}
              onMouseDown={(e) => e.stopPropagation()}
              onClick={(e) => {
                e.stopPropagation();
                void closeTab(id);
              }}
            >
              <Icon name="close" size={14} />
            </span>
          </Tabs.Trigger>
        );
      })}
    </Tabs.List>
  );
}
