//! gfx-final: dev, the float uniforms a dumped draw's vertex program reads
//! (all outputs). usage: uniforms <gpu-*.bin from FATES_GFX_ORACLE_DIR>
fn main() {
    let path = std::env::args().nth(1).expect("a gpu-*.bin dump");
    let b = std::fs::read(path).unwrap();
    let w = |i: usize| u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
    let n = w(0) as usize;
    let mut o = 1 + n + 96 * 4 + 4;
    let entry = w(o + 1);
    o += 2;
    let program: Vec<u32> = (0..4096).map(|i| w(o + i)).collect();
    o += 4096;
    let desc: Vec<u32> = (0..128).map(|i| w(o + i)).collect();
    let (all, shared) = zakuro_gpu::native_hfr::placing_uniforms(&program, &desc, entry, 0x7F, 0);
    let list: Vec<usize> = (0..96).filter(|k| all >> k & 1 != 0).collect();
    println!("entry {entry}: reads c{list:?} (through an address register: {shared:032X})");
    if std::env::args().nth(2).is_some() {
        println!("{}", zakuro_gpu::native_hfr::program_glsl(&program, &desc, entry).unwrap_or_else(|e| e));
    }
}
