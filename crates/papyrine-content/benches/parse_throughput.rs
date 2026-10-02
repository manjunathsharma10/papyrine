//! Parse throughput on a generated ~10 MB content stream.
//! Run: `cargo bench -p papyrine-content --bench parse_throughput`

use papyrine_content::{Lexer, SerializeOptions, parse, serialize};
use std::hint::black_box;
use std::time::Instant;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn range(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn coord(&mut self) -> String {
        format!("{}.{:02}", self.range(600), self.range(100))
    }
}

fn generate(target: usize) -> Vec<u8> {
    let mut r = Rng(0x9E3779B97F4A7C15);
    let mut s = String::with_capacity(target + 4096);
    while s.len() < target {
        match r.range(4) {
            0 => {
                s.push_str("BT\n/F1 10.5 Tf\n");
                for _ in 0..(1 + r.range(8)) {
                    s.push_str(&format!(
                        "1 0 0 1 {} {} Tm\n[(Lorem ipsum \\(dolor\\)) ",
                        r.coord(),
                        r.coord()
                    ));
                    s.push_str(&format!(
                        "{} (sit) -{} (amet)] TJ\n",
                        r.range(200),
                        r.range(50)
                    ));
                }
                s.push_str("ET\n");
            }
            1 => {
                s.push_str(&format!(
                    "q {} {} {} rg {} {} m\n",
                    r.coord(),
                    r.coord(),
                    r.coord(),
                    r.coord(),
                    r.coord()
                ));
                for _ in 0..(2 + r.range(6)) {
                    s.push_str(&format!(
                        "{} {} {} {} {} {} c\n",
                        r.coord(),
                        r.coord(),
                        r.coord(),
                        r.coord(),
                        r.coord(),
                        r.coord()
                    ));
                }
                s.push_str("h f Q\n");
            }
            2 => s.push_str(&format!(
                "q {} 0 0 {} {} {} cm /Im{} Do Q\n/GS{} gs {} w {} {} {} {} re S\n",
                r.coord(),
                r.coord(),
                r.coord(),
                r.coord(),
                r.range(20),
                r.range(5),
                r.coord(),
                r.coord(),
                r.coord(),
                r.coord(),
                r.coord()
            )),
            _ => s.push_str(&format!(
                "/P <</MCID {}>> BDC <48656C6C6F> Tj -.5 {} Td (x) ' EMC\n",
                r.range(1000),
                r.coord()
            )),
        }
    }
    s.into_bytes()
}

fn time<T>(label: &str, bytes: usize, iters: u32, mut f: impl FnMut() -> T) {
    let mut best = f64::MAX;
    for _ in 0..iters {
        let t = Instant::now();
        black_box(f());
        best = best.min(t.elapsed().as_secs_f64());
    }
    println!(
        "{label:<22} {:>8.1} MB/s   ({:.1} ms best of {iters})",
        bytes as f64 / 1e6 / best,
        best * 1e3
    );
}

fn main() {
    let data = generate(10 * 1024 * 1024);
    let n = data.len();
    println!("input: {:.2} MB", n as f64 / 1e6);
    time("lex only", n, 5, || {
        let mut l = Lexer::new(&data);
        let mut c = 0usize;
        while l.next_token().is_some() {
            c += 1;
        }
        c
    });
    time("parse", n, 5, || parse(&data));
    let parsed = parse(&data);
    println!("ops: {}, errors: {}", parsed.ops.len(), parsed.errors.len());
    if let Some(e) = parsed.errors.first() {
        println!(
            "first error: {e}; context: {:?}",
            String::from_utf8_lossy(&data[e.offset.saturating_sub(40)..(e.offset + 40).min(n)])
        );
    }
    let opts = SerializeOptions::default();
    time("serialize", n, 5, || serialize(&parsed.ops, &opts));
    let out = serialize(&parsed.ops, &opts);
    println!(
        "serialized size: {:.2} MB ({:+.1}% vs input)",
        out.len() as f64 / 1e6,
        (out.len() as f64 / n as f64 - 1.0) * 100.0
    );
}
