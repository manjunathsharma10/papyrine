import type { DocId, HostApi, PageText, Tile, TileRequest } from "../ipc/contract";

/**
 * Where the viewer gets pixels and text. The real implementation wraps the
 * host (tiles over papyrine://); the mock draws placeholder pages.
 */
export interface TileSource {
  tile(req: TileRequest, signal?: AbortSignal): Promise<Tile>;
  text(docId: DocId, page: number, signal?: AbortSignal): Promise<PageText>;
}

export function hostTileSource(host: HostApi): TileSource {
  return {
    tile: (req, signal) => host.getTile(req, signal),
    text: (docId, page, signal) => host.getPageText(docId, page, signal),
  };
}

export const tileKey = (r: TileRequest): string => `${r.docId}|${r.revision}|${r.page}|${r.scale.toFixed(4)}|${r.tx}|${r.ty}`;

interface Entry {
  promise: Promise<Tile>;
  tile: Tile | null;
  refs: number;
  controller: AbortController;
}

/**
 * LRU tile cache with request de-duplication and ref-counted cancellation:
 * a request nobody waits for any more is aborted before the renderer starts.
 */
export class TileCache {
  private entries = new Map<string, Entry>();
  private bytes = 0;

  constructor(
    private source: TileSource,
    private maxBytes = 160 * 1024 * 1024,
  ) {}

  /** Cached tile if ready (marks it recently used). */
  peek(req: TileRequest): Tile | null {
    const e = this.entries.get(tileKey(req));
    if (!e?.tile) return null;
    this.entries.delete(tileKey(req));
    this.entries.set(tileKey(req), e);
    return e.tile;
  }

  /** Acquire a tile; call `release` when the consumer goes away. */
  acquire(req: TileRequest): { promise: Promise<Tile>; release: () => void } {
    const key = tileKey(req);
    let e = this.entries.get(key);
    if (!e) {
      const controller = new AbortController();
      const entry: Entry = { promise: Promise.resolve(null as unknown as Tile), tile: null, refs: 0, controller };
      entry.promise = this.source.tile(req, controller.signal).then(
        (tile) => {
          entry.tile = tile;
          this.bytes += tile.width * tile.height * 4;
          this.evict();
          return tile;
        },
        (err) => {
          if (this.entries.get(key) === entry) this.entries.delete(key);
          throw err;
        },
      );
      this.entries.set(key, entry);
      e = entry;
    }
    const entry = e;
    entry.refs++;
    let released = false;
    return {
      promise: entry.promise,
      release: () => {
        if (released) return;
        released = true;
        entry.refs--;
        if (entry.refs === 0 && !entry.tile) {
          entry.controller.abort();
          if (this.entries.get(key) === entry) this.entries.delete(key);
        } else if (entry.refs === 0) {
          this.evict();
        }
      },
    };
  }

  /** Drop everything for a document (revision bump, close). */
  invalidate(docId: DocId): void {
    for (const [k, e] of this.entries) {
      if (k.startsWith(`${docId}|`)) {
        if (e.tile) {
          this.bytes -= e.tile.width * e.tile.height * 4;
          e.tile.bitmap.close();
        } else e.controller.abort();
        this.entries.delete(k);
      }
    }
  }

  get size(): number {
    return this.entries.size;
  }

  private evict(): void {
    for (const [k, e] of this.entries) {
      if (this.bytes <= this.maxBytes) break;
      if (!e.tile || e.refs > 0) continue;
      this.bytes -= e.tile.width * e.tile.height * 4;
      e.tile.bitmap.close();
      this.entries.delete(k);
    }
  }
}

/** Small LRU for page text. */
export class TextCache {
  private map = new Map<string, Promise<PageText>>();
  constructor(
    private source: TileSource,
    private max = 64,
  ) {}

  get(docId: DocId, page: number, revision: number): Promise<PageText> {
    const key = `${docId}|${revision}|${page}`;
    let p = this.map.get(key);
    if (p) {
      this.map.delete(key);
    } else {
      p = this.source.text(docId, page);
      p.catch(() => this.map.delete(key));
    }
    this.map.set(key, p);
    if (this.map.size > this.max) this.map.delete(this.map.keys().next().value as string);
    return p;
  }
}
