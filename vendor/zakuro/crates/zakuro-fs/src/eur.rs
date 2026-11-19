//! eur: the DLC of the EUR Special Edition (CTR-P-BFZP).
//!
//! The EUR game asks for its DLC by its own unique id, 0x1795:
//! 0004008C00179500. The DLC players have for it is often the USA one,
//! 0004008C00179400 (a region-free CIA: same contents, a content index for
//! every region), whose text is only USA English (`m/@E`), while the EUR
//! game reads `m/@U` (English), `@F`, `@G`, `@I` or `@S`. So after the DLC is
//! installed next to an EUR game, [`adapt_dlc`]:
//!
//! - files the USA DLC under the title the EUR game asks for;
//! - copies its English text to each EUR language folder the DLC has none
//!   of (the DLC maps' text is then English in every language; an EUR DLC
//!   with text of its own keeps it).
//!
//! eurdlc: the genuine EUR DLC (0004008C00179500, CTR-M-BFXP) has every
//! content's text in English (UK), French, German, Italian and Spanish
//! (`@U`, `@F`, `@G`, `@I`, `@S`) and no `@E`: it is installed as it is,
//! under its own title, and nothing is copied over it.
//!
//! The USA game's data is never touched: this does nothing unless
//! data/install.txt names CTR-P-BFZP.

use std::path::Path;

use crate::datadir::{DLC_DIR, DLC_FILE, MANIFEST_FILE, ROMFS_DIR};
use crate::FsError;

pub const USA_DLC: u64 = 0x0004_008C_0017_9400;
pub const EUR_DLC: u64 = 0x0004_008C_0017_9500;
pub const EUR_PRODUCT: &str = "CTR-P-BFZP";
/// the EUR game's language folders: English (UK), French, German, Italian,
/// Spanish
pub const EUR_FOLDERS: [&str; 5] = ["@U", "@F", "@G", "@I", "@S"];

/// whether the game installed in out (data/) is the EUR Special Edition.
pub fn is_eur_data(out: &Path) -> bool {
    std::fs::read_to_string(out.join(MANIFEST_FILE)).is_ok_and(|text| text.lines().any(|line| line.trim() == format!("product {EUR_PRODUCT}")))
}

/// makes the DLC installed in out/dlc fit the EUR game, as the module says;
/// what it did, a line each.
///
/// eurdlc: what a DLC content holds is read from its index (romfs.tsv, the
/// files the install extracted), never from what else is in its folder, so
/// a genuine EUR DLC installed where the region-free one was before is never
/// given the old English text; and only the USA DLC's English (`@E`) is
/// copied: a genuine EUR DLC has no `@E` and keeps its own five languages.
pub fn adapt_dlc(out: &Path) -> Result<Vec<String>, FsError> {
    let mut done = Vec::new();
    if !is_eur_data(out) {
        return Ok(done);
    }
    let dlc = out.join(DLC_DIR);
    let (usa, eur) = (dlc.join(format!("{USA_DLC:016X}")), dlc.join(format!("{EUR_DLC:016X}")));
    if usa.join(DLC_FILE).is_file() {
        if !eur.exists() {
            std::fs::rename(&usa, &eur)?;
            let text = std::fs::read_to_string(eur.join(DLC_FILE))?;
            let text = text.replace(&format!("title {USA_DLC:016X}"), &format!("title {EUR_DLC:016X}"));
            // eurdlc: where it came from (dlc.txt's readers skip the line)
            let text = format!("{text}filed_from {USA_DLC:016X}\n");
            std::fs::write(eur.join(DLC_FILE), text)?;
            done.push(format!("the USA DLC {USA_DLC:016X} is filed as {EUR_DLC:016X}, the DLC the EUR game asks for"));
        } else {
            // eurdlc: the EUR game reads only its own
            done.push(format!(
                "the USA DLC {USA_DLC:016X} is left as it is: the EUR game reads its own DLC, {EUR_DLC:016X}, which is installed"
            ));
        }
    }
    if !eur.join(DLC_FILE).is_file() {
        return Ok(done);
    }
    let mut copied = 0;
    let mut own = [false; 5];
    for entry in std::fs::read_dir(&eur)?.flatten() {
        let romfs = entry.path().join(ROMFS_DIR);
        if !romfs.is_dir() {
            continue;
        }
        let mut list = match crate::datadir::read_index(&romfs) {
            Some(list) => list,
            None => crate::datadir::folder_list(&romfs)?,
        };
        let english = language_dirs(&list, "@E");
        for (i, folder) in EUR_FOLDERS.iter().enumerate() {
            if !language_dirs(&list, folder).is_empty() && english.is_empty() {
                own[i] = true;
            }
        }
        let mut added = Vec::new();
        for dir in &english {
            let parent = dir.strip_suffix("@E").unwrap_or(dir);
            for folder in EUR_FOLDERS {
                let target = format!("{parent}{folder}");
                if list.iter().any(|(path, _)| path.starts_with(&format!("{target}/"))) {
                    continue;
                }
                for (path, size) in list.iter().filter(|(path, _)| path.starts_with(&format!("{dir}/"))) {
                    let to = format!("{target}{}", &path[dir.len()..]);
                    let dest = romfs.join(&to);
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::copy(romfs.join(path), &dest)?;
                    added.push((to, *size));
                }
            }
        }
        if !added.is_empty() {
            copied += added.len();
            list.extend(added);
            list.sort();
            crate::datadir::write_index(&romfs, &list)?;
        }
    }
    if copied > 0 {
        done.push(format!("the DLC's English text (@E) copied to the EUR language folders it has none of: {copied} files"));
    }
    let names = ["English", "French", "German", "Italian", "Spanish"];
    let kept: Vec<&str> = names.iter().zip(own).filter(|(_, o)| *o).map(|(n, _)| *n).collect();
    if !kept.is_empty() {
        done.push(format!("the EUR DLC {EUR_DLC:016X} has its own text ({}): kept as it is", kept.join(", ")));
    }
    Ok(done)
}

/// eurdlc: why the DLC in the CIA at cia can't serve the game installed in
/// out, when it can't: the European DLC beside the USA game (which only asks
/// for the USA DLC). Haiku Setup says the same through its own check.
pub fn dlc_misfit(out: &Path, cia: &Path) -> Option<String> {
    let image = crate::RomImage::open(cia).ok()?;
    let title = crate::Cia::read(image.data()).ok()?.title_id;
    let usa = std::fs::read_to_string(out.join(MANIFEST_FILE)).is_ok_and(|text| text.lines().any(|line| line.trim() == "product CTR-P-BFZE"));
    (usa && title == EUR_DLC).then(|| {
        format!("this is the European DLC ({EUR_DLC:016X}), and the game is the USA Special Edition, which only reads the USA DLC ({USA_DLC:016X})")
    })
}

/// eurdlc: the folders named `name` (`@E`, `@U`...) of the files in a
/// content's index, as paths from its RomFS.
fn language_dirs(list: &[(String, u64)], name: &str) -> Vec<String> {
    let mut dirs: Vec<String> = list
        .iter()
        .filter_map(|(path, _)| {
            let at = path.find(&format!("/{name}/")).or_else(|| path.starts_with(&format!("{name}/")).then_some(usize::MAX))?;
            Some(if at == usize::MAX { name.to_owned() } else { path[..at + 1 + name.len()].to_owned() })
        })
        .collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_usa_dlc_is_filed_and_given_every_eur_language() {
        let out = std::env::temp_dir().join(format!("haiku-eur-dlc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out);
        let content = out.join("dlc/0004008C00179400/0001/romfs/local/m/@E");
        std::fs::create_dir_all(&content).unwrap();
        std::fs::write(content.join("A.bin.lz"), b"text").unwrap();
        std::fs::write(out.join("dlc/0004008C00179400/dlc.txt"), "title 0004008C00179400\nversion 272\ncontent 0001 00000001 4000 4\n").unwrap();
        // the USA game: nothing changes
        std::fs::write(out.join("install.txt"), "product CTR-P-BFZE\n").unwrap();
        assert!(adapt_dlc(&out).unwrap().is_empty());
        std::fs::write(out.join("install.txt"), "product CTR-P-BFZP\n").unwrap();
        assert_eq!(adapt_dlc(&out).unwrap().len(), 2);
        let eur = out.join("dlc/0004008C00179500");
        assert!(std::fs::read_to_string(eur.join("dlc.txt")).unwrap().starts_with("title 0004008C00179500"));
        for folder in EUR_FOLDERS {
            assert_eq!(std::fs::read(eur.join(format!("0001/romfs/local/m/{folder}/A.bin.lz"))).unwrap(), b"text");
        }
        assert!(std::fs::read_to_string(eur.join("0001/romfs.tsv")).unwrap().contains("local/m/@U/A.bin.lz"));
        // again: nothing left to do
        assert!(adapt_dlc(&out).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&out);
    }

    /// jpprep: a genuine EUR DLC (0004008C00179500) keeps its own text in
    /// each language: nothing is copied over it, nothing is refiled
    #[test]
    fn a_genuine_eur_dlc_keeps_its_own_text() {
        let out = std::env::temp_dir().join(format!("haiku-eur-dlc-own-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out);
        let eur = out.join("dlc/0004008C00179500");
        for folder in EUR_FOLDERS {
            let dir = eur.join(format!("0001/romfs/local/m/{folder}"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("A.bin.lz"), folder.as_bytes()).unwrap();
        }
        std::fs::write(eur.join("dlc.txt"), "title 0004008C00179500\nversion 272\ncontent 0001 00000001 4000 4\n").unwrap();
        std::fs::write(out.join("install.txt"), "product CTR-P-BFZP\n").unwrap();
        // eurdlc: nothing done, and said
        let said = adapt_dlc(&out).unwrap();
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].contains("own text (English, French, German, Italian, Spanish): kept as it is"), "{said:?}");
        for folder in EUR_FOLDERS {
            assert_eq!(std::fs::read(eur.join(format!("0001/romfs/local/m/{folder}/A.bin.lz"))).unwrap(), folder.as_bytes());
        }
        assert!(!out.join("dlc/0004008C00179400").exists());
        // a Japanese game: nothing at all
        std::fs::write(out.join("install.txt"), "product CTR-P-BFZJ\n").unwrap();
        assert!(adapt_dlc(&out).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&out);
    }

    /// eurdlc: the genuine EUR DLC where the region-free one was (refiled,
    /// its English copied to every EUR folder): even with an old file left
    /// beside it, nothing outside the content's index is copied or listed
    #[test]
    fn a_genuine_eur_dlc_over_the_region_free_one_gets_no_english() {
        let out = std::env::temp_dir().join(format!("haiku-eur-dlc-over-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("install.txt"), "product CTR-P-BFZP\n").unwrap();
        let eur = out.join("dlc/0004008C00179500");
        // what the region-free DLC left: an @E folder
        let old = eur.join("0001/romfs/local/m/@E");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("B.bin.lz"), b"english").unwrap();
        // the genuine DLC's files, with the index the install writes
        let mut list = Vec::new();
        for folder in EUR_FOLDERS {
            let dir = eur.join(format!("0001/romfs/local/m/{folder}"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("A.bin.lz"), folder.as_bytes()).unwrap();
            list.push((format!("local/m/{folder}/A.bin.lz"), 2));
        }
        crate::datadir::write_index(&eur.join("0001/romfs"), &list).unwrap();
        std::fs::write(eur.join("dlc.txt"), "title 0004008C00179500\nversion 0\ncontent 0001 00000001 4000 4\n").unwrap();
        let said = adapt_dlc(&out).unwrap();
        assert_eq!(said.len(), 1, "{said:?}");
        for folder in EUR_FOLDERS {
            assert!(!eur.join(format!("0001/romfs/local/m/{folder}/B.bin.lz")).exists());
        }
        assert!(!std::fs::read_to_string(eur.join("0001/romfs.tsv")).unwrap().contains("B.bin.lz"));
        // a USA DLC installed beside it stays the USA one's
        let usa = out.join("dlc/0004008C00179400");
        std::fs::create_dir_all(usa.join("0001/romfs/local/m/@E")).unwrap();
        std::fs::write(usa.join("dlc.txt"), "title 0004008C00179400\nversion 272\n").unwrap();
        let said = adapt_dlc(&out).unwrap();
        assert!(said[0].contains("left as it is"), "{said:?}");
        assert!(usa.join("dlc.txt").is_file());
        assert!(std::fs::read_to_string(eur.join("dlc.txt")).unwrap().contains("version 0"));
        let _ = std::fs::remove_dir_all(&out);
    }

    /// eurdlc: installing a DLC again replaces its folder
    #[test]
    fn a_dlc_installed_again_starts_empty() {
        let dir = std::env::temp_dir().join(format!("haiku-dlc-fresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("0001/romfs/local/m/@E")).unwrap();
        std::fs::write(dir.join("dlc.txt"), "title 0004008C00179500\n").unwrap();
        crate::datadir::fresh_dlc_dir(&dir).unwrap();
        assert!(!dir.exists());
        // a folder that isn't a DLC's is left alone
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("keep"), b"").unwrap();
        crate::datadir::fresh_dlc_dir(&dir).unwrap();
        assert!(dir.join("keep").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
