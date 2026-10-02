import { memo, useContext, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { type DocId, type PageInfo, TILE_SIZE } from "../ipc/contract";
import { type SearchState } from "../store/app";
import { ViewportContext, type ViewportRect } from "./viewport";
import { getTileCache } from "./services";
import { TextLayer } from "./TextLayer";
import { CSS_PX_PER_PT, tileScale } from "./zoom";
import { reportFirstTile } from "./telemetry";

interface Props {
  docId: DocId;
  index: number;
  total: number;
  info: PageInfo;
  zoom: number;
  dpr: number;
  top: number;
  left: number;
  width: number;
  height: number;
  revision: number;
  search: SearchState | null;
}

interface TileSpec {
  tx: number;
  ty: number;
}

interface Layer {
  scale: number;
  tiles: TileSpec[];
}

const MARGIN_PX = 256;

function visibleTiles(vp: ViewportRect, p: { top: number; left: number; width: number; height: number }, scale: number, cssPerPt: number): TileSpec[] {
  // Intersection of the (padded) viewport with this page, in page-local CSS px.
  const x0 = Math.max(0, vp.left - MARGIN_PX - p.left);
  const y0 = Math.max(0, vp.top - MARGIN_PX - p.top);
  const x1 = Math.min(p.width, vp.left + vp.width + MARGIN_PX - p.left);
  const y1 = Math.min(p.height, vp.top + vp.height + MARGIN_PX - p.top);
  if (x1 <= x0 || y1 <= y0) return [];
  const px = (v: number) => (v / cssPerPt) * scale; // CSS px -> tile-scale device px
  const tx0 = Math.floor(px(x0) / TILE_SIZE);
  const tx1 = Math.floor((px(x1) - 1e-6) / TILE_SIZE);
  const ty0 = Math.floor(px(y0) / TILE_SIZE);
  const ty1 = Math.floor((px(y1) - 1e-6) / TILE_SIZE);
  const out: TileSpec[] = [];
  for (let ty = ty0; ty <= ty1; ty++) for (let tx = tx0; tx <= tx1; tx++) out.push({ tx, ty });
  return out;
}

const sameTiles = (a: TileSpec[], b: TileSpec[]) => a.length === b.length && a.every((t, i) => t.tx === b[i]?.tx && t.ty === b[i]?.ty);

export const PageView = memo(function PageView(props: Props) {
  const { docId, index, total, info, zoom, dpr, top, left, width, height, revision, search } = props;
  const { t } = useTranslation();
  const vp = useContext(ViewportContext);
  const cssPerPt = zoom * CSS_PX_PER_PT;
  const scale = tileScale(zoom, dpr);
  const geom = { top, left, width, height };
  const geomKey = `${top}|${left}|${width}|${height}`;

  const [layer, setLayer] = useState<Layer>(() => ({ scale, tiles: visibleTiles(vp.get(), geom, scale, cssPerPt) }));
  const [stale, setStale] = useState<Layer | null>(null);
  const ready = useRef(new Set<string>());
  const layerRef = useRef(layer);
  layerRef.current = layer;

  // Recompute on geometry/zoom change and on scroll.
  useLayoutEffect(() => {
    const compute = () => {
      const tiles = visibleTiles(vp.get(), geom, scale, cssPerPt);
      const cur = layerRef.current;
      if (cur.scale === scale && sameTiles(cur.tiles, tiles)) return;
      if (cur.scale !== scale) {
        ready.current = new Set();
        setStale(cur.tiles.length ? cur : null);
      }
      setLayer({ scale, tiles });
    };
    compute();
    return vp.subscribe(compute);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [vp, scale, dpr, cssPerPt, geomKey]);

  // Revision bump (edit): tiles must be re-requested.
  useEffect(() => {
    ready.current = new Set();
  }, [revision]);

  const onReady = (key: string) => {
    ready.current.add(key);
    const cur = layerRef.current;
    if (stale && cur.tiles.every((tl) => ready.current.has(`${tl.tx},${tl.ty}`))) setStale(null);
  };

  const label = t("viewer.pageLabel", { page: index + 1, total });
  const hasTiles = layer.tiles.length > 0;
  return (
    <div
      className="page"
      role="group"
      aria-label={label}
      data-page={index}
      style={{ top, left, width, height }}
    >
      {stale && <TileLayer key={`stale-${stale.scale}`} docId={docId} page={index} layer={stale} dpr={dpr} cssPerPt={cssPerPt} revision={revision} stale />}
      <TileLayer docId={docId} page={index} layer={layer} dpr={dpr} cssPerPt={cssPerPt} revision={revision} onReady={onReady} />
      {hasTiles && (
        <TextLayer docId={docId} page={index} revision={revision} widthPt={info.width} heightPt={info.height} cssScale={cssPerPt} search={search} />
      )}
    </div>
  );
});

function TileLayer({ docId, page, layer, dpr, cssPerPt, revision, onReady, stale }: {
  docId: DocId;
  page: number;
  layer: Layer;
  dpr: number;
  cssPerPt: number;
  revision: number;
  onReady?: (key: string) => void;
  stale?: boolean;
}) {
  // CSS size of one tile at this layer's (quantised) scale, stretched to the exact zoom.
  const cssPerDevice = cssPerPt / layer.scale;
  return (
    <div className={stale ? "tiles tiles-stale" : "tiles"} aria-hidden="true">
      {layer.tiles.map((tl) => (
        <TileCanvas
          key={`${tl.tx},${tl.ty}`}
          docId={docId}
          page={page}
          scale={layer.scale}
          tx={tl.tx}
          ty={tl.ty}
          revision={revision}
          cssPerDevice={cssPerDevice}
          dpr={dpr}
          onReady={onReady}
        />
      ))}
    </div>
  );
}

function TileCanvas({ docId, page, scale, tx, ty, revision, cssPerDevice, onReady }: {
  docId: DocId;
  page: number;
  scale: number;
  tx: number;
  ty: number;
  revision: number;
  cssPerDevice: number;
  dpr: number;
  onReady?: (key: string) => void;
}) {
  const ref = useRef<HTMLCanvasElement>(null);
  const [size, setSize] = useState<{ w: number; h: number } | null>(null);

  useEffect(() => {
    const cache = getTileCache();
    const req = { docId, page, scale, tx, ty, revision };
    const { promise, release } = cache.acquire(req);
    let live = true;
    promise
      .then((tile) => {
        const c = ref.current;
        if (!live || !c) return;
        c.width = tile.width;
        c.height = tile.height;
        c.getContext("2d")?.drawImage(tile.bitmap, 0, 0);
        setSize({ w: tile.width, h: tile.height });
        c.dataset.ready = "1";
        onReady?.(`${tx},${ty}`);
        reportFirstTile();
      })
      .catch(() => undefined);
    return () => {
      live = false;
      release();
    };
    // onReady identity changes every render by design; the tile identity is the dependency.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [docId, page, scale, tx, ty, revision]);

  return (
    <canvas
      ref={ref}
      className="tile"
      style={{
        left: tx * TILE_SIZE * cssPerDevice,
        top: ty * TILE_SIZE * cssPerDevice,
        width: size ? size.w * cssPerDevice : TILE_SIZE * cssPerDevice,
        height: size ? size.h * cssPerDevice : TILE_SIZE * cssPerDevice,
      }}
    />
  );
}
