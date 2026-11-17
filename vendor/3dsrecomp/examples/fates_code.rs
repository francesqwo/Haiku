//! codemods: writes a title's decompressed code.bin and its exheader, the
//! files a code mod's patches are made against (dev/codemods/README).
//! usage: fates_code <rom> <code.bin out> [exheader.bin out]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        return Err("usage: fates_code <rom> <code.bin out> [exheader.bin out]".into());
    }
    let title = recomp3ds::rom::Title::load(&args[1])?;
    let code = title.code()?;
    std::fs::write(&args[2], &code)?;
    let ex = &title.exheader;
    println!("code.bin: {} bytes; text 0x{:08X} {} pages ({} bytes), rodata 0x{:08X} {} pages, data 0x{:08X} {} pages ({} bytes), bss {}",
        code.len(), ex.text.address, ex.text.num_pages, ex.text.size, ex.rodata.address, ex.rodata.num_pages, ex.data.address, ex.data.num_pages, ex.data.size, ex.bss_size);
    Ok(())
}
