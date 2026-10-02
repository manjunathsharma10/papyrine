import { invoke } from "@tauri-apps/api/core";

export const TILE_SIZE = 512;

// Windows/Android WebView2 maps custom schemes to http://<scheme>.localhost.
const isWindows = navigator.userAgent.includes("Windows");
const tileBase = isWindows ? "http://papyrine.localhost" : "papyrine://localhost";

/** Fetch one raw-RGBA tile over papyrine:// and decode it (ADR-009). */
export async function fetchTile(page: number, x: number, y: number): Promise<ImageBitmap> {
  const res = await fetch(`${tileBase}/tile/${page}/${x}/${y}`);
  if (!res.ok) throw new Error(`tile request failed: ${res.status}`);
  const bytes = new Uint8ClampedArray(await res.arrayBuffer());
  if (bytes.length !== TILE_SIZE * TILE_SIZE * 4) throw new Error("unexpected tile size");
  return createImageBitmap(new ImageData(bytes, TILE_SIZE, TILE_SIZE));
}

/**
 * Tell the host about the first tile (launch-time measurement). "drawn" fires
 * right after drawImage; "presented" two animation frames later. WebKit stops
 * animation frames for an occluded window, so the host keeps both.
 */
export function reportFirstTilePainted(stage: "drawn" | "presented"): Promise<void> {
  return invoke("first_tile_painted", { stage });
}

/** Surface a UI failure in the host trace (visible when PAPYRINE_TRACE is set). */
export function reportUiError(message: string): Promise<void> {
  return invoke("ui_error", { message });
}
