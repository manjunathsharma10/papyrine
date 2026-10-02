import type { DocId, PageInfo, PageText, Tile, TileRequest } from "../ipc/contract";
import { mockPageText, renderMockTile } from "../ipc/mockPaint";
import type { TileSource } from "./tileSource";

/** Standalone placeholder TileSource: draws pages on canvas, no host needed. */
export class MockTileSource implements TileSource {
  constructor(
    private pages: PageInfo[],
    public delayMs = 0,
  ) {}

  async tile(req: TileRequest, signal?: AbortSignal): Promise<Tile> {
    if (this.delayMs) await new Promise((r) => setTimeout(r, this.delayMs));
    if (signal?.aborted) throw new DOMException("aborted", "AbortError");
    const info = this.pages[req.page];
    if (!info) throw new Error(`no page ${req.page}`);
    return renderMockTile(info, req.page, req.page, req.scale, req.tx, req.ty);
  }

  async text(_docId: DocId, page: number): Promise<PageText> {
    return mockPageText(page, page);
  }
}
