import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";

interface Props {
  value: number;
  min: number;
  max: number;
  onChange: (v: number) => void;
  /** Which side of the splitter the pane sits on; decides drag direction. */
  pane: "start" | "end";
  label: string;
}

/** Vertical splitter: pointer drag and keyboard (arrows, Home/End), exposed as a separator. */
export function Splitter({ value, min, max, onChange, pane, label }: Props) {
  const { t } = useTranslation();
  const [dragging, setDragging] = useState(false);
  const start = useRef({ x: 0, v: 0 });
  const clamp = (v: number) => Math.min(max, Math.max(min, v));
  const rtl = () => document.documentElement.dir === "rtl";
  // Growth direction in screen space: a start-side pane grows with +x in LTR.
  const sign = () => (pane === "start" ? 1 : -1) * (rtl() ? -1 : 1);

  return (
    <div
      className="splitter"
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      aria-valuenow={Math.round(value)}
      aria-valuemin={min}
      aria-valuemax={max}
      aria-description={t("splitter.hint")}
      tabIndex={0}
      data-dragging={dragging}
      onPointerDown={(e) => {
        (e.target as HTMLElement).setPointerCapture(e.pointerId);
        start.current = { x: e.clientX, v: value };
        setDragging(true);
      }}
      onPointerMove={(e) => {
        if (dragging) onChange(clamp(start.current.v + (e.clientX - start.current.x) * sign()));
      }}
      onPointerUp={() => setDragging(false)}
      onPointerCancel={() => setDragging(false)}
      onKeyDown={(e) => {
        const step = e.shiftKey ? 48 : 16;
        if (e.key === "ArrowRight") onChange(clamp(value + step * sign()));
        else if (e.key === "ArrowLeft") onChange(clamp(value - step * sign()));
        else if (e.key === "Home") onChange(min);
        else if (e.key === "End") onChange(max);
        else return;
        e.preventDefault();
      }}
    />
  );
}
