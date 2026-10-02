//! ACCEPTANCE B3: 1,000 pages x ~2,000 chars searched in < 3 s, first hit < 300 ms.
//! Strict limits apply to optimized builds (`cargo test --release`, and the
//! `b3` bench); debug builds only check a 20x looser bound.
mod common;
use common::*;
use papyrine_core::CancelToken;
use papyrine_text::*;

fn slack() -> f64 {
    if cfg!(debug_assertions) { 20.0 } else { 1.0 }
}

fn timed(pages: Vec<PageInput>, q: &str, o: &SearchOptions) -> (SearchSummary, f64) {
    let t = std::time::Instant::now();
    let s = search_document(pages, q, o, &CancelToken::new(), |_| {});
    (s, t.elapsed().as_secs_f64())
}

#[test]
fn b3_thousand_pages() {
    // First needle on page 0 (fast path) and on page 700 (worst-ish case).
    for (needle_pages, label) in [(vec![0usize, 400], "early"), (vec![700usize], "late")] {
        let pages = generate(1000, 2000, "zymurgy ﬁnale", &needle_pages);
        let total_chars: usize = pages.iter().map(|p| p.text.chars.len()).sum();
        assert!(total_chars > 1_900_000);
        for (name, o) in [
            ("default", opts()),
            (
                "whole-word+diacritic",
                SearchOptions {
                    whole_word: true,
                    diacritic_insensitive: true,
                    ..opts()
                },
            ),
        ] {
            let (s, secs) = timed(pages.clone(), "Zymurgy Finale", &o);
            eprintln!(
                "B3 {label} {name}: {} pages, {} hits, total {:.1} ms, first hit {:?}",
                s.pages_searched,
                s.hits,
                secs * 1e3,
                s.first_hit_at
            );
            assert_eq!(s.pages_searched, 1000);
            assert_eq!(s.hits, needle_pages.len());
            assert!(secs < 3.0 * slack(), "total {secs}");
            let first = s.first_hit_at.unwrap().as_secs_f64();
            assert!(first < 0.3 * slack(), "first hit {first}");
        }
    }
}
