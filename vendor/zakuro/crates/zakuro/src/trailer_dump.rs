//! trailer: dev-only switches for headless frame dumps, used by the trailer
//! pipeline (dev/trailer). None of them changes what the game does.
//!
//! - `ZAKURO_DUMP_FRAMES=range:A-B`: every frame from A to B (inclusive);
//! - `FATES_DUMP_SCREENS=top`: only the top screen is written;
//! - `FATES_DUMP_FAST=1`: PNGs written with fast compression (bigger files,
//!   several times quicker to write; the pixels are the same).

use std::path::Path;
use std::sync::OnceLock;

/// `range:A-B` (or several, `range:A-B+C-D`) in the dump spec.
pub fn range(spec: &str) -> Option<Ranges> {
    let list: Vec<(u64, u64)> = spec
        .trim()
        .strip_prefix("range:")?
        .split('+')
        .filter_map(|r| {
            let (a, b) = r.split_once('-')?;
            Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
        })
        .collect();
    (!list.is_empty()).then_some(Ranges(list))
}

/// frame ranges to dump, inclusive.
#[derive(Debug, PartialEq)]
pub struct Ranges(pub Vec<(u64, u64)>);

impl Ranges {
    pub fn contains(&self, frame: u64) -> bool {
        self.0.iter().any(|&(a, b)| frame >= a && frame <= b)
    }
}

/// whether the bottom screen is dumped (all but FATES_DUMP_SCREENS=top).
pub fn wants_bottom() -> bool {
    static TOP_ONLY: OnceLock<bool> = OnceLock::new();
    !*TOP_ONLY.get_or_init(|| std::env::var("FATES_DUMP_SCREENS").is_ok_and(|v| v.trim() == "top"))
}

/// FATES_LOCKSTEP=1 (dev, windowed at a 60 Hz frame rate): each game frame
/// is shown, however long the captures take, and the bottom screen's slide
/// moves by a 60th of a second a frame, so a window's own captures
/// (FATES_BOTTOM_SCRIPT `capture`) make an even 60 fps sequence.
pub fn lockstep() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("FATES_LOCKSTEP").is_ok_and(|v| v.trim() == "1"))
}

/// FATES_STAY_CLOAKED=1 (dev): the window stays cloaked (DWM), presented to
/// but never on the desktop, for capture runs that should not disturb it.
pub fn stay_cloaked() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("FATES_STAY_CLOAKED").is_ok_and(|v| v.trim() == "1"))
}

fn fast() -> bool {
    static FAST: OnceLock<bool> = OnceLock::new();
    *FAST.get_or_init(|| std::env::var("FATES_DUMP_FAST").is_ok_and(|v| v.trim() == "1"))
}

/// writes an RGBA picture as PNG, quickly when FATES_DUMP_FAST=1.
pub fn save(path: &Path, pixels: &[u8], width: u32, height: u32) -> image::ImageResult<()> {
    if !fast() {
        return image::save_buffer(path, pixels, width, height, image::ColorType::Rgba8);
    }
    use image::ImageEncoder;
    let file = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(path)?);
    let encoder = image::codecs::png::PngEncoder::new_with_quality(file, image::codecs::png::CompressionType::Fast, image::codecs::png::FilterType::Sub);
    encoder.write_image(pixels, width, height, image::ExtendedColorType::Rgba8)
}

#[cfg(test)]
mod tests {
    #[test]
    fn ranges_parse() {
        assert_eq!(super::range("range:10-20"), Some(super::Ranges(vec![(10, 20)])));
        assert!(super::range("range:10-20+30-40").is_some_and(|r| r.contains(35) && !r.contains(25)));
        assert_eq!(super::range("every:4"), None);
        assert_eq!(super::range("1,2,3"), None);
    }
}
