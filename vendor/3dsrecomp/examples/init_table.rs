//! eur2: the static initializers the C++ init table lists, for a dump
//! (Image::init_table): `init_table <rom>` prints how many and the first.
use recomp3ds::{image, rom::Title};

fn main() {
    let path = std::env::args().nth(1).expect("usage: init_table <rom>");
    let title = Title::load(&path).expect("could not load the dump");
    let image = image::Image::from_title(&title).expect("could not read the code");
    let targets = image.init_table();
    println!("{} initializers, first {:08X?}", targets.len(), &targets[..targets.len().min(4)]);
}
