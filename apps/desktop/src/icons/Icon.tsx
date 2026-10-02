import type { SVGProps } from "react";

/**
 * Original icon set: 20x20 grid, 1.6 stroke, round caps and joins, drawn for
 * Papyrine. Every icon is decorative (aria-hidden); the control that hosts it
 * supplies the accessible name.
 */
const PATHS = {
  open: "M2.5 5.5a1.5 1.5 0 0 1 1.5-1.5h3.4l1.8 2H16a1.5 1.5 0 0 1 1.5 1.5v7A1.5 1.5 0 0 1 16 16H4a1.5 1.5 0 0 1-1.5-1.5v-9Z M2.5 9h15",
  save: "M4 3.5h9l3.5 3.5v9a.5.5 0 0 1-.5.5H4a.5.5 0 0 1-.5-.5v-12a.5.5 0 0 1 .5-.5Z M6.5 3.5v4h5v-4 M6.5 16.5v-5h7v5",
  search: "M8.5 14a5.5 5.5 0 1 0 0-11 5.5 5.5 0 0 0 0 11Z M12.5 12.5l4.5 4.5",
  annotate: "M12.5 3.5l4 4-8 8H4.5v-4l8-8Z M10.5 5.5l4 4 M3.5 17.5h13",
  fillsign: "M4 3.5h8l3 3v4 M12 3.5v3h3 M4 3.5v13h6 M6.5 9h5 M6.5 12h3 M11 16.5c1.5 0 1.5-2.5 3-2.5s1 2 3 2",
  organize: "M3.5 3.5h5.5v5.5H3.5z M11 3.5h5.5v5.5H11z M3.5 11H9v5.5H3.5z M11 11h5.5v5.5H11z",
  view: "M1.8 10S5 4.5 10 4.5 18.2 10 18.2 10 15 15.5 10 15.5 1.8 10 1.8 10Z M10 12.3a2.3 2.3 0 1 0 0-4.6 2.3 2.3 0 0 0 0 4.6Z",
  "zoom-in": "M8.5 14a5.5 5.5 0 1 0 0-11 5.5 5.5 0 0 0 0 11Z M12.5 12.5l4.5 4.5 M6.3 8.5h4.4 M8.5 6.3v4.4",
  "zoom-out": "M8.5 14a5.5 5.5 0 1 0 0-11 5.5 5.5 0 0 0 0 11Z M12.5 12.5l4.5 4.5 M6.3 8.5h4.4",
  "fit-page": "M3.5 7V3.5H7 M13 3.5h3.5V7 M16.5 13v3.5H13 M7 16.5H3.5V13 M7.5 6.5h5v7h-5z",
  "fit-width": "M3 4v12 M17 4v12 M6.5 10h7 M8.5 8l-2 2 2 2 M11.5 8l2 2-2 2",
  actual: "M3.5 7.5l2-1.5V14 M10 8.5h.01 M10 12h.01 M13.5 7.5l2-1.5V14",
  "page-single": "M5.5 3.5h9a.5.5 0 0 1 .5.5v12a.5.5 0 0 1-.5.5h-9a.5.5 0 0 1-.5-.5V4a.5.5 0 0 1 .5-.5Z M8 8h4 M8 11h4",
  "page-continuous": "M5.5 2.5h9 M5.5 2.5v6h9v-6 M5.5 11.5v6h9v-6 M5.5 17.5h9",
  thumbnails: "M3.5 3.5h5v5h-5z M11.5 3.5h5v5h-5z M3.5 11.5h5v5h-5z M11.5 11.5h5v5h-5z",
  bookmarks: "M5.5 3.5h9v13l-4.5-3.2-4.5 3.2v-13Z",
  chevronright: "M7.5 4.5l5.5 5.5-5.5 5.5",
  chevrondown: "M4.5 7.5l5.5 5.5 5.5-5.5",
  chevronleft: "M12.5 4.5L7 10l5.5 5.5",
  chevronup: "M4.5 12.5L10 7l5.5 5.5",
  arrowleft: "M16 10H4.5 M9 5.5L4.5 10 9 14.5",
  arrowright: "M4 10h11.5 M11 5.5l4.5 4.5-4.5 4.5",
  close: "M5 5l10 10 M15 5L5 15",
  more: "M5 10h.01 M10 10h.01 M15 10h.01",
  command: "M7.5 7.5V5a2 2 0 1 0-2 2h9a2 2 0 1 0-2-2v10a2 2 0 1 0 2-2h-9a2 2 0 1 0 2 2V7.5Z",
  tools: "M3.5 3.5h5v5h-5z M11.5 3.5h5v5h-5z M3.5 11.5h5v5h-5z M14 11.5v5 M11.5 14h5",
  sun: "M10 13.5a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7Z M10 2.5v2 M10 15.5v2 M2.5 10h2 M15.5 10h2 M4.7 4.7l1.4 1.4 M13.9 13.9l1.4 1.4 M4.7 15.3l1.4-1.4 M13.9 6.1l1.4-1.4",
  moon: "M16.5 11.5A6.5 6.5 0 0 1 8.5 3.5a6.5 6.5 0 1 0 8 8Z",
  contrast: "M10 17a7 7 0 1 0 0-14 7 7 0 0 0 0 14Z M10 3v14 M10 3a7 7 0 0 1 0 14Z",
  system: "M3 4.5h14v9H3z M7 16.5h6 M10 13.5v3",
  globe: "M10 17.5a7.5 7.5 0 1 0 0-15 7.5 7.5 0 0 0 0 15Z M2.5 10h15 M10 2.5c2.2 2.1 3.2 4.6 3.2 7.5S12.200 15.400 10 17.500c-2.200-2.100-3.200-4.600-3.200-7.500S7.800 4.600 10 2.500Z",
  pin: "M12.5 3l4.5 4.5-2.5.8-3 3 .3 3.200L10.500 15 5 9.500 6.500 8l3.200.3 3-3 .8-2.300Z M5 15l-1.500 1.500",
  info: "M10 17.500a7.500 7.500 0 1 0 0-15 7.500 7.500 0 0 0 0 15Z M10 9v4.500 M10 6.500h.01",
  undo: "M7.500 5L3.500 9l4 4 M3.500 9h8a4.500 4.500 0 0 1 0 9H8",
  redo: "M12.500 5l4 4-4 4 M16.500 9h-8a4.500 4.500 0 0 0 0 9H12",
  file: "M5 2.500h6.500l4 4v10.500a.5.5 0 0 1-.5.5H5a.5.5 0 0 1-.5-.5V3a.5.5 0 0 1 .5-.5Z M11.500 2.500v4h4",
  warning: "M10 3l7.500 13h-15L10 3Z M10 8.500v3.500 M10 14.500h.01",
  check: "M4.500 10.500l3.500 3.500 7.500-8",
  goto: "M4 5.500h8 M4 10h5 M4 14.500h8 M14.500 8v7 M12 12.500l2.500 2.500 2.500-2.500",
  rotate: "M15.500 10a5.500 5.500 0 1 1-1.700-4 M15.500 3.500v3h-3",
} as const;

export type IconName = keyof typeof PATHS;

export function Icon({ name, size = 20, ...rest }: { name: IconName; size?: number } & Omit<SVGProps<SVGSVGElement>, "name">) {
  return (
    <svg
      viewBox="0 0 20 20"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      strokeWidth={1.6}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      {...rest}
    >
      <path d={PATHS[name]} />
    </svg>
  );
}
