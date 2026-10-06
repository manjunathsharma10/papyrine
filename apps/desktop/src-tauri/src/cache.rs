//! Broker caches (ADR-009): L1 decoded tiles with a 48 MB soft cap, trimmed to the
//! tiles that were on screen when the user went idle; L2 previews/thumbnails kept
//! QOI-compressed under 16 MB.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::qoi;

pub const L1_SOFT_CAP: usize = 48 << 20;
pub const L2_CAP: usize = 16 << 20;
/// After this long without a tile request, L1 shrinks to the "visible" set.
pub const IDLE_TRIM: Duration = Duration::from_secs(5);
/// Tiles requested within this window of the last request count as visible.
const VISIBLE_WINDOW: Duration = Duration::from_millis(1500);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub doc: u64,
    pub page: u32,
    pub bucket: i32,
    pub tx: u32,
    pub ty: u32,
}

/// One rendered tile: opaque RGBA, `width * height * 4` bytes.
#[derive(Debug)]
pub struct TileData {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

struct Entry {
    tile: Arc<TileData>,
    touched: Instant,
    /// Monotonic touch counter: LRU order independent of clock resolution.
    tick: u64,
}

#[derive(Default)]
pub struct L1 {
    map: HashMap<TileKey, Entry>,
    bytes: usize,
    tick: u64,
    last_request: Option<Instant>,
}

impl L1 {
    pub fn get(&mut self, k: &TileKey) -> Option<Arc<TileData>> {
        let now = Instant::now();
        self.tick += 1;
        self.last_request = Some(now);
        let tick = self.tick;
        let e = self.map.get_mut(k)?;
        e.touched = now;
        e.tick = tick;
        Some(e.tile.clone())
    }

    /// Note a request that missed (it still counts as "recent activity").
    pub fn note_request(&mut self) {
        self.last_request = Some(Instant::now());
    }

    pub fn put(&mut self, k: TileKey, tile: Arc<TileData>) {
        self.tick += 1;
        self.bytes += tile.rgba.len();
        let old = self.map.insert(
            k,
            Entry {
                tile,
                touched: Instant::now(),
                tick: self.tick,
            },
        );
        if let Some(o) = old {
            self.bytes -= o.tile.rgba.len();
        }
        self.enforce_cap();
    }

    fn enforce_cap(&mut self) {
        if self.bytes <= L1_SOFT_CAP {
            return;
        }
        // Soft cap: evict least recently used, but never below one screenful.
        let mut order: Vec<(u64, TileKey)> = self.map.iter().map(|(k, e)| (e.tick, *k)).collect();
        order.sort_unstable_by_key(|o| o.0);
        for (_, k) in order {
            if self.bytes <= L1_SOFT_CAP {
                break;
            }
            self.remove(&k);
        }
    }

    fn remove(&mut self, k: &TileKey) {
        if let Some(e) = self.map.remove(k) {
            self.bytes -= e.tile.rgba.len();
        }
    }

    pub fn invalidate_pages(&mut self, doc: u64, pages: &[u32]) {
        let drop: Vec<TileKey> = self
            .map
            .keys()
            .filter(|k| k.doc == doc && pages.contains(&k.page))
            .copied()
            .collect();
        for k in drop {
            self.remove(&k);
        }
    }

    pub fn invalidate_doc(&mut self, doc: u64) {
        let drop: Vec<TileKey> = self.map.keys().filter(|k| k.doc == doc).copied().collect();
        for k in drop {
            self.remove(&k);
        }
    }

    /// Idle trim: if nothing was requested for [`IDLE_TRIM`], keep only the tiles touched
    /// in the last burst (the ones on screen). Returns bytes freed.
    pub fn trim_if_idle(&mut self, now: Instant) -> usize {
        let Some(last) = self.last_request else {
            return 0;
        };
        if now.duration_since(last) < IDLE_TRIM {
            return 0;
        }
        let before = self.bytes;
        let keep_after = last.checked_sub(VISIBLE_WINDOW).unwrap_or(last);
        let drop: Vec<TileKey> = self
            .map
            .iter()
            .filter(|(_, e)| e.touched < keep_after)
            .map(|(k, _)| *k)
            .collect();
        for k in drop {
            self.remove(&k);
        }
        if self.map.is_empty() {
            self.map.shrink_to_fit();
        }
        before - self.bytes
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PreviewKey {
    pub doc: u64,
    pub page: u32,
    pub edge: u32,
}

struct PreviewEntry {
    qoi: Arc<Vec<u8>>,
    tick: u64,
}

#[derive(Default)]
pub struct L2 {
    map: HashMap<PreviewKey, PreviewEntry>,
    bytes: usize,
    tick: u64,
}

impl L2 {
    pub fn get(&mut self, k: &PreviewKey) -> Option<TileData> {
        self.tick += 1;
        let tick = self.tick;
        let e = self.map.get_mut(k)?;
        e.tick = tick;
        let (width, height, rgba) = qoi::decode(&e.qoi)?;
        Some(TileData {
            width,
            height,
            rgba,
        })
    }

    pub fn put(&mut self, k: PreviewKey, t: &TileData) {
        let Some(enc) = qoi::encode(&t.rgba, t.width, t.height) else {
            return;
        };
        self.tick += 1;
        self.bytes += enc.len();
        if let Some(o) = self.map.insert(
            k,
            PreviewEntry {
                qoi: Arc::new(enc),
                tick: self.tick,
            },
        ) {
            self.bytes -= o.qoi.len();
        }
        while self.bytes > L2_CAP {
            let Some((&victim, _)) = self.map.iter().min_by_key(|(_, e)| e.tick) else {
                break;
            };
            if let Some(o) = self.map.remove(&victim) {
                self.bytes -= o.qoi.len();
            }
        }
    }

    pub fn invalidate_pages(&mut self, doc: u64, pages: &[u32]) {
        let drop: Vec<PreviewKey> = self
            .map
            .keys()
            .filter(|k| k.doc == doc && pages.contains(&k.page))
            .copied()
            .collect();
        for k in drop {
            if let Some(o) = self.map.remove(&k) {
                self.bytes -= o.qoi.len();
            }
        }
    }

    pub fn invalidate_doc(&mut self, doc: u64) {
        let drop: Vec<PreviewKey> = self.map.keys().filter(|k| k.doc == doc).copied().collect();
        for k in drop {
            if let Some(o) = self.map.remove(&k) {
                self.bytes -= o.qoi.len();
            }
        }
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(n: usize) -> Arc<TileData> {
        Arc::new(TileData {
            width: 1,
            height: 1,
            rgba: vec![7; n],
        })
    }
    fn key(page: u32) -> TileKey {
        TileKey {
            doc: 1,
            page,
            bucket: 0,
            tx: 0,
            ty: 0,
        }
    }

    #[test]
    fn l1_soft_cap_evicts_least_recently_used() {
        let mut c = L1::default();
        let n = 1 << 20;
        for p in 0..60 {
            c.put(key(p), tile(n));
            if p == 10 {
                assert!(c.get(&key(0)).is_some());
            }
        }
        assert!(c.bytes() <= L1_SOFT_CAP);
        assert!(c.get(&key(59)).is_some());
        assert!(c.get(&key(1)).is_none(), "oldest untouched tile is gone");
    }

    #[test]
    fn l1_invalidation_is_per_page() {
        let mut c = L1::default();
        c.put(key(0), tile(10));
        c.put(key(1), tile(10));
        c.invalidate_pages(1, &[0]);
        assert!(c.get(&key(0)).is_none());
        assert!(c.get(&key(1)).is_some());
        c.invalidate_doc(1);
        assert!(c.is_empty());
        assert_eq!(c.bytes(), 0);
    }

    #[test]
    fn idle_trim_keeps_only_the_last_burst() {
        let mut c = L1::default();
        for p in 0..10 {
            c.put(key(p), tile(100));
        }
        std::thread::sleep(Duration::from_millis(1700));
        for p in 8..10 {
            assert!(c.get(&key(p)).is_some());
        }
        // Not idle yet.
        assert_eq!(c.trim_if_idle(Instant::now()), 0);
        let freed = c.trim_if_idle(Instant::now() + IDLE_TRIM + Duration::from_secs(1));
        assert_eq!(freed, 800);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn l2_is_compressed_and_capped() {
        let mut c = L2::default();
        let t = TileData {
            width: 200,
            height: 200,
            rgba: vec![255; 200 * 200 * 4],
        };
        c.put(
            PreviewKey {
                doc: 1,
                page: 0,
                edge: 200,
            },
            &t,
        );
        assert!(c.bytes() < 2000);
        let back = c
            .get(&PreviewKey {
                doc: 1,
                page: 0,
                edge: 200,
            })
            .unwrap();
        assert_eq!(back.rgba, t.rgba);
        // Incompressible noise: the cap bounds the total.
        let mut x = 12345u32;
        let noise: Vec<u8> = (0..512 * 512 * 4)
            .map(|_| {
                x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                (x >> 24) as u8
            })
            .collect();
        for p in 0..40 {
            c.put(
                PreviewKey {
                    doc: 1,
                    page: p,
                    edge: 512,
                },
                &TileData {
                    width: 512,
                    height: 512,
                    rgba: noise.clone(),
                },
            );
        }
        assert!(c.bytes() <= L2_CAP);
    }
}
