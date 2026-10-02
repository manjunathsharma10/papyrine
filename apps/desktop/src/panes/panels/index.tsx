import { lazy, type ComponentType, type LazyExoticComponent } from "react";

type Panel = LazyExoticComponent<ComponentType<{ toolId: string }>>;

/** Every non-core panel is its own chunk, loaded on first use. */
export const PANELS: Record<string, Panel> = {
  search: lazy(() => import("./SearchPanel")),
  organize: lazy(() => import("./OrganizePanel")),
  "all-tools": lazy(() => import("./AllToolsPanel")),
  annotate: lazy(() => import("./PendingPanel")),
  "fill-sign": lazy(() => import("./PendingPanel")),
};
