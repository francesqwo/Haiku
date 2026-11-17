//! gfx: what fates-gfx's native H3D renderer shares with the hardware
//! renderer, so the two shade a mesh the same way: the PICA fragment stages
//! (raster.frag), the vertex program's interface (shade.vert), and the
//! layouts of the blocks they read, built from a fixed-function
//! configuration given as parameter words. Nothing here reads a command
//! list or emulated memory.

use crate::lighting::Lighting;

/// raster.frag, the fragment stages, as SPIR-V (no depth writes build).
pub const RASTER_FRAG: &[u8] = include_bytes!("../shaders/raster.frag.spv");
/// gfx-final: raster.frag built with WRITES_DEPTH, which works the PICA's
/// depth map out itself (scale and offset from the Draw block's flags),
/// for a depth map a Vulkan viewport can't hold (a polygon offset past 0)
pub const RASTER_FRAG_DEPTH: &[u8] = include_bytes!("../shaders/raster_depth.frag.spv");
/// shade.vert's source: the vertex program interpreted per vertex.
pub const SHADE_VERT: &str = include_str!("../shaders/shade.vert");

/// a varying the program does not write, and one it writes as zero, as
/// hardware.rs numbers them
pub const MISSING_SEMANTIC: u32 = u32::MAX;
pub const ZERO_SEMANTIC: u32 = u32::MAX - 1;

/// where shade.vert finds each varying of a program whose output map and
/// mask `regs` hold.
pub fn semantics(regs: &[u32]) -> [u32; 24] {
    crate::raster::output_semantics(regs)
}

const STAGE_REGISTERS: [usize; 6] = [0x0C0, 0x0C8, 0x0D0, 0x0D8, 0x0F0, 0x0F8];
const TEXTURE_UNIT_BASES: [usize; 3] = [0x081, 0x091, 0x099];
const PROCTEX_CONFIG: usize = 0x0A8;
/// words of raster.frag's Draw block.
pub const DRAW_WORDS: usize = 12 * 4 + 4 + 3 * 4 + 4 + 328 + 8 + 4;
/// vec2 entries of its Tables block: 24 light tables of 256, the
/// procedural texture's maps and colours, the fog's 128.
pub const TABLE_ENTRIES: usize = 24 * 256 + 3 * 128 + 4 * 256 + 128;
pub const FOG_TABLE: usize = 24 * 256 + 3 * 128 + 4 * 256;

/// raster.frag's Draw block for a configuration: `enabled` the texture
/// units bound, the depth map's scale and offset.
pub fn draw_block(r: &[u32], enabled: u32, depth_scale: f32, depth_offset: f32) -> Vec<u32> {
    let mut words = Vec::with_capacity(DRAW_WORDS);
    for base in STAGE_REGISTERS {
        words.extend([r[base], r[base + 1], r[base + 2], r[base + 3], r[base + 4], 0, 0, 0]);
    }
    let texture_config = (r[0x080] & !7) | enabled;
    words.extend([r[0x0E0], r[0x0FD], r[0x104], texture_config]);
    for base in TEXTURE_UNIT_BASES {
        words.extend([r[base + 2], r[base], 0, 0]);
    }
    let lighting = Lighting::read(r);
    // a shaded draw: depth from the fragment's z
    words.extend([2, lighting.is_some() as u32, depth_scale.to_bits(), depth_offset.to_bits()]);
    match lighting {
        Some(lighting) => lighting.pack(&mut words),
        None => words.resize(DRAW_WORDS - 8 - 4, 0),
    }
    words.extend((0..6).map(|k| r[PROCTEX_CONFIG + k]));
    words.extend([0, 0]);
    words.extend([r[0x0E1], 0, 0, 0]);
    words
}

/// whether a configuration lights its fragments, for raster.frag's
/// specialization.
pub fn lights(r: &[u32]) -> bool {
    Lighting::read(r).is_some()
}
