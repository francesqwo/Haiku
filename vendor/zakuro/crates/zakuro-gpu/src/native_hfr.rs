//! gfx-hfr: what frame interpolation's matching knows about a vertex
//! program, for the native renderer (fates-gfx), which draws the game's own
//! vertex programs and places its pictures in between the way
//! `interpolation` does: only the float uniforms that place the vertices
//! move, see ShaderUnit::uniforms_feeding.

use crate::shader::{ShaderUnit, DESCRIPTOR_SIZE, PROGRAM_SIZE};

/// the output registers that place the vertices and those that hold the
/// textures' coordinates, one bit each, for a draw whose output map and
/// mask `registers` hold (PICA internal registers, indexed by word).
pub fn placing_outputs(registers: &[u32]) -> (u16, u16) {
    crate::raster::placing_outputs(registers)
}

/// the float uniforms a program run from `entry` places the vertices with
/// (bit k for uniform k), and those of them the textures' coordinates can
/// read through an address register too, see ShaderUnit::uniforms_feeding.
/// Decodes the program; callers cache it by program.
pub fn placing_uniforms(program: &[u32], descriptors: &[u32], entry: u32, outputs: u16, coordinates: u16) -> (u128, u128) {
    let mut unit = ShaderUnit::new();
    let n = program.len().min(PROGRAM_SIZE);
    unit.program[..n].copy_from_slice(&program[..n]);
    let n = descriptors.len().min(DESCRIPTOR_SIZE);
    unit.descriptors[..n].copy_from_slice(&descriptors[..n]);
    unit.entry_point = entry;
    unit.prepare();
    let (own, shared) = unit.uniforms_feeding(outputs, coordinates);
    (own | shared, shared)
}

/// gfxleft: the float uniforms a program run from `entry` reads straight
/// (not through an address register) into the output registers `outputs`.
pub fn uniforms_read_directly(program: &[u32], descriptors: &[u32], entry: u32, outputs: u16) -> u128 {
    let mut unit = ShaderUnit::new();
    let n = program.len().min(PROGRAM_SIZE);
    unit.program[..n].copy_from_slice(&program[..n]);
    let n = descriptors.len().min(DESCRIPTOR_SIZE);
    unit.descriptors[..n].copy_from_slice(&descriptors[..n]);
    unit.entry_point = entry;
    unit.prepare();
    unit.prepared().map_or(0, |p| p.uniforms_feeding(entry, outputs).0)
}

/// gfx-final: dev, a program from an entry point as GLSL (the hardware
/// renderer's translation), every output component in order, for reading
/// what a vertex program computes.
pub fn program_glsl(program: &[u32], descriptors: &[u32], entry: u32) -> Result<String, String> {
    let mut unit = ShaderUnit::new();
    let n = program.len().min(PROGRAM_SIZE);
    unit.program[..n].copy_from_slice(&program[..n]);
    let n = descriptors.len().min(DESCRIPTOR_SIZE);
    unit.descriptors[..n].copy_from_slice(&descriptors[..n]);
    unit.entry_point = entry;
    unit.prepare();
    let prepared = unit.prepared().ok_or("not prepared")?;
    let semantics: [u32; crate::shader::glsl::SEMANTICS] = std::array::from_fn(|i| i as u32);
    crate::shader::glsl::translate(&prepared, entry, &semantics)
}
