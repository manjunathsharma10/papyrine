#![no_main]

use libfuzzer_sys::fuzz_target;
use papyrine_content::{SerializeOptions, ops_equal, parse, serialize};

// Invariants: parsing never panics; spans stay in bounds; and
// parse -> serialize -> parse is stable (ops identical, no errors).
fuzz_target!(|data: &[u8]| {
    let first = parse(data);
    for op in &first.ops {
        assert!(op.span.start <= op.span.end && op.span.end <= data.len());
    }
    let bytes = serialize(&first.ops, &SerializeOptions::lossless());
    let second = parse(&bytes);
    assert!(
        ops_equal(&first.ops, &second.ops),
        "roundtrip mismatch\nfirst:  {:?}\nsecond: {:?}",
        first.ops,
        second.ops
    );
    // Default (rounded) formatting must at least parse cleanly and be a fixed point.
    let rounded = serialize(&first.ops, &SerializeOptions::default());
    let third = parse(&rounded);
    let again = serialize(&third.ops, &SerializeOptions::default());
    assert_eq!(rounded, again, "rounded output not a fixed point");
});
