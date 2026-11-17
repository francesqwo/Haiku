//! The few Pillow operations Haiku's picture scripts used (dev/glyphs/
//! make_glyphs.py's 4x upscale, dev/amiibo/make_icons.py's icon), done the
//! way Pillow does them: its separable resampling in 22-bit fixed point
//! (libImaging/Resample.c), its RGBA <-> premultiplied conversions
//! (Convert.c), ImageChops.multiply and alpha_composite. Reproducing them
//! keeps the residuals derive.rs adds near zero.

const PRECISION_BITS: i32 = 32 - 8 - 2;

#[derive(Clone, Copy)]
pub enum Filter {
    Bicubic,
    Lanczos,
}

fn bicubic(x: f64) -> f64 {
    let a = -0.5;
    let x = x.abs();
    if x < 1.0 {
        ((a + 2.0) * x - (a + 3.0)) * x * x + 1.0
    } else if x < 2.0 {
        (((x - 5.0) * x + 8.0) * x - 4.0) * a
    } else {
        0.0
    }
}

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        return 1.0;
    }
    let x = x * std::f64::consts::PI;
    x.sin() / x
}

fn lanczos(x: f64) -> f64 {
    if (-3.0..3.0).contains(&x) {
        sinc(x) * sinc(x / 3.0)
    } else {
        0.0
    }
}

/// per output pixel: the first input pixel, how many, and their weights.
fn coefficients(in_size: usize, out_size: usize, filter: Filter) -> (usize, Vec<(usize, usize)>, Vec<i32>) {
    let (support, f): (f64, fn(f64) -> f64) = match filter {
        Filter::Bicubic => (2.0, bicubic),
        Filter::Lanczos => (3.0, lanczos),
    };
    let scale = in_size as f64 / out_size as f64;
    let filterscale = scale.max(1.0);
    let support = support * filterscale;
    let ksize = support.ceil() as usize * 2 + 1;
    let mut bounds = Vec::with_capacity(out_size);
    let mut kk = vec![0f64; out_size * ksize];
    for xx in 0..out_size {
        let center = (xx as f64 + 0.5) * scale;
        let ss = 1.0 / filterscale;
        let xmin = ((center - support + 0.5) as i64).max(0) as usize;
        let xmax = ((center + support + 0.5) as i64).min(in_size as i64) as usize;
        let count = xmax - xmin;
        let k = &mut kk[xx * ksize..(xx + 1) * ksize];
        let mut ww = 0.0;
        for x in 0..count {
            let w = f((x as f64 + xmin as f64 - center + 0.5) * ss);
            k[x] = w;
            ww += w;
        }
        if ww != 0.0 {
            for x in 0..count {
                k[x] /= ww;
            }
        }
        bounds.push((xmin, count));
    }
    let fixed = kk
        .iter()
        .map(|&k| if k < 0.0 { (-0.5 + k * (1 << PRECISION_BITS) as f64) as i32 } else { (0.5 + k * (1 << PRECISION_BITS) as f64) as i32 })
        .collect();
    (ksize, bounds, fixed)
}

fn clip8(v: i32) -> u8 {
    (v >> PRECISION_BITS).clamp(0, 255) as u8
}

/// Pillow's resample of 4-channel 8-bit pixels (as they are: Image.resize
/// premultiplies an RGBA image first, see resize_rgba).
pub fn resample(px: &[u8], w: usize, h: usize, ow: usize, oh: usize, filter: Filter) -> Vec<u8> {
    let mut cur = px.to_vec();
    let mut cw = w;
    if ow != w {
        let (ksize, bounds, kk) = coefficients(w, ow, filter);
        let mut out = vec![0u8; ow * h * 4];
        for y in 0..h {
            for xx in 0..ow {
                let (xmin, count) = bounds[xx];
                let k = &kk[xx * ksize..];
                let mut ss = [1i32 << (PRECISION_BITS - 1); 4];
                for x in 0..count {
                    let at = (y * cw + x + xmin) * 4;
                    for c in 0..4 {
                        ss[c] += cur[at + c] as i32 * k[x];
                    }
                }
                for c in 0..4 {
                    out[(y * ow + xx) * 4 + c] = clip8(ss[c]);
                }
            }
        }
        cur = out;
        cw = ow;
    }
    if oh != h {
        let (ksize, bounds, kk) = coefficients(h, oh, filter);
        let mut out = vec![0u8; cw * oh * 4];
        for yy in 0..oh {
            let (ymin, count) = bounds[yy];
            let k = &kk[yy * ksize..];
            for x in 0..cw {
                let mut ss = [1i32 << (PRECISION_BITS - 1); 4];
                for y in 0..count {
                    let at = ((y + ymin) * cw + x) * 4;
                    for c in 0..4 {
                        ss[c] += cur[at + c] as i32 * k[y];
                    }
                }
                for c in 0..4 {
                    out[(yy * cw + x) * 4 + c] = clip8(ss[c]);
                }
            }
        }
        cur = out;
    }
    cur
}

fn muldiv255(a: u32, b: u32) -> u8 {
    let t = a * b + 128;
    (((t >> 8) + t) >> 8) as u8
}

/// RGBA -> RGBa (Convert.c rgbA2rgba).
pub fn premultiply(px: &[u8]) -> Vec<u8> {
    px.chunks_exact(4).flat_map(|p| [muldiv255(p[0] as u32, p[3] as u32), muldiv255(p[1] as u32, p[3] as u32), muldiv255(p[2] as u32, p[3] as u32), p[3]]).collect()
}

/// RGBa -> RGBA (Convert.c rgba2rgbA).
pub fn unpremultiply(px: &[u8]) -> Vec<u8> {
    px.chunks_exact(4)
        .flat_map(|p| {
            let a = p[3] as u32;
            if a == 0 || a == 255 {
                [p[0], p[1], p[2], p[3]]
            } else {
                let f = |c: u8| ((255 * c as u32) / a).min(255) as u8;
                [f(p[0]), f(p[1]), f(p[2]), p[3]]
            }
        })
        .collect()
}

/// Image.resize of an RGBA picture: premultiplied, resampled, back.
pub fn resize_rgba(px: &[u8], w: usize, h: usize, ow: usize, oh: usize, filter: Filter) -> Vec<u8> {
    unpremultiply(&resample(&premultiply(px), w, h, ow, oh, filter))
}

/// make_glyphs.py's up(): the art premultiplied by ImageChops.multiply,
/// resized x k with BICUBIC (which premultiplies once more), and divided
/// by its alpha again.
pub fn glyph_up(px: &[u8], w: usize, h: usize, k: usize) -> Vec<u8> {
    let pm: Vec<u8> = px.chunks_exact(4).flat_map(|p| [(p[0] as u32 * p[3] as u32 / 255) as u8, (p[1] as u32 * p[3] as u32 / 255) as u8, (p[2] as u32 * p[3] as u32 / 255) as u8, p[3]]).collect();
    let big = resize_rgba(&pm, w, h, w * k, h * k, Filter::Bicubic);
    big.chunks_exact(4)
        .flat_map(|p| {
            let a = p[3] as u32;
            if a == 0 {
                [0, 0, 0, 0]
            } else {
                let f = |c: u8| (c as u32 * 255 / a).min(255) as u8;
                [f(p[0]), f(p[1]), f(p[2]), p[3]]
            }
        })
        .collect()
}

/// Image.alpha_composite(dst, src) for one pixel (AlphaComposite.c).
pub fn alpha_composite(dst: [u8; 4], src: [u8; 4]) -> [u8; 4] {
    if src[3] == 0 {
        return dst;
    }
    const P: u32 = 7;
    let shift = |a: u32| ((a >> 8) + a) >> 8;
    let (sa, da) = (src[3] as u32, dst[3] as u32);
    let blend = da * (255 - sa);
    let outa255 = sa * 255 + blend;
    let coef1 = sa * 255 * 255 * (1 << P) / outa255;
    let coef2 = 255 * (1 << P) - coef1;
    let ch = |c: usize| (shift(src[c] as u32 * coef1 + dst[c] as u32 * coef2 + (0x80 << P)) >> P) as u8;
    [ch(0), ch(1), ch(2), shift(outa255 + 0x80) as u8]
}
