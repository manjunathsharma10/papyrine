/// Deterministic payload for a command: sizes vary, ~1 in 120 is above the 1 MB fsync
/// threshold, odd seqs are compressible, even ones are not.
fn payload(seq: u64) -> Vec<u8> {
    let len = match seq % 120 {
        0 => 1_200_000 + (seq as usize % 7) * 60_000,
        r if r % 9 == 0 => 5_000 + (seq as usize * 37) % 20_000,
        _ => 64 + (seq as usize * 131) % 3_000,
    };
    if seq % 2 == 1 {
        (0..len).map(|i| ((i / 7) as u8).wrapping_add(seq as u8)).collect()
    } else {
        let mut x = seq.wrapping_mul(0x9E3779B97F4A7C15) | 1;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 8) as u8
            })
            .collect()
    }
}

