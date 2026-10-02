import type { HostApi } from "./contract";
import { MockHost } from "./mock";

let host: HostApi | null = null;

/**
 * The host in use. The Rust-backed implementation (Tauri commands + events +
 * papyrine:// tiles) registers itself with `setHost` next wave; until then the
 * in-memory MockHost serves Vite dev and E2E.
 */
export function getHost(): HostApi {
  if (!host) {
    if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) {
      console.warn("Papyrine: no Tauri host implementation registered yet; using the mock host.");
    }
    host = new MockHost();
  }
  return host;
}

export function setHost(h: HostApi): void {
  host = h;
}

export * from "./contract";
