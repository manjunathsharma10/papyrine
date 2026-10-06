import type { HostApi } from "./contract";
import { MockHost } from "./mock";
import { TauriHost, isTauri } from "./tauriHost";
import { BridgeHost } from "./bridgeHost";

let host: HostApi | null = null;

/**
 * The host in use. The Rust-backed implementation (Tauri commands + events +
 * papyrine:// tiles) registers itself with `setHost` next wave; until then the
 * in-memory MockHost serves Vite dev and E2E.
 */
export function getHost(): HostApi {
  if (!host) {
    const bridge = import.meta.env.DEV && typeof location !== "undefined" ? new URLSearchParams(location.search).get("bridge") : null;
    if (isTauri()) host = new TauriHost();
    // Dev only: `?bridge=ws://127.0.0.1:PORT/TOKEN` drives the real engine through papyrine-bridge.
    else if (bridge) host = new BridgeHost(bridge);
    else host = new MockHost();
  }
  return host;
}

export function setHost(h: HostApi): void {
  host = h;
}

export * from "./contract";
