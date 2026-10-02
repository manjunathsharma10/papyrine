// Spike 0.1: bare Tauri 2 shell serving one static RGBA tile over papyrine://.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use tauri::http::{Response, StatusCode, header};

const TILE: usize = 512;

static START: OnceLock<Instant> = OnceLock::new();

fn epoch_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

/// Deterministic test pattern: diagonal gradient with a 64 px grid.
fn render_tile(page: u32, tx: u32, ty: u32) -> Vec<u8> {
    let mut buf = Vec::with_capacity(TILE * TILE * 4);
    let seed = (page.wrapping_mul(31) ^ tx.wrapping_mul(17) ^ ty.wrapping_mul(7)) as u8;
    for y in 0..TILE {
        for x in 0..TILE {
            let grid = x % 64 == 0 || y % 64 == 0;
            let (r, g, b) = if grid {
                (40, 40, 60)
            } else {
                ((x / 2) as u8, (y / 2) as u8, 200u8.wrapping_add(seed))
            };
            buf.extend_from_slice(&[r, g, b, 255]);
        }
    }
    buf
}

/// Parses `/tile/{page}/{x}/{y}`.
fn parse_tile_path(path: &str) -> Option<(u32, u32, u32)> {
    let mut it = path.trim_start_matches('/').split('/');
    if it.next()? != "tile" {
        return None;
    }
    let page = it.next()?.parse().ok()?;
    let x = it.next()?.parse().ok()?;
    let y = it.next()?.parse().ok()?;
    it.next().is_none().then_some((page, x, y))
}

#[tauri::command]
fn first_tile_painted(stage: &str) {
    let since_main = START.get().map_or(0, |s| s.elapsed().as_millis());
    if std::env::var_os("PAPYRINE_TRACE").is_some() {
        println!(
            "PAPYRINE_TRACE first_tile_painted stage={stage} epoch_ms={} since_main_ms={since_main}",
            epoch_ms()
        );
    }
}

#[tauri::command]
fn ui_error(message: String) {
    if std::env::var_os("PAPYRINE_TRACE").is_some() {
        println!("PAPYRINE_TRACE ui_error {message}");
    }
}

fn main() {
    START.get_or_init(Instant::now);
    if std::env::var_os("PAPYRINE_TRACE").is_some() {
        println!("PAPYRINE_TRACE main_start epoch_ms={}", epoch_ms());
    }
    tauri::Builder::default()
        .register_uri_scheme_protocol("papyrine", |_ctx, req| {
            let builder = Response::builder().header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*");
            match parse_tile_path(req.uri().path()) {
                Some((p, x, y)) => builder
                    .header(header::CONTENT_TYPE, "application/octet-stream")
                    .body(render_tile(p, x, y)),
                None => builder.status(StatusCode::NOT_FOUND).body(Vec::new()),
            }
            .expect("static response parts are valid")
        })
        .invoke_handler(tauri::generate_handler![first_tile_painted, ui_error])
        .run(tauri::generate_context!())
        .expect("error while running Papyrine");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_path_parsing() {
        assert_eq!(parse_tile_path("/tile/0/1/2"), Some((0, 1, 2)));
        assert_eq!(parse_tile_path("/tile/0/1"), None);
        assert_eq!(parse_tile_path("/tile/0/1/2/3"), None);
        assert_eq!(parse_tile_path("/other/0/1/2"), None);
        assert_eq!(parse_tile_path("/tile/a/1/2"), None);
    }

    #[test]
    fn tile_is_rgba_and_opaque() {
        let t = render_tile(0, 0, 0);
        assert_eq!(t.len(), TILE * TILE * 4);
        assert!(t.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
    }
}
