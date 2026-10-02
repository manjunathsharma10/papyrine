import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Icon } from "../../icons/Icon";
import { useApp } from "../../store/app";
import { allToolsSections, type ToolDef } from "../../tools/registry";

/** All Tools: Core, then Standard by group, Advanced behind a disclosure; any tool can be pinned. */
export default function AllToolsPanel() {
  const { t } = useTranslation();
  const pinned = useApp((s) => s.pinned);
  const togglePin = useApp((s) => s.togglePin);
  const toggleTool = useApp((s) => s.toggleTool);
  const setProps = useApp((s) => s.setProps);
  const hasDoc = useApp((s) => s.activeId !== null);
  const [showAdvanced, setShowAdvanced] = useState(false);
  const sections = allToolsSections();
  const advanced = sections.filter((s) => s.tier === "advanced");

  const row = (tool: ToolDef) => (
    <div className="tool-row" key={tool.id}>
      <button
        type="button"
        className="btn open"
        disabled={tool.kind === "menu" || !hasDoc}
        onClick={() => (tool.kind === "dialog" ? setProps(true) : toggleTool(tool.id))}
      >
        <Icon name={tool.icon} />
        <span>
          {t(tool.titleKey)}
          <span className="tool-desc">{t(tool.descriptionKey)}</span>
        </span>
      </button>
      {tool.tier !== "core" && (
        <button
          type="button"
          className="btn btn-icon"
          aria-pressed={pinned.includes(tool.id)}
          aria-label={t("tools.pin", { tool: t(tool.titleKey) })}
          onClick={() => togglePin(tool.id)}
        >
          <Icon name="pin" />
        </button>
      )}
    </div>
  );

  return (
    <div className="panel" data-testid="all-tools">
      {sections
        .filter((s) => s.tier !== "advanced")
        .map((sec) => (
          <section key={`${sec.tier}-${sec.group}`} aria-labelledby={`grp-${sec.group}`}>
            <h3 id={`grp-${sec.group}`}>{t(sec.group === "core" ? "tools.core" : `group.${sec.group}`)}</h3>
            {sec.tools.map(row)}
          </section>
        ))}
      {advanced.length > 0 && (
        <section>
          <button type="button" className="btn" aria-expanded={showAdvanced} onClick={() => setShowAdvanced((v) => !v)}>
            <Icon name={showAdvanced ? "chevrondown" : "chevronright"} size={14} className={showAdvanced ? undefined : "icon-mirror"} />
            {t("tools.advanced")}
          </button>
          {showAdvanced &&
            advanced.map((sec) => (
              <div key={sec.group}>
                <h3>{t(`group.${sec.group}`)}</h3>
                {sec.tools.map(row)}
              </div>
            ))}
        </section>
      )}
    </div>
  );
}
