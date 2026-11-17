//! randomizer: what the randomizer knows of the game, read from the game's
//! own files once per session: the characters (GameData's person table and
//! the story characters of the chapters' person files), the classes, the
//! weapons, the portraits' names and voice sets (FaceData) and the voice
//! clips the sound archive has.
//!
//! Layouts (offsets in an entry), found from the game's code and its data:
//! - Person (0x98): +0 flags (byte 0 bit 0 female), +8 PID, +0xC FID,
//!   +0x10 AID, +0x14 MPID (name), +0x18 MPID_H (help), +0x24 id,
//!   +0x2A parent, +0x2C class, +0x2E the class of the other tier,
//!   +0x32 level, +0x33 internal level, +0x38 personal bases[8],
//!   +0x40 growths[8], +0x48 max-stat modifiers[8], +0x60 weapon exp[8],
//!   +0x74 personal skills (normal, hard, lunatic), +0x7A birthday,
//!   +0x7C/+0x7E Heart Seal classes, +0x82 child id, +0x87 body type.
//! - Job (0x80): +0 flags (byte 0: bit 0 female, bit 3 promoted), +8 JID,
//!   +0x18 id, +0x1C bases[8], +0x34 max stats[8], +0x44 max weapon exp[8],
//!   +0x64/+0x66 promotions, +0x68/+0x6A base classes, +0x70 the same class
//!   for the other gender, +0x74 origin (1 Hoshido, 2 Nohr).
//! - Item (0x68): +8 IID, +0x14 id, +0x1A category (kind * 2 + Hoshido),
//!   +0x1C weapon exp needed, +0x2C price.
//! - FaceData (0x48): +0 FSID_ST_/FSID_BU_ name, +4 MPID, +8 voice set.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::rom::{lz11, messages, voice_labels, Bin, Rom};
use super::sprites::{sjis_text, Sprites};

pub const PERSON_SIZE: u32 = 0x98;
pub const JOB_SIZE: u32 = 0x80;
pub const ITEM_SIZE: u32 = 0x68;

#[derive(Clone, Debug)]
pub struct PersonRec {
    /// offset of the entry in GameData's data
    pub offset: u32,
    pub index: usize,
    pub pid: Vec<u8>,
    pub fid: Option<Vec<u8>>,
    pub female: bool,
    pub class: u16,
    pub class2: u16,
    pub level: u8,
    pub internal: u8,
    pub bases: [i8; 8],
    pub growths: [u8; 8],
    pub modifiers: [i8; 8],
    pub wexp: [u8; 8],
    pub pskills: [u8; 6],
    pub birthday: [u8; 2],
    pub reclass: [u16; 2],
    pub body: u8,
    pub parent: u16,
    pub child_id: u16,
}

#[derive(Clone, Debug)]
pub struct JobRec {
    pub id: u16,
    pub offset: u32,
    pub jid: Vec<u8>,
    pub name: String,
    pub female: bool,
    pub promoted: bool,
    pub flags: [u8; 8],
    pub bases: [i8; 8],
    pub max: [u8; 8],
    pub wexp: [u8; 8],
    pub promotions: [u16; 2],
    /// the base classes it promotes from
    pub from: [u16; 2],
    /// what it rides: 0 on foot, 3 a wyvern, 4 a horse (or a mechanist's
    /// mount, or an armored horse), 5 a pegasus or a kinshi
    pub ride: u8,
    pub other_gender: u16,
    pub origin: u8,
    /// rando3: the JID without JID_, as text (its map sprite's folder)
    pub key: String,
}

impl JobRec {
    pub fn uses(&self, kind: usize) -> bool {
        self.wexp.get(kind).is_some_and(|&v| v > 0)
    }

    /// the weapon kind the class uses most, for weapons it is handed.
    /// a staff only for a class that has nothing else (a weapon the unit
    /// can fight with comes first: a Strategist gets a tome).
    pub fn main_kind(&self) -> Option<usize> {
        (0..8)
            .filter(|&k| self.wexp[k] > 0 && k != 6)
            .max_by_key(|&k| (self.wexp[k], 8 - k))
            .or_else(|| self.uses(6).then_some(6))
    }

    pub fn staff_only(&self) -> bool {
        (0..8).all(|k| k == 6 || self.wexp[k] == 0) && self.wexp[6] > 0
    }
}

#[derive(Clone, Debug)]
pub struct ItemRec {
    pub id: u16,
    pub offset: u32,
    pub iid: Vec<u8>,
    pub name: String,
    pub category: u8,
    pub rank: u8,
    pub price: i32,
    pub flags: [u8; 8],
}

impl ItemRec {
    /// sword, lance, axe, dagger, bow, tome, staff, stone: the weapon exp
    /// index; None for what isn't a weapon.
    pub fn kind(&self) -> Option<usize> {
        (self.category < 16).then_some(self.category as usize / 2)
    }
}

/// a character the randomizer can put in another one's place: their
/// portrait (FID), model (AID), name, voice and, for the playable ones,
/// their own growths and skills.
#[derive(Clone, Debug)]
pub struct Identity {
    /// the FID without "FID_", Shift JIS: the name the game's talk scripts
    /// use ($Wm<key>|)
    pub key: Vec<u8>,
    pub fid: Vec<u8>,
    pub aid: Option<Vec<u8>>,
    pub mpid: Vec<u8>,
    pub mpid_h: Option<Vec<u8>>,
    pub female: bool,
    pub body: u8,
    pub voice: Option<String>,
    pub name: String,
    /// their entry in GameData's person table, when they are playable
    pub global: Option<usize>,
    pub child: bool,
    /// a story character, not playable
    pub story: bool,
    /// the class they have in their own place (GameData's, or a chapter's
    /// for a story character)
    pub class: u16,
    /// rando3: the AID without AID_, as text (their map sprite's head)
    pub aid_key: String,
    /// rando3: the key as text, for the talk scripts' $Wm<key>|
    pub key_text: String,
    /// rando3: the person index of their parent (children)
    pub parent: Option<usize>,
    /// lilith: the rules version that added them to the pool (1 for the
    /// first rounds' characters, 3 for Lilith, Kana, the amiibo heroes, the
    /// named captured bosses and the other story characters)
    pub since: u32,
}

pub struct Catalog {
    pub persons: Vec<PersonRec>,
    pub person_table: u32,
    pub jobs: Vec<JobRec>,
    pub items: Vec<ItemRec>,
    pub identities: Vec<Identity>,
    pub by_key: HashMap<Vec<u8>, usize>,
    /// voice set -> identity
    pub voice_owner: HashMap<String, usize>,
    pub voice_sets: Vec<String>,
    pub voices: HashSet<String>,
    /// voice set -> category -> clips
    pub voice_categories: HashMap<String, BTreeMap<String, Vec<String>>>,
    pub jid_index: HashMap<Vec<u8>, u16>,
    pub iid_index: HashMap<Vec<u8>, usize>,
    /// GameData's own pointer words (offsets in its data), to put back
    /// entries exactly as loaded
    pub gamedata: Bin,
    /// rando3: the map sprites there are
    pub sprites: Sprites,
    /// rando3: the other named characters of the game (bosses, villagers):
    /// English name -> gender, None when it varies
    pub extra_names: Vec<(String, Option<bool>)>,
    /// corrinswap: each portrait's hair colour (FaceData +0x1C, RGBA), by
    /// key, for a character shown in Corrin's place
    pub face_hair: HashMap<Vec<u8>, [u8; 4]>,
}

/// story characters, by their portraits, that take part when the story
/// option is on: Garon, Mikoto, Sumeragi, Iago, Hans, Zola, Arete, Kotaro,
/// Lilith (human form). Their talk scenes are throughout all three paths.
/// Anna, the last of the army's own characters in GameData's table
const LAST_PLAYABLE: usize = 71;

const STORY_KEYS: &[&str] = &["ガロン", "ミコト", "スメラギ", "マクベス", "ガンズ", "ゾーラ", "シェンメイ", "コタロウ", "リリス"];

/// lilith: the rules version that adds everyone below to the pool. A
/// catalog for an older version leaves them out, so a run saved before
/// keeps exactly the same people.
pub const SINCE_ALL: u32 = 3;

/// the rules version from which Layla leaves the pool again: she never
/// fights, so the game has no battle or map head of her own (the user's
/// call, 2026-11-16). A version 3 run keeps her, so its people stay the same.
pub const SINCE_NO_LAYLA: u32 = 4;

/// the talk-only characters in the pool of runs with the rules `version`
fn talk_only(version: u32) -> impl Iterator<Item = &'static (&'static str, bool, &'static str)> {
    TALK_ONLY.iter().filter(move |(k, _, _)| !(version >= SINCE_NO_LAYLA && *k == "ララ"))
}

/// lilith: characters the chapters' person files never name, who still
/// have a portrait (FaceData) and a model of their own (AID_<key> in the
/// asset tables): their gender (the model's body) and the class their
/// place is given (it only decides the tier, and the class of whoever is
/// shown there in a characters-only run). Lilith is the Northern Fortress's
/// servant of Chapters 1 and 3 (a robed staff user, so a Shrine Maiden);
/// Layla a villager.
const TALK_ONLY: &[(&str, bool, &str)] = &[("リリス", true, "巫女女"), ("ララ", true, "村人女")];

/// lilith: story characters of the chapters' person files with a portrait
/// and a model of their own, beyond STORY_KEYS: Kilma (the Ice Tribe's
/// chief), Anthony (Revelation) and Omozu (Chapter 3's border ninja)
const MORE_STORY_KEYS: &[&str] = &["クーリア", "ロンタオ", "モズ"];

/// whether a key is the given katakana name.
pub fn key_is(key: &[u8], text: &str) -> bool {
    key == sjis(text).as_slice()
}

pub fn sjis(text: &str) -> Vec<u8> {
    // the keys above, in Shift JIS: katakana only, which Shift JIS keeps in
    // 0x8340..0x8396 in the order of U+30A1..U+30F6
    let mut out = Vec::new();
    for c in text.chars() {
        let u = c as u32;
        if (0x30A1..=0x30F6).contains(&u) {
            let mut code = 0x8340 + (u - 0x30A1);
            if code >= 0x837F {
                code += 1; // Shift JIS skips 0x7F
            }
            out.push((code >> 8) as u8);
            out.push(code as u8);
        } else if u == 0x30FC {
            out.extend_from_slice(&[0x81, 0x5B]); // ー
        } else if u < 0x80 {
            out.push(u as u8);
        }
    }
    out
}

fn arr8<T: Copy + Default>(bytes: &[u8], f: impl Fn(u8) -> T) -> [T; 8] {
    let mut out = [T::default(); 8];
    for (i, b) in bytes.iter().take(8).enumerate() {
        out[i] = f(*b);
    }
    out
}

fn strip(prefix: &[u8], value: &[u8]) -> Option<Vec<u8>> {
    value.strip_prefix(prefix).map(|v| v.to_vec())
}

impl Catalog {
    /// the catalog for runs with the rules `version`: from SINCE_ALL on,
    /// everyone listed above is in the pool too.
    pub fn load(rom: &Rom, version: u32) -> Result<Catalog, String> {
        let started = std::time::Instant::now();
        let gamedata = rom
            .read("GameData/GameData.bin.lz")
            .and_then(|f| lz11(&f))
            .and_then(|f| Bin::parse(&f))
            .ok_or("GameData/GameData.bin.lz could not be read")?;
        let english = rom.read("m/@E/GameData.bin.lz").and_then(|f| lz11(&f)).map(|f| messages(&f)).unwrap_or_default();
        let name_of = |mpid: &[u8]| english.get(mpid).cloned().unwrap_or_else(|| String::from_utf8_lossy(mpid).into_owned());

        // the person table, its jobs and items: GameData's data starts
        // with the tables' pointers (GameData::Initialize)
        let person_table = gamedata.ptr(8).ok_or("no person table")?;
        let job_table = gamedata.ptr(12).ok_or("no class table")?;
        let item_table = gamedata.ptr(0x2C).ok_or("no item table")?;

        let mut persons = Vec::new();
        let count = gamedata.u16(person_table + 4) as usize;
        for index in 0..count {
            let at = person_table + 0x10 + index as u32 * PERSON_SIZE;
            let Some(pid) = gamedata.str_field(at + 8) else { continue };
            let mut pskills = [0u8; 6];
            pskills.copy_from_slice(gamedata.bytes(at + 0x74, 6));
            persons.push(PersonRec {
                offset: at,
                index,
                pid,
                fid: gamedata.str_field(at + 0xC),
                female: gamedata.u8(at) & 1 != 0,
                class: gamedata.u16(at + 0x2C),
                class2: gamedata.u16(at + 0x2E),
                level: gamedata.u8(at + 0x32),
                internal: gamedata.u8(at + 0x33),
                bases: arr8(gamedata.bytes(at + 0x38, 8), |b| b as i8),
                growths: arr8(gamedata.bytes(at + 0x40, 8), |b| b),
                modifiers: arr8(gamedata.bytes(at + 0x48, 8), |b| b as i8),
                wexp: arr8(gamedata.bytes(at + 0x60, 8), |b| b),
                pskills,
                birthday: [gamedata.u8(at + 0x7A), gamedata.u8(at + 0x7B)],
                reclass: [gamedata.u16(at + 0x7C), gamedata.u16(at + 0x7E)],
                body: gamedata.u8(at + 0x87),
                parent: gamedata.u16(at + 0x2A),
                child_id: gamedata.u16(at + 0x82),
            });
        }

        let mut jobs = Vec::new();
        let count = gamedata.u16(job_table + 6) as usize;
        for index in 0..count {
            let at = job_table + 8 + index as u32 * JOB_SIZE;
            let flags = arr8(gamedata.bytes(at, 8), |b| b);
            jobs.push(JobRec {
                id: gamedata.u16(at + 0x18),
                offset: at,
                jid: gamedata.str_field(at + 8).unwrap_or_default(),
                name: gamedata.str_field(at + 0x10).map(|m| name_of(&m)).unwrap_or_default(),
                female: flags[0] & 1 != 0,
                promoted: flags[0] & 8 != 0,
                flags,
                bases: arr8(gamedata.bytes(at + 0x1C, 8), |b| b as i8),
                max: arr8(gamedata.bytes(at + 0x34, 8), |b| b),
                wexp: arr8(gamedata.bytes(at + 0x44, 8), |b| b),
                promotions: [gamedata.u16(at + 0x64), gamedata.u16(at + 0x66)],
                from: [gamedata.u16(at + 0x68), gamedata.u16(at + 0x6A)],
                ride: {
                    let (b0, b1) = (gamedata.u8(at + 0x1A), gamedata.u8(at + 0x1B));
                    if b1 & 1 == 0 {
                        0
                    } else if b0 & 0x20 != 0 {
                        4
                    } else {
                        b0 & 7
                    }
                },
                other_gender: gamedata.u16(at + 0x70),
                origin: gamedata.u8(at + 0x74),
                key: gamedata.str_field(at + 8).map(|j| sjis_text(j.strip_prefix(b"JID_".as_slice()).unwrap_or(&j))).unwrap_or_default(),
            });
        }
        jobs.sort_by_key(|j| j.id);

        let mut items = Vec::new();
        let count = gamedata.u16(item_table + 6) as usize;
        for index in 0..count {
            let at = item_table + 8 + index as u32 * ITEM_SIZE;
            items.push(ItemRec {
                id: gamedata.u16(at + 0x14),
                offset: at,
                iid: gamedata.str_field(at + 8).unwrap_or_default(),
                name: gamedata.str_field(at + 0xC).map(|m| name_of(&m)).unwrap_or_default(),
                category: gamedata.u8(at + 0x1A),
                rank: gamedata.u8(at + 0x1C),
                price: gamedata.u32(at + 0x2C) as i32,
                flags: arr8(gamedata.bytes(at, 8), |b| b),
            });
        }

        // portraits: name and voice set by FID
        let mut faces: HashMap<Vec<u8>, (Vec<u8>, Option<String>)> = HashMap::new();
        let mut face_hair: HashMap<Vec<u8>, [u8; 4]> = HashMap::new();
        let face_file = rom.read("face/FaceData.bin.lz");
        log::debug!("randomizer: FaceData {:?} bytes, {:?} unpacked", face_file.as_ref().map(|f| f.len()), face_file.as_ref().and_then(|f| lz11(f)).map(|f| f.len()));
        if let Some(face) = face_file.and_then(|f| lz11(&f)).and_then(|f| Bin::parse(&f)) {
            log::debug!("randomizer: FaceData count {} pointers {}", face.u32(0), face.pointers.len());
            let count = face.u32(0) as u32;
            for index in 0..count {
                let at = 4 + index * 0x48;
                let Some(fsid) = face.str_field(at) else { continue };
                let Some(key) = strip(b"FSID_ST_", &fsid) else { continue };
                let mpid = face.str_field(at + 4).unwrap_or_default();
                let voice = face.str_field(at + 8).map(|v| String::from_utf8_lossy(&v).into_owned());
                let hair = face.bytes(at + 0x1C, 4);
                face_hair.insert(key.clone(), [hair[0], hair[1], hair[2], hair[3]]);
                faces.insert(key, (mpid, voice));
            }
        }

        // the voice clips the game has
        let mut voices: HashSet<String> = voice_labels(rom, "sound/IRON15_sound.bcsar").into_iter().collect();
        voices.retain(|v| !v.ends_with("_W") && !v.ends_with("_WS"));
        let mut prefixes: HashSet<String> = HashSet::new();
        for v in &voices {
            // VOICE_<SET>_...: sets have no digits; take every prefix
            let rest = &v[6..];
            let mut at = 0;
            while let Some(next) = rest[at..].find('_') {
                prefixes.insert(rest[..at + next].to_string());
                at += next + 1;
            }
        }
        let fix_voice = |set: Option<String>| -> Option<String> {
            let set = set?;
            if prefixes.contains(&set) {
                return Some(set);
            }
            // a set FaceData misspells (Ryoma's RTOUMA): the one clip set a
            // letter away
            let near: Vec<&String> = prefixes
                .iter()
                .filter(|p| p.len() == set.len() && p.bytes().zip(set.bytes()).filter(|(a, b)| a != b).count() == 1)
                .collect();
            (near.len() == 1).then(|| near[0].clone())
        };

        log::debug!("randomizer: {} persons, {} portraits, {} voice sets", persons.len(), faces.len(), prefixes.len());
        // the playable characters and their children
        let mut identities: Vec<Identity> = Vec::new();
        for p in &persons {
            let Some(fid) = &p.fid else { continue };
            let Some(key) = strip(b"FID_", fid) else { continue };
            // nobody, Corrin and Kana stay who they are; so do the captured
            // (PID_捕獲_, 捕 is 0x95DF) and bond units (PID_絆_, 0xE34A)
            // and so does everyone after Anna in the table: Lilith, the two
            // villagers and the amiibo heroes (Marth, Ike, Lucina, Robin),
            // whose Hero Battles and special classes are theirs alone
            if p.index < 3 || p.index > LAST_PLAYABLE || p.pid.starts_with(&sjis("PID_カンナ")) {
                continue;
            }
            if p.pid.starts_with(b"PID_\x95\xDF") || p.pid.starts_with(b"PID_\xE3\x4A") {
                continue;
            }
            let Some((mpid, voice)) = faces.get(&key) else { continue };
            let child = p.parent != 0 && p.child_id != 0xFFFF;
            let aid = gamedata.str_field(p.offset + 0x10);
            identities.push(Identity {
                aid_key: aid.as_ref().map(|a| sjis_text(a.strip_prefix(b"AID_".as_slice()).unwrap_or(a))).unwrap_or_default(),
                key_text: sjis_text(&key),
                parent: child.then_some(p.parent as usize),
                key: key.clone(),
                fid: fid.clone(),
                aid,
                mpid: mpid.clone(),
                mpid_h: gamedata.str_field(p.offset + 0x18),
                female: p.female,
                body: p.body,
                voice: fix_voice(voice.clone()),
                name: name_of(mpid),
                global: Some(p.index),
                child,
                story: false,
                class: p.class,
                since: 1,
            });
        }
        // story characters: from the chapters' person files
        let story: Vec<Vec<u8>> = STORY_KEYS.iter().map(|k| sjis(k)).collect();
        // lilith: the further story characters are looked for too (only used
        // from SINCE_ALL on)
        let more: Vec<Vec<u8>> = MORE_STORY_KEYS.iter().map(|k| sjis(k)).collect();
        let mut found: HashMap<Vec<u8>, (Option<Vec<u8>>, bool, u8, u16)> = HashMap::new();
        // rando3: every named character of the chapters, for the talk lines'
        // he and she (a boss named in a line is a candidate too)
        let mut extra: BTreeMap<String, Option<bool>> = BTreeMap::new();
        for path in rom.list("GameData/Person") {
            let Some(bin) = rom.read(&path).and_then(|f| lz11(&f)).and_then(|f| Bin::parse(&f)) else { continue };
            let count = bin.u16(4) as u32;
            for index in 0..count {
                let at = 0x10 + index * PERSON_SIZE;
                if let Some(name) = bin.str_field(at + 0x14).and_then(|m| english.get(&m).cloned()) {
                    let female = bin.u8(at) & 1 != 0;
                    extra.entry(name).and_modify(|g| if *g != Some(female) { *g = None }).or_insert(Some(female));
                }
                let Some(key) = bin.str_field(at + 0xC).and_then(|f| strip(b"FID_", &f)) else { continue };
                if (story.contains(&key) || more.contains(&key)) && !found.contains_key(&key) {
                    found.insert(key, (bin.str_field(at + 0x10), bin.u8(at) & 1 != 0, bin.u8(at + 0x87), bin.u16(at + 0x2C)));
                }
            }
        }
        // lilith: from SINCE_ALL on, the talk-only characters (Lilith) take
        // the model the asset tables have for their key, and the further
        // story characters follow
        let all = version >= SINCE_ALL;
        let jid = |text: &str| jobs.iter().find(|j| j.key == text).map_or(0, |j| j.id);
        if all {
            for (key, female, class) in talk_only(version) {
                let k = sjis(key);
                if !found.contains_key(&k) {
                    found.insert(k.clone(), (Some([b"AID_".as_slice(), &k].concat()), *female, 0, jid(class)));
                }
            }
        }
        let story_keys: Vec<Vec<u8>> = if all {
            story.iter().cloned().chain(talk_only(version).map(|(k, _, _)| sjis(k)).filter(|k| !story.contains(k))).chain(more.iter().cloned()).collect()
        } else {
            story.clone()
        };
        for key in &story_keys {
            let (Some((aid, female, body, class)), Some((mpid, voice))) = (found.get(key), faces.get(key)) else { continue };
            if aid.is_none() {
                continue;
            }
            let since = if STORY_KEYS.iter().any(|k| sjis(k) == *key) && !TALK_ONLY.iter().any(|(k, _, _)| sjis(k) == *key) { 1 } else { SINCE_ALL };
            let mut fid = b"FID_".to_vec();
            fid.extend_from_slice(key);
            let mut help = b"MPID_H_".to_vec();
            help.extend_from_slice(key);
            identities.push(Identity {
                aid_key: aid.as_ref().map(|a| sjis_text(a.strip_prefix(b"AID_".as_slice()).unwrap_or(a))).unwrap_or_default(),
                key_text: sjis_text(key),
                parent: None,
                key: key.clone(),
                fid,
                aid: aid.clone(),
                mpid: mpid.clone(),
                mpid_h: english.contains_key(&help).then_some(help),
                female: *female,
                body: if *body == 0xFF { 0 } else { *body },
                voice: fix_voice(voice.clone()),
                name: name_of(mpid),
                global: None,
                child: false,
                story: true,
                class: *class,
                since,
            });
        }
        // lilith: GameData's other characters with a portrait, a name and a
        // model of their own, from SINCE_ALL on: Kana (both; a run shows
        // one, see plan2.rs), the named captured bosses and the amiibo
        // heroes. Not the generic captured units (a class's portrait, no
        // model of their own), the bond units, the castle's shopkeepers and
        // Lilith's dragon (PID_CASTLE_), or Corrin.
        if all {
            let castle = sjis("PID_CASTLE_");
            for p in &persons {
                let Some(key) = p.fid.as_ref().and_then(|f| strip(b"FID_", f)) else { continue };
                if p.index < 3 || identities.iter().any(|i| i.global == Some(p.index) || i.key == key) || p.pid.starts_with(b"PID_\xE3\x4A") || p.pid.starts_with(&castle) {
                    continue;
                }
                let Some(aid) = gamedata.str_field(p.offset + 0x10) else { continue };
                let Some((mpid, voice)) = faces.get(&key) else { continue };
                let child = p.parent != 0 && p.child_id != 0xFFFF;
                identities.push(Identity {
                    aid_key: sjis_text(aid.strip_prefix(b"AID_".as_slice()).unwrap_or(&aid)),
                    key_text: sjis_text(&key),
                    parent: child.then_some(p.parent as usize),
                    key: key.clone(),
                    fid: p.fid.clone().unwrap_or_default(),
                    aid: Some(aid),
                    mpid: mpid.clone(),
                    mpid_h: gamedata.str_field(p.offset + 0x18),
                    female: p.female,
                    body: if p.body == 0xFF { 0 } else { p.body },
                    voice: fix_voice(voice.clone()),
                    name: name_of(mpid),
                    global: Some(p.index),
                    child,
                    story: false,
                    class: p.class,
                    since: SINCE_ALL,
                });
            }
        }

        let by_key = identities.iter().enumerate().map(|(i, id)| (id.key.clone(), i)).collect();
        // rando3: GameData's own named characters too (the captured bosses,
        // Kana, Lilith, the amiibo heroes)
        for p in &persons {
            if let Some(name) = gamedata.str_field(p.offset + 0x14).and_then(|m| english.get(&m).cloned()) {
                extra.entry(name).and_modify(|g| if *g != Some(p.female) { *g = None }).or_insert(Some(p.female));
            }
        }
        let pool_names: std::collections::HashSet<&str> = identities.iter().map(|i| i.name.as_str()).collect();
        let extra_names: Vec<(String, Option<bool>)> = extra
            .into_iter()
            .filter(|(n, _)| n.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && !n.contains(' ') && !pool_names.contains(n.as_str()))
            .collect();
        let sprites = Sprites::load(rom);
        let mut voice_owner = HashMap::new();
        for (i, id) in identities.iter().enumerate() {
            if let Some(v) = &id.voice {
                voice_owner.entry(v.clone()).or_insert(i);
            }
        }
        let mut voice_sets: Vec<String> = voice_owner.keys().cloned().collect();
        voice_sets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        let mut voice_categories: HashMap<String, BTreeMap<String, Vec<String>>> = HashMap::new();
        for v in &voices {
            let rest = &v[6..];
            let Some(set) = voice_sets.iter().find(|s| rest.starts_with(s.as_str()) && rest.as_bytes().get(s.len()) == Some(&b'_')) else { continue };
            let tail = &rest[set.len() + 1..];
            voice_categories.entry(set.clone()).or_default().entry(category(tail)).or_default().push(v.clone());
        }
        for sets in voice_categories.values_mut() {
            for clips in sets.values_mut() {
                clips.sort();
            }
        }
        let jid_index = jobs.iter().map(|j| (j.jid.clone(), j.id)).collect();
        let iid_index = items.iter().enumerate().map(|(i, it)| (it.iid.clone(), i)).collect();
        log::info!(
            "randomizer: read the game's data in {:.0?}: {} characters ({} story), {} classes, {} items, {} voice clips",
            started.elapsed(),
            identities.len(),
            identities.iter().filter(|i| i.story).count(),
            jobs.len(),
            items.len(),
            voices.len()
        );
        Ok(Catalog {
            persons,
            person_table,
            jobs,
            items,
            identities,
            by_key,
            voice_owner,
            voice_sets,
            voices,
            voice_categories,
            jid_index,
            iid_index,
            gamedata,
            sprites,
            extra_names,
            face_hair,
        })
    }

    pub fn job(&self, id: u16) -> Option<&JobRec> {
        self.jobs.get(id as usize).filter(|j| j.id == id)
    }
}

/// a clip's category: its name after the set without the number at the
/// end (B_ATTACK_02 -> B_ATTACK, EVT_S_00 -> EVT_S, MYROOM_SP_A stays).
pub fn category(tail: &str) -> String {
    let mut parts: Vec<&str> = tail.split('_').collect();
    while parts.len() > 1 && parts.last().is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) {
        parts.pop();
    }
    parts.join("_")
}

