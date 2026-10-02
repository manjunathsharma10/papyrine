import { useEffect, useRef } from "react";
import { fetchTile, reportFirstTilePainted, reportUiError, TILE_SIZE } from "./tile";

export function App() {
  const canvas = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      const bitmap = await fetchTile(0, 0, 0);
      const ctx = canvas.current?.getContext("2d");
      if (cancelled || !ctx) return;
      ctx.drawImage(bitmap, 0, 0);
      bitmap.close();
      // Two frames: the first commits the draw, the second is after it was presented.
      requestAnimationFrame(() => requestAnimationFrame(() => void reportFirstTilePainted()));
    })().catch((e) => {
      console.error(e);
      void reportUiError(String(e));
    });
    return () => {
      cancelled = true;
    };
  }, []);

  return <canvas ref={canvas} width={TILE_SIZE} height={TILE_SIZE} aria-label="Static test tile" />;
}
