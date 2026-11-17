//! builder: Haiku's icon, made at build time from its one source picture,
//! assets/icon/haiku.png (square, transparent; 256 px or more). Both
//! Haiku.exe (source/build.rs) and Haiku Setup.exe (builder/build.rs) use
//! this, so replacing that PNG and rebuilding changes every icon: the exe's
//! (Explorer, shortcuts), the window's and the taskbar's.
//!
//! The .ico holds every size Windows asks for, each a PNG, resized from the
//! source with Lanczos3 (the filter Haiku's window icon is resized with).

use std::path::Path;

/// the sizes in the .ico.
pub const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

/// writes ico from the square PNG at png.
pub fn write_ico(png: &Path, ico: &Path) -> Result<(), String> {
    let source = image::open(png).map_err(|e| format!("{}: {e}", png.display()))?.into_rgba8();
    if source.width() != source.height() {
        return Err(format!("{} is {}x{}; the icon has to be square", png.display(), source.width(), source.height()));
    }
    let mut images = Vec::new();
    for side in SIZES {
        let resized = if side == source.width() {
            source.clone()
        } else {
            image::imageops::resize(&source, side, side, image::imageops::FilterType::Lanczos3)
        };
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(resized)
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .map_err(|e| format!("encoding the {side}px icon: {e}"))?;
        images.push((side, bytes));
    }
    // ICONDIR, then an ICONDIRENTRY per image, then the images
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len();
    for (side, bytes) in &images {
        let dim = if *side >= 256 { 0 } else { *side as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += bytes.len();
    }
    for (_, bytes) in &images {
        out.extend_from_slice(bytes);
    }
    std::fs::write(ico, out).map_err(|e| format!("{}: {e}", ico.display()))
}
