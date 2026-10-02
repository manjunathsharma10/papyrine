import { describe, expect, it } from "vitest";
import type { PageText, Tile, TileRequest } from "../ipc/contract";
import { TileCache, type TileSource } from "./tileSource";

const req = (tx: number): TileRequest => ({ docId: "d", page: 0, scale: 1, tx, ty: 0, revision: 0 });
const fakeTile = (): Tile => ({ bitmap: { close() {} } as unknown as ImageBitmap, width: 512, height: 512 });

function source() {
  const calls: { req: TileRequest; aborted: () => boolean; resolve: () => void }[] = [];
  const s: TileSource = {
    tile: (r, signal) =>
      new Promise<Tile>((resolve, reject) => {
        const call = { req: r, aborted: () => !!signal?.aborted, resolve: () => resolve(fakeTile()) };
        signal?.addEventListener("abort", () => reject(new Error("aborted")));
        calls.push(call);
      }),
    text: async () => ({ page: 0, runs: [] }) as PageText,
  };
  return { s, calls };
}

describe("TileCache", () => {
  it("de-duplicates concurrent requests", async () => {
    const { s, calls } = source();
    const c = new TileCache(s);
    const a = c.acquire(req(0));
    const b = c.acquire(req(0));
    expect(calls).toHaveLength(1);
    calls[0]!.resolve();
    await Promise.all([a.promise, b.promise]);
    expect(c.peek(req(0))).not.toBeNull();
  });

  it("aborts a request nobody waits for any more, but not while someone still does", async () => {
    const { s, calls } = source();
    const c = new TileCache(s);
    const a = c.acquire(req(1));
    const b = c.acquire(req(1));
    a.release();
    expect(calls[0]!.aborted()).toBe(false);
    b.release();
    expect(calls[0]!.aborted()).toBe(true);
    await expect(b.promise).rejects.toThrow();
    expect(c.size).toBe(0);
  });

  it("evicts least-recently-used tiles beyond the byte budget", async () => {
    const { s, calls } = source();
    const c = new TileCache(s, 2 * 512 * 512 * 4);
    const hs = [0, 1, 2].map((i) => c.acquire(req(i)));
    calls.forEach((x) => x.resolve());
    await Promise.all(hs.map((h) => h.promise));
    hs.forEach((h) => h.release());
    c.acquire(req(3)); // triggers nothing yet
    expect(c.peek(req(0))).toBeNull();
    expect(c.peek(req(2))).not.toBeNull();
  });
});
