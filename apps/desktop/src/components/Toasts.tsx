import { useTranslation } from "react-i18next";
import { useApp } from "../store/app";

export function Toasts() {
  const { t } = useTranslation();
  const toasts = useApp((s) => s.toasts);
  return (
    <div className="toasts" role="status" aria-live="polite">
      {toasts.map((x) => (
        <div className="toast" key={x.id} data-kind={x.kind}>
          {t(x.key, x.params)}
        </div>
      ))}
    </div>
  );
}
