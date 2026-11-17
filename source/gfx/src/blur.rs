//! gfx-perf: ShadowGauss's blur (shadow.rs `gauss`) without its per-tap
//! closures and bounds checks: the red channel as floats with a zero
//! border, and each tap's integer and fractional position worked out once
//! per row and per column. Every value is computed with the same operations
//! in the same order as `shadow::gauss`, so the result is bit-identical
//! (checked by the test below); it takes about a sixth of the time.

/// where a tap at offset `off` (in texels, times `texel`) falls along an
/// axis of `n`: the padded index of the texel before it, and the fraction.
fn axis(n: usize, off: f32, texel: f32, pad: usize) -> (Vec<usize>, Vec<f32>) {
    let mut index = Vec::with_capacity(n);
    let mut frac = Vec::with_capacity(n);
    for v in 0..n {
        let p = v as f32 + off * texel;
        let p0 = p.floor();
        index.push((p0 as i64 + pad as i64) as usize);
        frac.push(p - p0);
    }
    (index, frac)
}

struct Tap {
    x: (Vec<usize>, Vec<f32>),
    y: (Vec<usize>, Vec<f32>),
}

impl Tap {
    fn new(w: usize, h: usize, ox: f32, oy: f32, texel: f32, pad: usize) -> Tap {
        Tap { x: axis(w, ox, texel, pad), y: axis(h, oy, texel, pad) }
    }
}

/// shadow::gauss, the same picture in window order, the same result.
pub fn gauss(win: &mut [u8], w: usize, h: usize, texel: f32, offsets: [f32; 4]) {
    let reach = offsets.iter().fold(0f32, |m, o| m.max(o.abs())) * texel;
    if !reach.is_finite() || reach > 64.0 || win.len() < w * h * 4 {
        return crate::shadow::gauss(win, w, h, texel, offsets);
    }
    let pad = reach.ceil() as usize + 2;
    let pw = w + 2 * pad;
    // the red channel, which neither pass writes until the last step: both
    // passes sample it from here
    let mut plane = vec![0f32; pw * (h + 2 * pad)];
    for y in 0..h {
        let row = &mut plane[(y + pad) * pw + pad..(y + pad) * pw + pad + w];
        for (x, v) in row.iter_mut().enumerate() {
            *v = win[(y * w + x) * 4] as f32 / 255.0;
        }
    }
    // one tap along a row: the bilinear sample at each column, computed
    // with the same operations in the same order as the reference
    let tap_row = |t: &Tap, y: usize, out: &mut [f32]| {
        let (yi, fy) = (t.y.0[y], t.y.1[y]);
        let r0 = &plane[yi * pw..(yi + 1) * pw];
        let r1 = &plane[(yi + 1) * pw..(yi + 2) * pw];
        for ((o, &xi), &fx) in out.iter_mut().zip(&t.x.0).zip(&t.x.1) {
            let top = r0[xi] * (1.0 - fx) + r0[xi + 1] * fx;
            let bot = r1[xi] * (1.0 - fx) + r1[xi + 1] * fx;
            *o = top * (1.0 - fy) + bot * fy;
        }
    };
    // (v.clamp(0, 1) * 255).round() as u8, without the library round: the
    // value is at most 255, so its fraction comes out exactly
    let q = |v: f32| {
        let v = v.clamp(0.0, 1.0) * 255.0;
        let i = v as u32;
        (i + (v - i as f32 >= 0.5) as u32) as u8
    };
    let [a, b, c, d] = offsets;
    // pass 1: unit 0 at the texel, unit 1 at (b, a), unit 2 at (d, c)
    let (t0, t1, t2) = (Tap::new(w, h, 0.0, 0.0, 1.0, pad), Tap::new(w, h, b, a, texel, pad), Tap::new(w, h, d, c, texel, pad));
    let c4 = 0x80 as f32 / 255.0;
    let c5 = 0x53 as f32 / 255.0;
    // gfx-perf: rows in parallel when asked; each pixel's value is computed
    // exactly as before, so the result does not depend on the split
    rows_parallel(win, w, h, |y, row| {
        let mut s = vec![0f32; 3 * w];
        let (s0, rest) = s.split_at_mut(w);
        let (s1, s2) = rest.split_at_mut(w);
        tap_row(&t0, y, s0);
        tap_row(&t1, y, s1);
        tap_row(&t2, y, s2);
        for x in 0..w {
            let s4 = s1[x] * c4 + s0[x] * (1.0 - c4);
            let s5 = s2[x] * c5 + s4 * (1.0 - c5);
            row[x * 4 + 1] = q(s5);
            row[x * 4 + 3] = 255;
        }
    });
    // pass 2: unit 0 at (c, b), unit 1 at (a, d), unit 2's green at the texel
    let (u0, u1) = (Tap::new(w, h, c, b, texel, pad), Tap::new(w, h, a, d, texel, pad));
    let k = ((0x40u32 << 8) / (0x40 + 0x33)) as f32 / 255.0;
    let kc = 0x99 as f32 / 255.0;
    rows_parallel(win, w, h, |y, row| {
        let mut s = vec![0f32; 2 * w];
        let (s0, s1) = s.split_at_mut(w);
        tap_row(&u0, y, s0);
        tap_row(&u1, y, s1);
        for x in 0..w {
            let t2g = row[x * 4 + 1] as f32 / 255.0;
            let s4 = s1[x] * k + s0[x] * (1.0 - k);
            let v = q(s4 * kc + t2g * (1.0 - kc));
            row[x * 4..x * 4 + 4].copy_from_slice(&[v, v, v, 255]);
        }
    });
}

/// runs `f(y, row)` over the picture's rows (RGBA, `w` wide), split over a
/// few threads when it is big enough to be worth it.
fn rows_parallel(win: &mut [u8], w: usize, h: usize, f: impl Fn(usize, &mut [u8]) + Sync) {
    let row_bytes = w * 4;
    // FATES_GFX_BLUR_THREADS=n: how many (default 1: on this thread; a
    // thread costs about as much to start as the rows it would take)
    static MOST: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let most = *MOST.get_or_init(|| crate::perf::var("FATES_GFX_BLUR_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(1).max(1));
    let threads = if w * h >= 128 * 128 { std::thread::available_parallelism().map_or(1, |n| n.get()).min(most) } else { 1 };
    let per = h.div_ceil(threads.max(1)).max(1);
    let rows = &mut win[..row_bytes * h];
    if threads <= 1 {
        for (y, row) in rows.chunks_mut(row_bytes).enumerate() {
            f(y, row);
        }
        return;
    }
    std::thread::scope(|scope| {
        for (k, part) in rows.chunks_mut(row_bytes * per).enumerate() {
            let f = &f;
            scope.spawn(move || {
                for (i, row) in part.chunks_mut(row_bytes).enumerate() {
                    f(k * per + i, row);
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn same_as_the_reference() {
        let mut seed = 0x1234_5678u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for (w, h, texel) in [(16usize, 16usize, 1.0f32), (64, 32, 1.0), (40, 24, 2.0), (33, 17, 3.0), (256, 256, 1.0), (200, 300, 2.0)] {
            let pic: Vec<u8> = (0..w * h * 4).map(|_| next() as u8).collect();
            let mut a = pic.clone();
            let mut b = pic.clone();
            crate::shadow::gauss(&mut a, w, h, texel, [-1.5, 0.5, 1.5, -0.5]);
            super::gauss(&mut b, w, h, texel, [-1.5, 0.5, 1.5, -0.5]);
            assert!(a == b, "{w}x{h} at {texel}");
        }
    }
}

#[cfg(test)]
mod bench {
    #[test]
    fn timing() {
        let (w, h) = (256usize, 256usize);
        let pic: Vec<u8> = (0..w * h * 4).map(|i| (i * 7 % 251) as u8).collect();
        for _ in 0..3 {
            let mut a = pic.clone();
            let t = std::time::Instant::now();
            crate::shadow::gauss(&mut a, w, h, 1.0, [-1.5, 0.5, 1.5, -0.5]);
            let r = t.elapsed();
            let mut b = pic.clone();
            let t = std::time::Instant::now();
            super::gauss(&mut b, w, h, 1.0, [-1.5, 0.5, 1.5, -0.5]);
            eprintln!("reference {r:?} fast {:?}", t.elapsed());
        }
    }
}
