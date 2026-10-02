import { getHost } from "../ipc";
import { TextCache, TileCache, type TileSource, hostTileSource } from "./tileSource";

let source: TileSource | null = null;
let tiles: TileCache | null = null;
let texts: TextCache | null = null;

export function getTileSource(): TileSource {
  source ??= hostTileSource(getHost());
  return source;
}

/** Replace the tile source (tests, dev without a host). */
export function setTileSource(s: TileSource): void {
  source = s;
  tiles = null;
  texts = null;
}

export function getTileCache(): TileCache {
  tiles ??= new TileCache(getTileSource());
  return tiles;
}

export function getTextCache(): TextCache {
  texts ??= new TextCache(getTileSource());
  return texts;
}
