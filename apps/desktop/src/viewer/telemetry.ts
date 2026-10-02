let reported = false;

/**
 * Inside Tauri, tell the host the first tile reached the screen (launch
 * measurement, spike 0.1). A no-op in a plain browser.
 */
export function reportFirstTile(): void {
  if (reported) return;
  reported = true;
  if (!("__TAURI_INTERNALS__" in window)) return;
  void import("../tile").then(({ reportFirstTilePainted }) => {
    void reportFirstTilePainted("drawn").catch(() => undefined);
    requestAnimationFrame(() => requestAnimationFrame(() => void reportFirstTilePainted("presented").catch(() => undefined)));
  });
}
