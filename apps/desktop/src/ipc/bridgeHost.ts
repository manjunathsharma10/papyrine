import { HostError, type DocumentInfo, type HostEvent, type Tile, type TileRequest } from "./contract";
import { RemoteHost, bitmapFromRgba, newRequestId, toHostError, type ExtraEvent } from "./tauriHost";

/**
 * Client of the dev bridge (`papyrine-bridge`, cargo feature `dev-bridge`): the same host
 * contract over a localhost WebSocket, so headless Chromium drives the real UI against the
 * real engine and renderer. Dev and test only; never reachable from a release build.
 *
 * Wire format: see `src-tauri/src/bridge.rs`.
 */
export class BridgeHost extends RemoteHost {
  private ws: WebSocket;
  private ready: Promise<void>;
  private pending = new Map<number, { resolve: (v: never) => void; reject: (e: Error) => void }>();
  private nextId = 1;

  constructor(url: string) {
    super();
    this.ws = new WebSocket(url);
    this.ws.binaryType = "arraybuffer";
    this.ready = new Promise<void>((resolve, reject) => {
      this.ws.addEventListener("open", () => resolve(), { once: true });
      this.ws.addEventListener("error", () => reject(new HostError("io", `cannot reach the dev bridge at ${url}`)), { once: true });
    });
    this.ws.addEventListener("message", (m) => this.onMessage(m));
    this.ws.addEventListener("close", () => {
      for (const p of this.pending.values()) p.reject(new HostError("io", "the dev bridge closed"));
      this.pending.clear();
    });
  }

  private onMessage(m: MessageEvent): void {
    if (typeof m.data !== "string") {
      // Tile frame: u32 id, u32 width, u32 height, RGBA.
      const buf = m.data as ArrayBuffer;
      const dv = new DataView(buf);
      const id = dv.getUint32(0, true);
      const w = dv.getUint32(4, true);
      const h = dv.getUint32(8, true);
      const p = this.pending.get(id);
      if (!p) return;
      this.pending.delete(id);
      bitmapFromRgba(new Uint8Array(buf, 12), w, h).then(
        (t) => (p.resolve as (v: Tile) => void)(t),
        (e) => p.reject(e),
      );
      return;
    }
    const msg = JSON.parse(m.data) as
      | { event: HostEvent | ExtraEvent }
      | { id: number; ok: true; result: unknown }
      | { id: number; ok: false; error: unknown };
    if ("event" in msg) {
      this.dispatch(msg.event);
      return;
    }
    const p = this.pending.get(msg.id);
    if (!p) return;
    this.pending.delete(msg.id);
    if (msg.ok) (p.resolve as (v: unknown) => void)(msg.result);
    else p.reject(toHostError(msg.error));
  }

  private async request<T>(send: (id: number) => string | ArrayBuffer): Promise<T> {
    await this.ready;
    const id = this.nextId++;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (v: never) => void, reject });
      this.ws.send(send(id));
    });
  }

  protected call<T>(method: string, args: unknown[]): Promise<T> {
    return this.request<T>((id) => JSON.stringify({ id, method, args }));
  }

  protected uploadBytes(name: string, data: ArrayBuffer): Promise<DocumentInfo> {
    const nameBytes = new TextEncoder().encode(name);
    return this.request<DocumentInfo>((id) => {
      const out = new Uint8Array(8 + nameBytes.length + data.byteLength);
      const dv = new DataView(out.buffer);
      dv.setUint32(0, id, true);
      dv.setUint32(4, nameBytes.length, true);
      out.set(nameBytes, 8);
      out.set(new Uint8Array(data), 8 + nameBytes.length);
      return out.buffer;
    });
  }

  protected async fetchTile(req: TileRequest, signal?: AbortSignal): Promise<Tile> {
    if (signal?.aborted) throw new HostError("cancelled", "tile cancelled");
    await this.ready;
    // The request id doubles as the cancel handle on the host.
    const id = this.nextId++;
    const result = new Promise<Tile>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (v: never) => void, reject });
    });
    signal?.addEventListener(
      "abort",
      () => {
        if (this.pending.delete(id)) {
          void this.call("cancelTile", [id]);
          // The host answers the cancelled request with an error frame; ignore it.
        }
        rejectLater(result);
      },
      { once: true },
    );
    this.ws.send(
      JSON.stringify({
        id,
        method: "getTile",
        args: [{ docId: req.docId, page: req.page, scale: req.scale, tx: req.tx, ty: req.ty }],
      }),
    );
    if (signal) {
      return Promise.race([
        result,
        new Promise<Tile>((_, reject) =>
          signal.addEventListener("abort", () => reject(new HostError("cancelled", "tile cancelled")), { once: true }),
        ),
      ]);
    }
    return result;
  }

  /** Dev-only commands (`dev.killEngine`, `dev.queueOpen`, ...). */
  dev<T = unknown>(method: string, ...args: unknown[]): Promise<T> {
    return this.call<T>(`dev.${method}`, args);
  }

  /** Test hook: a request id nobody uses (keeps `newRequestId` referenced for tree shaking). */
  static freshId = newRequestId;
}

function rejectLater(p: Promise<unknown>): void {
  p.catch(() => undefined);
}
