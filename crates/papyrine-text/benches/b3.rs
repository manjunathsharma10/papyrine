//! `cargo bench -p papyrine-text`: prints B3 timings and asserts the gate
//! (1,000 pages < 3 s, first hit < 300 ms).
#[path = "../tests/common/mod.rs"]
mod common;
use common::*;
use papyrine_core::CancelToken;
use papyrine_text::*;
use std::time::Instant;

fn main() {
    let cases = [
        ("first hit page 0", vec![0usize, 400]),
        ("first hit page 700", vec![700]),
        ("no hits", vec![]),
    ];
    for (label, np) in cases {
        let pages = generate(1000, 2000, "zymurgy ﬁnale", &np);
        for (name, o) in [
            ("default", SearchOptions::default()),
            (
                "whole-word+diacritic",
                SearchOptions {
                    whole_word: true,
                    diacritic_insensitive: true,
                    ..Default::default()
                },
            ),
        ] {
            let mut best = f64::MAX;
            let mut first = None;
            for _ in 0..5 {
                let p = pages.clone();
                let t = Instant::now();
                let s = search_document(p, "Zymurgy Finale", &o, &CancelToken::new(), |_| {});
                best = best.min(t.elapsed().as_secs_f64());
                first = s.first_hit_at;
            }
            println!(
                "B3 [{label}] [{name}] best total {:.1} ms, first hit {:?}",
                best * 1e3,
                first
            );
            assert!(best < 3.0);
            assert!(first.is_none_or(|f| f.as_secs_f64() < 0.3));
        }
    }
}
