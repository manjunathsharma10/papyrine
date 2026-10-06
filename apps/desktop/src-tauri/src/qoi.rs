//! Minimal QOI codec (https://qoiformat.org, MIT-licensed spec) for the L2 preview
//! cache: lossless, fast, and 3-6x smaller than raw RGBA on document pages.
//! Pixels are RGBA8; previews are opaque so alpha stays 255, but the codec handles
//! any alpha.

const OP_INDEX: u8 = 0x00;
const OP_DIFF: u8 = 0x40;
const OP_LUMA: u8 = 0x80;
const OP_RUN: u8 = 0xc0;
const OP_RGB: u8 = 0xfe;
const OP_RGBA: u8 = 0xff;
const END: [u8; 8] = [0, 0, 0, 0, 0, 0, 0, 1];
const HEADER: usize = 14;

fn hash(p: [u8; 4]) -> usize {
    (p[0] as usize * 3 + p[1] as usize * 5 + p[2] as usize * 7 + p[3] as usize * 11) % 64
}

/// Encode `rgba` (`width * height * 4` bytes). Returns `None` for inconsistent sizes.
pub fn encode(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || rgba.len() != width as usize * height as usize * 4 {
        return None;
    }
    let mut out = Vec::with_capacity(rgba.len() / 3 + HEADER + END.len());
    out.extend_from_slice(b"qoif");
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&[4, 0]);
    let mut index = [[0u8; 4]; 64];
    let mut prev = [0u8, 0, 0, 255];
    let mut run = 0u8;
    let n = rgba.len() / 4;
    for (i, px) in rgba.as_chunks::<4>().0.iter().enumerate() {
        let px = *px;
        if px == prev {
            run += 1;
            if run == 62 || i == n - 1 {
                out.push(OP_RUN | (run - 1));
                run = 0;
            }
            continue;
        }
        if run > 0 {
            out.push(OP_RUN | (run - 1));
            run = 0;
        }
        let h = hash(px);
        if index[h] == px {
            out.push(OP_INDEX | h as u8);
        } else {
            index[h] = px;
            if px[3] == prev[3] {
                let dr = px[0].wrapping_sub(prev[0]) as i8 as i32;
                let dg = px[1].wrapping_sub(prev[1]) as i8 as i32;
                let db = px[2].wrapping_sub(prev[2]) as i8 as i32;
                let (dr_dg, db_dg) = (dr - dg, db - dg);
                if (-2..=1).contains(&dr) && (-2..=1).contains(&dg) && (-2..=1).contains(&db) {
                    out.push(OP_DIFF | ((dr + 2) << 4 | (dg + 2) << 2 | (db + 2)) as u8);
                } else if (-32..=31).contains(&dg)
                    && (-8..=7).contains(&dr_dg)
                    && (-8..=7).contains(&db_dg)
                {
                    out.push(OP_LUMA | (dg + 32) as u8);
                    out.push(((dr_dg + 8) << 4 | (db_dg + 8)) as u8);
                } else {
                    out.extend_from_slice(&[OP_RGB, px[0], px[1], px[2]]);
                }
            } else {
                out.extend_from_slice(&[OP_RGBA, px[0], px[1], px[2], px[3]]);
            }
        }
        prev = px;
    }
    out.extend_from_slice(&END);
    Some(out)
}

/// Decode to `(width, height, rgba)`. Rejects malformed or absurdly large input.
pub fn decode(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    if data.len() < HEADER + END.len() || &data[..4] != b"qoif" {
        return None;
    }
    let width = u32::from_be_bytes(data[4..8].try_into().ok()?);
    let height = u32::from_be_bytes(data[8..12].try_into().ok()?);
    let total = (width as usize).checked_mul(height as usize)?;
    if width == 0 || height == 0 || total > (1 << 28) {
        return None;
    }
    let mut out = Vec::with_capacity(total * 4);
    let mut index = [[0u8; 4]; 64];
    let mut px = [0u8, 0, 0, 255];
    let mut p = HEADER;
    let end = data.len() - END.len();
    while out.len() < total * 4 {
        if p >= end {
            return None;
        }
        let b = data[p];
        p += 1;
        match b {
            OP_RGB => {
                px[..3].copy_from_slice(data.get(p..p + 3)?);
                p += 3;
            }
            OP_RGBA => {
                px.copy_from_slice(data.get(p..p + 4)?);
                p += 4;
            }
            _ => match b & 0xc0 {
                OP_INDEX => px = index[(b & 0x3f) as usize],
                OP_DIFF => {
                    px[0] = px[0].wrapping_add(((b >> 4) & 3).wrapping_sub(2));
                    px[1] = px[1].wrapping_add(((b >> 2) & 3).wrapping_sub(2));
                    px[2] = px[2].wrapping_add((b & 3).wrapping_sub(2));
                }
                OP_LUMA => {
                    let b2 = *data.get(p)?;
                    p += 1;
                    let dg = (b & 0x3f).wrapping_sub(32);
                    px[0] = px[0].wrapping_add(dg.wrapping_sub(8).wrapping_add(b2 >> 4));
                    px[1] = px[1].wrapping_add(dg);
                    px[2] = px[2].wrapping_add(dg.wrapping_sub(8).wrapping_add(b2 & 0x0f));
                }
                _ => {
                    let run = (b & 0x3f) as usize + 1;
                    if out.len() + run * 4 > total * 4 {
                        return None;
                    }
                    for _ in 0..run {
                        out.extend_from_slice(&px);
                    }
                    continue;
                }
            },
        }
        index[hash(px)] = px;
        out.extend_from_slice(&px);
    }
    Some((width, height, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) {
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                px.extend_from_slice(&f(x, y));
            }
        }
        let enc = encode(&px, w, h).unwrap();
        let (dw, dh, dec) = decode(&enc).unwrap();
        assert_eq!((dw, dh), (w, h));
        assert_eq!(dec, px);
    }

    #[test]
    fn solid_gradient_and_noise_round_trip() {
        roundtrip(64, 64, |_, _| [255, 255, 255, 255]);
        roundtrip(37, 19, |x, y| {
            [(x * 7) as u8, (y * 13) as u8, (x ^ y) as u8, 255]
        });
        roundtrip(50, 50, |x, y| {
            let v = (x.wrapping_mul(2654435761) ^ y.wrapping_mul(40503)) as u8;
            [
                v,
                v.wrapping_mul(3),
                v ^ 0x5a,
                if v.is_multiple_of(5) { 128 } else { 255 },
            ]
        });
        roundtrip(1, 1, |_, _| [1, 2, 3, 4]);
    }

    #[test]
    fn text_like_pages_compress() {
        let mut px = Vec::new();
        for y in 0..200u32 {
            for x in 0..300u32 {
                let ink = y % 12 < 2 && x % 40 < 25;
                px.extend_from_slice(if ink {
                    &[20, 20, 20, 255]
                } else {
                    &[255, 255, 255, 255]
                });
            }
        }
        let enc = encode(&px, 300, 200).unwrap();
        assert!(enc.len() * 8 < px.len(), "{} vs {}", enc.len(), px.len());
        assert_eq!(decode(&enc).unwrap().2, px);
    }

    #[test]
    fn malformed_input_is_rejected() {
        assert!(decode(b"nope").is_none());
        let mut enc = encode(&[0; 16], 2, 2).unwrap();
        enc.truncate(enc.len() - 12);
        assert!(decode(&enc).is_none());
        assert!(encode(&[0; 15], 2, 2).is_none());
        let mut huge = b"qoif".to_vec();
        huge.extend_from_slice(&u32::MAX.to_be_bytes());
        huge.extend_from_slice(&u32::MAX.to_be_bytes());
        huge.extend_from_slice(&[4, 0]);
        huge.extend_from_slice(&[0; 16]);
        assert!(decode(&huge).is_none());
    }
}
