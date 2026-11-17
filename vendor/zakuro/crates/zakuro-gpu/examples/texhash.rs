//! sunglasses: a texture's Citra hashes, as texture packs name it.
//! usage: texhash <raw texture file> <width> <height> <pica format>
//! prints `rows <hash>` (Citra's legacy hash, what packs without
//! use_new_hash use) and `bytes <hash>`, as tex1_WxH_HASH_FMT wants them.
use zakuro_gpu::pack::{hash, Hashing};
use zakuro_gpu::texture::TextureFormat;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data = std::fs::read(&args[1]).expect("read");
    let (w, h, f): (u32, u32, u32) = (args[2].parse().unwrap(), args[3].parse().unwrap(), args[4].parse().unwrap());
    let format = TextureFormat::from_raw(f);
    let mut rows = Vec::new();
    for (name, hashing) in [("rows", Hashing::Rows), ("bytes", Hashing::Bytes)] {
        match hash(hashing, &data, format, w, h, &mut rows) {
            Some(value) => println!("{name} {value:016X}"),
            None => println!("{name} -"),
        }
    }
}
