import { createContext } from "react";

export interface ViewportRect {
  top: number;
  left: number;
  width: number;
  height: number;
}

export interface ViewportSource {
  get(): ViewportRect;
  subscribe(cb: () => void): () => void;
}

export const ViewportContext = createContext<ViewportSource>({
  get: () => ({ top: 0, left: 0, width: 0, height: 0 }),
  subscribe: () => () => undefined,
});
