import * as Tabs from "@radix-ui/react-tabs";
import { useTranslation } from "react-i18next";
import { Icon } from "../icons/Icon";
import { type DocState, type LeftPane as Pane, useApp } from "../store/app";
import { Bookmarks } from "./Bookmarks";
import { Thumbnails } from "./Thumbnails";

export function LeftPane({ doc, docId, pane }: { doc: DocState; docId: string; pane: Pane }) {
  const { t } = useTranslation();
  const setPane = useApp((s) => s.setLeftPane);
  return (
    <Tabs.Root className="left-pane-root pane-content" value={pane} onValueChange={(v) => setPane(v as Pane)} activationMode="automatic">
      <Tabs.List className="pane-tabs" aria-label={t("pane.left")} data-region="left-tabs">
        <Tabs.Trigger value="thumbnails" className="pane-tab" data-testid="tab-thumbnails">
          <Icon name="thumbnails" size={16} /> {t("pane.thumbnails")}
        </Tabs.Trigger>
        <Tabs.Trigger value="bookmarks" className="pane-tab" data-testid="tab-bookmarks">
          <Icon name="bookmarks" size={16} /> {t("pane.bookmarks")}
        </Tabs.Trigger>
      </Tabs.List>
      <Tabs.Content value="thumbnails" className="pane-content">
        <Thumbnails doc={doc} docId={docId} />
      </Tabs.Content>
      <Tabs.Content value="bookmarks" className="pane-content">
        <Bookmarks doc={doc} />
      </Tabs.Content>
    </Tabs.Root>
  );
}
