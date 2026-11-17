//! randomizer: a talk line's text, made to fit who is shown (rando3).
//!
//! Two things are rewritten in one pass over the line (UTF-16, as the
//! game's TalkExpander leaves it), outside the line's `$` codes:
//!
//! - **names**: "Leo" said anywhere becomes the name of whoever is shown in
//!   Leo's place (whole words only, one pass so two swapped names never
//!   undo each other);
//! - **gendered words that refer to a swapped character** whose shown
//!   character has the other gender: he/she, him/her, his/her(s),
//!   himself/herself, brother/sister, son/daughter, father/mother (dad/mom,
//!   papa/mama), prince/princess, king/queen, lord/lady, milord/milady,
//!   sir/ma'am, man/woman, boy/girl, husband/wife and the like.
//!
//! The second is done from the line's own structure, never blindly. The
//! talk script says who is on screen (`$Wm<key>|<slot>`), who speaks
//! (`$Ws<key>|`) and who leaves (`$Wd`), so each word knows its speaker,
//! the listener (the one other character on screen, or the one who spoke
//! last) and the characters named before it. A gendered word is changed
//! only when exactly one character can be the one it means:
//!
//! - a title or a family word right before a name ("Prince Leo", "brother
//!   Leo") means that name;
//! - a family word means a relative of its owner ("my" the speaker, "your"
//!   the listener, "Leo's" Leo; with no owner, as in "dear little brother",
//!   the speaker): siblings, parents and children come from a curated table
//!   of the families (the royal siblings, Garon, Sumeragi, Mikoto, Arete,
//!   the twins) and from the game's own parent of each child; the relative
//!   has to be on screen or named in this line or the one before, except a
//!   parent the table names alone (a Nohr royal's "Father" is Garon);
//! - an address (milord, milady, sir, ma'am, my lord, my lady) means the
//!   listener;
//! - "I'm a ... man" means the speaker, "you're a ... man" the listener;
//! - he/she and the like mean the one character of that gender named or on
//!   screen (not the speaker, not the listener) in this line and the one
//!   before it; with two candidates, or none, nothing changes.
//!
//! Corrin is never changed by this (the game says Corrin's words with its
//! own `$G` code), but counts as a candidate of either gender, so a word
//! that could mean Corrin stays as written. Every change is logged.

use std::collections::HashMap;

/// a character as the text knows them.
#[derive(Clone, Debug, Default)]
pub struct TextPerson {
    /// the English name the game says (its own, not whoever is shown)
    pub name: Vec<u16>,
    /// the talk script's key ($Wm<key>|), UTF-16
    pub key: Vec<u16>,
    pub female: bool,
    pub child: bool,
    /// who is shown in this place: their name and gender
    pub shown_name: Vec<u16>,
    pub shown_female: bool,
    pub siblings: Vec<usize>,
    pub parents: Vec<usize>,
    pub children: Vec<usize>,
    /// gender unknown (Corrin, whose words the game genders itself)
    pub either: bool,
}

impl TextPerson {
    fn swapped(&self) -> bool {
        !self.either && self.shown_female != self.female
    }
    fn renamed(&self) -> bool {
        self.shown_name != self.name
    }
}

#[derive(Default)]
pub struct Cast {
    pub people: Vec<TextPerson>,
    pub by_key: HashMap<Vec<u16>, usize>,
    /// English names (theirs) -> person
    pub by_name: HashMap<Vec<u16>, usize>,
    /// other named characters of the game (bosses, villagers' names): their
    /// gender, None when it varies (Kana)
    pub extra: HashMap<Vec<u16>, Option<bool>>,
    /// Corrin, the "username" window and $Nu
    pub corrin: Option<usize>,
    /// corrinswap: the name said for Corrin's ($Nu) when someone else is
    /// shown in Corrin's place
    pub corrin_name: Option<Vec<u16>>,
}

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

impl Cast {
    pub fn add(&mut self, p: TextPerson) -> usize {
        let i = self.people.len();
        if !p.key.is_empty() {
            self.by_key.insert(p.key.clone(), i);
        }
        if !p.name.is_empty() && p.name[0] >= b'A' as u16 && p.name[0] <= b'Z' as u16 {
            self.by_name.entry(p.name.clone()).or_insert(i);
        }
        self.people.push(p);
        i
    }

    /// whether anything in a line could change at all
    pub fn any_change(&self) -> bool {
        self.corrin_name.is_some() || self.people.iter().any(|p| p.renamed() || p.swapped())
    }
}

// -- words -------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    /// he/she, him/her(obj), his/her(det), his/hers(pron), himself/herself
    Pronoun,
    /// a relative: the relation and the word's own gender
    Kin(Rel),
    /// a title said before a name
    Title,
    /// an address to the listener
    Address,
    /// man/woman, boy/girl...
    Noun,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Rel {
    Sibling,
    Parent,
    Child,
    Spouse,
    /// rando4: uncle, niece...: only from the patterns (I'm your aunt)
    Other,
}

struct Entry {
    word: &'static str,
    other: &'static str,
    female: bool,
    kind: Kind,
}

const fn e(word: &'static str, other: &'static str, female: bool, kind: Kind) -> Entry {
    Entry { word, other, female, kind }
}

/// the gendered words, lower case, each with its other gender's word. "her"
/// and "his" are decided by what follows (see swap_word).
const WORDS: &[Entry] = &[
    e("he", "she", false, Kind::Pronoun),
    e("she", "he", true, Kind::Pronoun),
    e("him", "her", false, Kind::Pronoun),
    e("her", "him", true, Kind::Pronoun),
    e("his", "her", false, Kind::Pronoun),
    e("hers", "his", true, Kind::Pronoun),
    e("himself", "herself", false, Kind::Pronoun),
    e("herself", "himself", true, Kind::Pronoun),
    e("brother", "sister", false, Kind::Kin(Rel::Sibling)),
    e("sister", "brother", true, Kind::Kin(Rel::Sibling)),
    e("bro", "sis", false, Kind::Kin(Rel::Sibling)),
    e("sis", "bro", true, Kind::Kin(Rel::Sibling)),
    e("father", "mother", false, Kind::Kin(Rel::Parent)),
    e("mother", "father", true, Kind::Kin(Rel::Parent)),
    e("dad", "mom", false, Kind::Kin(Rel::Parent)),
    e("mom", "dad", true, Kind::Kin(Rel::Parent)),
    e("daddy", "mommy", false, Kind::Kin(Rel::Parent)),
    e("mommy", "daddy", true, Kind::Kin(Rel::Parent)),
    e("papa", "mama", false, Kind::Kin(Rel::Parent)),
    e("mama", "papa", true, Kind::Kin(Rel::Parent)),
    e("son", "daughter", false, Kind::Kin(Rel::Child)),
    e("daughter", "son", true, Kind::Kin(Rel::Child)),
    e("husband", "wife", false, Kind::Kin(Rel::Spouse)),
    e("wife", "husband", true, Kind::Kin(Rel::Spouse)),
    e("prince", "princess", false, Kind::Title),
    e("princess", "prince", true, Kind::Title),
    e("king", "queen", false, Kind::Title),
    e("queen", "king", true, Kind::Title),
    e("lord", "lady", false, Kind::Title),
    e("lady", "lord", true, Kind::Title),
    e("milord", "milady", false, Kind::Address),
    e("milady", "milord", true, Kind::Address),
    e("sir", "ma'am", false, Kind::Address),
    e("ma'am", "sir", true, Kind::Address),
    e("man", "woman", false, Kind::Noun),
    e("woman", "man", true, Kind::Noun),
    e("boy", "girl", false, Kind::Noun),
    e("girl", "boy", true, Kind::Noun),
    e("lad", "lass", false, Kind::Noun),
    e("lass", "lad", true, Kind::Noun),
    e("gentleman", "lady", false, Kind::Noun),
    // rando4: more words (found by the audit, dev/randomizer/textcheck)
    e("guy", "gal", false, Kind::Noun),
    e("gal", "guy", true, Kind::Noun),
    e("fella", "gal", false, Kind::Noun),
    e("uncle", "aunt", false, Kind::Kin(Rel::Other)),
    e("aunt", "uncle", true, Kind::Kin(Rel::Other)),
    e("nephew", "niece", false, Kind::Kin(Rel::Other)),
    e("niece", "nephew", true, Kind::Kin(Rel::Other)),
    e("miss", "mister", true, Kind::Title),
    e("mister", "miss", false, Kind::Title),
    e("madam", "sir", true, Kind::Address),
];

/// words after which "her" is the object (him), not a determiner (his)
const AFTER_OBJECT: &[&str] = &[
    "to", "and", "or", "but", "in", "on", "at", "with", "for", "from", "about", "of", "as", "so", "that", "than", "too", "again",
    "back", "out", "up", "down", "off", "away", "now", "then", "here", "there", "alone", "once", "more", "if", "when", "because",
    "before", "after", "until", "into", "over", "through", "the", "a", "an", "this", "some", "any", "my", "your", "his", "her",
    "our", "their", "what", "how", "why", "where", "who", "go", "be", "is", "was", "are", "were", "will", "would", "could",
    "should", "can", "just", "very", "much", "well", "anymore", "either", "yet", "already", "soon", "today", "tonight",
    "tomorrow", "yesterday", "anything", "everything", "nothing", "something", "all", "both", "either", "by", "like",
    "around", "too", "first", "last", "enough", "since", "while", "though", "although", "yourself", "myself", "i", "you",
    "we", "they", "it", "he", "she", "him", "them", "us", "me", "do", "does", "did", "has", "have", "had", "not", "know",
];

/// words between an owner (my, Leo's) and a family word
const KIN_ADJECTIVES: &[&str] = &[
    "dear", "little", "big", "older", "younger", "elder", "baby", "twin", "beloved", "sweet", "darling", "own", "only", "kid",
    "half", "lovely", "precious", "foolish", "silly", "poor", "favorite", "dearest", "sweetest", "wonderful", "lost", "adoptive",
    "real", "true", "late", "beautiful", "noble", "brave", "honored", "esteemed", "royal",
];

/// a word of the line: its place in the text
#[derive(Clone, Copy, Debug)]
struct Tok {
    start: usize,
    end: usize,
    word: bool,
    /// a $Nu (Corrin's name) code
    corrin: bool,
}

fn is_letter(u: u16) -> bool {
    (b'a' as u16..=b'z' as u16).contains(&u) || (b'A' as u16..=b'Z' as u16).contains(&u) || (0xC0..=0x24F).contains(&u)
}

fn lower(s: &[u16]) -> String {
    String::from_utf16_lossy(s).to_lowercase()
}

/// the $ code at i: where it ends
fn code_end(text: &[u16], i: usize) -> usize {
    let c1 = text.get(i + 1).copied().unwrap_or(0);
    let c2 = text.get(i + 2).copied().unwrap_or(0);
    let to_bar = || text[i..].iter().position(|&u| u == b'|' as u16).map_or(text.len(), |p| i + p + 1);
    match c1 {
        0x6B | 0x70 | 0x61 => i + 2,               // $k $p $a
        0x4E | 0x74 => i + 3,                      // $Nu $t0
        0x57 if c2 != 0x6D && c2 != 0x73 => i + 3, // $Wa $Wd $Wc ...
        _ => to_bar(),
    }
    .min(text.len())
}

/// what a line's codes say about the scene at a point
#[derive(Clone, Default)]
struct Scene {
    /// window slot -> who is in it
    windows: Vec<(u16, usize)>,
    speaker: Option<usize>,
    last_other: Option<usize>,
}

impl Scene {
    fn on_screen(&self) -> Vec<usize> {
        self.windows.iter().map(|w| w.1).collect()
    }
    /// the one spoken to: the one other character on screen, else whoever
    /// spoke before (and is still on screen)
    fn listener(&self) -> Option<usize> {
        let sp = self.speaker?;
        let others: Vec<usize> = self.windows.iter().map(|w| w.1).filter(|&p| p != sp).collect();
        if others.len() == 1 {
            return Some(others[0]);
        }
        self.last_other.filter(|p| others.contains(p))
    }
}

/// a name said, or a character on screen, that a word could mean
#[derive(Clone, Copy)]
enum Who {
    Person(usize),
    Other(Option<bool>),
}

struct Mention {
    who: Who,
    /// rando4: the token it is
    tok: usize,
    utterance: usize,
    /// said to them ("Leo, he didn't mean..."): not who a he or she means
    vocative: bool,
}

/// one change, for the log
pub struct Change {
    pub from: String,
    pub to: String,
    pub why: String,
}

/// the line with names and gendered words made to fit who is shown; None
/// when nothing changed.
pub fn rewrite(cast: &Cast, text: &[u16], changes: &mut Vec<Change>) -> Option<Vec<u16>> {
    // tokens: words outside codes; codes update the scene as they come
    let mut out: Vec<u16> = Vec::with_capacity(text.len() + 32);
    let mut changed = false;
    // first pass: words and the scene at each
    let mut toks: Vec<Tok> = Vec::new();
    let mut scenes: Vec<(usize, Scene, usize)> = Vec::new(); // (token index, scene, utterance number)
    let mut scene = Scene::default();
    let mut utterance = 0usize;
    let mut i = 0;
    let mut code_runs: Vec<(usize, usize)> = Vec::new();
    while i < text.len() {
        if text[i] == b'$' as u16 {
            let end = code_end(text, i);
            let code = &text[i..end];
            code_runs.push((i, end));
            if code.len() > 3 && code[1] == b'W' as u16 && (code[2] == b'm' as u16 || code[2] == b's' as u16) {
                let key = &code[3..code.len().saturating_sub(1)];
                let who = cast.by_key.get(key).copied();
                if code[2] == b'm' as u16 {
                    let slot = text.get(end).copied().unwrap_or(0);
                    if let Some(p) = who {
                        scene.windows.retain(|w| w.0 != slot && w.1 != p);
                        scene.windows.push((slot, p));
                    } else {
                        scene.windows.retain(|w| w.0 != slot);
                    }
                } else if who != scene.speaker {
                    if let Some(sp) = scene.speaker {
                        scene.last_other = Some(sp);
                    }
                    scene.speaker = who;
                    utterance += 1;
                }
            } else if code.len() >= 3 && code[1] == b'W' as u16 && code[2] == b'd' as u16 {
                if let Some(sp) = scene.speaker {
                    scene.windows.retain(|w| w.1 != sp);
                }
            }
            // Corrin's name, a token of its own (copied as it is)
            if code.len() >= 3 && code[1] == b'N' as u16 {
                toks.push(Tok { start: i, end, word: false, corrin: true });
                scenes.push((toks.len() - 1, scene.clone(), utterance));
            }
            i = end;
            continue;
        }
        if is_letter(text[i]) {
            let start = i;
            while i < text.len() && is_letter(text[i]) {
                i += 1;
            }
            // ma'am
            if lower(&text[start..i]) == "ma" && text.get(i) == Some(&(b'\'' as u16)) && text.get(i + 1..i + 3).is_some_and(|s| lower(s) == "am") {
                i += 3;
            }
            toks.push(Tok { start, end: i, word: true, corrin: false });
            scenes.push((toks.len() - 1, scene.clone(), utterance));
            continue;
        }
        let start = i;
        i += 1;
        toks.push(Tok { start, end: i, word: false, corrin: false });
        scenes.push((toks.len() - 1, scene.clone(), utterance));
    }
    let _ = code_runs;

    // what each word means, in order; mentions as they come
    let mut mentions: Vec<Mention> = Vec::new();
    let mut replace: Vec<Option<Vec<u16>>> = vec![None; toks.len()];
    // rando4: what is said around an animal (a dragon, a horse, a pegasus):
    // "Good boy!", "Hold on, big boy" there are said to the animal
    let animal_toks: Vec<usize> = toks
        .iter()
        .enumerate()
        .filter(|(_, tk)| tk.word && ANIMALS.contains(&lower(&text[tk.start..tk.end]).as_str()))
        .map(|(k, _)| k)
        .collect();
    // rando4: who each he or she means (changed or not), for "she's a girl"
    let mut resolved: Vec<Option<usize>> = vec![None; toks.len()];
    let word_at = |t: usize| -> &[u16] { &text[toks[t].start..toks[t].end] };
    // the word before/after a token, across spaces and line breaks only
    let prev_word = |t: usize| -> Option<usize> {
        let mut k = t;
        while k > 0 {
            k -= 1;
            if toks[k].word {
                return Some(k);
            }
            let u = text[toks[k].start];
            if u != b' ' as u16 && u != b'\n' as u16 {
                return None;
            }
            // a code between the words ends the run
            if toks[k].end != toks[k + 1].start {
                return None;
            }
        }
        None
    };
    let next_word = |t: usize| -> Option<usize> {
        let mut k = t + 1;
        while k < toks.len() {
            if toks[k - 1].end != toks[k].start {
                return None;
            }
            if toks[k].word {
                return Some(k);
            }
            let u = text[toks[k].start];
            if u != b' ' as u16 && u != b'\n' as u16 {
                return None;
            }
            k += 1;
        }
        None
    };
    // "Leo's": the word after a name is s, after an apostrophe
    let possessive = |t: usize| -> bool {
        t + 2 < toks.len()
            && !toks[t + 1].word
            && (text[toks[t + 1].start] == b'\'' as u16 || text[toks[t + 1].start] == 0x2019)
            && toks[t + 2].word
            && lower(word_at(t + 2)) == "s"
    };
    let person_named = |t: usize| -> Option<usize> { cast.by_name.get(word_at(t)).copied() };
    // rando4: the word before a token, across punctuation (not codes)
    let prev_word_across = |t: usize| -> Option<usize> {
        let mut k = t;
        while k > 0 {
            k -= 1;
            if toks[k].end != toks[k + 1].start {
                return None;
            }
            if toks[k].word {
                return Some(k);
            }
        }
        None
    };
    // rando4: the word after a token, across punctuation (not codes)
    let next_word_across = |t: usize| -> Option<usize> {
        let mut k = t + 1;
        while k < toks.len() {
            if toks[k - 1].end != toks[k].start {
                return None;
            }
            if toks[k].word {
                return Some(k);
            }
            k += 1;
        }
        None
    };
    // rando4: the first thing before a token that is not a space: a
    // character, b'|' for a code (a new window or speaker), b'w' for a word,
    // 0 at the start
    let before_char = |t: usize| -> u16 {
        let mut k = t;
        while k > 0 {
            k -= 1;
            if toks[k].end != toks[k + 1].start {
                return b'|' as u16;
            }
            if toks[k].word || toks[k].corrin {
                return b'w' as u16;
            }
            let u = text[toks[k].start];
            if u != b' ' as u16 && u != b'\n' as u16 {
                return u;
            }
        }
        0
    };

    // a line with no $G code is written for one gender of Corrin (the
    // _PCM/_PCF versions, Corrin's supports): there a plain he, his or
    // brother can be Corrin's, so Corrin counts as a candidate
    let one_gender = !text.windows(2).any(|w| w[0] == b'$' as u16 && w[1] == b'G' as u16);
    // a name said to someone: at the start of what they say or after a
    // sentence's end, then a comma; or after a comma, then the sentence's end
    let is_vocative = |t: usize| -> bool {
        let after = text.get(toks[t].end).copied().unwrap_or(0);
        let mut k = t;
        let mut before = 0u16;
        while k > 0 {
            k -= 1;
            let u = text[toks[k].start];
            if toks[k].word || toks[k].corrin {
                before = b'w' as u16;
                break;
            }
            if u != b' ' as u16 && u != b'\n' as u16 {
                before = if toks[k].end != toks[k + 1].start { b'|' as u16 } else { u };
                break;
            }
        }
        let start = before == 0 || before == b'|' as u16 || [b'.', b'!', b'?', b'\n'].iter().any(|&c| before == c as u16);
        let ends = [b'.', b'!', b'?'].iter().any(|&c| after == c as u16);
        (start && after == b',' as u16) || (before == b',' as u16 && ends)
    };
    for t in 0..toks.len() {
        if toks[t].corrin {
            // corrinswap: Corrin's name is that of whoever is shown there
            if let Some(name) = cast.corrin_name.as_ref().filter(|_| text.get(toks[t].start + 2) == Some(&(b'u' as u16))) {
                replace[t] = Some(name.clone());
            }
            if one_gender {
                mentions.push(Mention { who: Who::Other(None), tok: t, utterance: scenes[t].2, vocative: false });
            }
            continue;
        }
        if !toks[t].word {
            continue;
        }
        let (_, sc, utt) = &scenes[t];
        let word = word_at(t);
        if let Some(p) = person_named(t) {
            mentions.push(Mention { who: Who::Person(p), tok: t, utterance: *utt, vocative: is_vocative(t) });
            if cast.people[p].renamed() {
                replace[t] = Some(cast.people[p].shown_name.clone());
            }
            continue;
        }
        if let Some(&g) = cast.extra.get(word) {
            mentions.push(Mention { who: Who::Other(g), tok: t, utterance: *utt, vocative: is_vocative(t) });
            continue;
        }
        let low = lower(word);
        let Some(entry) = WORDS.iter().find(|e| e.word == low) else { continue };
        // rando4: Miss and Mister only as titles (capitalised, before a name)
        if (low == "miss" || low == "mister") && (word[0] != b'M' as u16 || next_word(t).and_then(person_named).is_none()) {
            continue;
        }
        // rando4: who the sentence's own structure says the word means: the
        // subject of "I'm a...", "you're my...", "Leo is a...", "she's
        // a..."; a name followed by "daughter of..."; "as a woman, I";
        // "my boy!" said to someone
        // an animal named shortly before: a word said on its own ("Good
        // boy!") is said to it
        let near_animal = entry.kind == Kind::Noun && animal_toks.iter().any(|&k| k < t && t - k <= 150);
        let pattern = if entry.kind == Kind::Pronoun || entry.kind == Kind::Address || next_word(t).and_then(person_named).is_some() || possessive(t) {
            None
        } else {
            let subject = |k: usize| -> Option<usize> {
                match lower(word_at(k)).as_str() {
                    "i" => sc.speaker,
                    "you" => sc.listener(),
                    // a family word about "he" is left to the family rules
                    // (the pronoun's own guess is weaker than the family's)
                    "he" | "she" if !matches!(entry.kind, Kind::Kin(_)) => resolved[k],
                    "he" | "she" => None,
                    _ => person_named(k),
                }
            };
            predicate_referent(t, &prev_word, &word_at, &possessive, &subject)
                .or_else(|| appositive_referent(t, &mentions, *utt, &next_word, &word_at, &before_char))
                .or_else(|| {
                    if near_animal {
                        None
                    } else if entry.kind == Kind::Noun || (entry.kind == Kind::Title && low != "king" && low != "queen") {
                        phrase_referent(t, sc, &prev_word, &next_word, &word_at, &|k| text.get(toks[k].end).copied(), &before_char, &prev_word_across, &next_word_across)
                    } else {
                        None
                    }
                })
        };
        let nounish = pattern.is_some();
        // the one the word means
        let referent: Option<usize> = if pattern.is_some() { pattern } else { match entry.kind {
            Kind::Title => {
                // Prince Leo; Lady Camilla. A title alone is left as it is
                // (my lady is an address)
                if let Some(n) = next_word(t).and_then(person_named) {
                    Some(n)
                } else if prev_word(t).is_some_and(|p| lower(word_at(p)) == "my") && low != "king" && low != "queen" {
                    sc.listener()
                } else {
                    None
                }
            }
            // Sir Silas: the name (and Lady for a woman); sir alone: the listener
            Kind::Address => next_word(t).and_then(person_named).or_else(|| sc.listener()),
            // I'm a (good) man / you're a fine woman
            Kind::Noun => noun_referent(t, sc, &prev_word, &word_at),
            Kind::Kin(rel) => {
                if let Some(n) = next_word(t).and_then(person_named) {
                    // brother Leo
                    Some(n)
                } else {
                    kin_referent(cast, t, rel, entry.female, sc, *utt, &mentions, &prev_word, &word_at, &possessive, &person_named)
                }
            }
            Kind::Pronoun => {
                // "his own", "her own": still the pronoun
                pronoun_referent(cast, entry.female, sc, *utt, &mentions, one_gender)
            }
        } };
        if entry.kind == Kind::Pronoun {
            resolved[t] = referent;
        }
        let Some(r) = referent else { continue };
        let person = &cast.people[r];
        if person.either || person.female != entry.female || !person.swapped() {
            continue;
        }
        let next = next_word(t).map(|n| lower(word_at(n)));
        let titled = low == "sir" && next_word(t).and_then(person_named).is_some();
        let new = if titled {
            w("lady")
        } else if nounish && low == "lady" && !prev_word(t).is_some_and(|p| lower(word_at(p)) == "my") {
            // rando4: a lady (not a title): a young man, a fine gentleman
            let before = prev_word(t).map(|p| lower(word_at(p)));
            w(if matches!(before.as_deref(), Some("young" | "little" | "old" | "grown")) { "man" } else { "gentleman" })
        } else {
            swap_word(&low, entry.other, next.as_deref(), text.get(toks[t].end).copied())
        };
        let new = match_case(word, &new);
        changes.push(Change {
            from: String::from_utf16_lossy(word),
            to: String::from_utf16_lossy(&new),
            why: format!(
                "{} (shown as {}), speaker {}: \"{}\"",
                String::from_utf16_lossy(&person.name),
                String::from_utf16_lossy(&person.shown_name),
                sc.speaker.map_or("?".to_string(), |s| String::from_utf16_lossy(&cast.people[s].name)),
                context(text, toks[t].start, toks[t].end)
            ),
        });
        replace[t] = Some(new);
    }

    // put the line together
    let mut at = 0;
    for (t, tok) in toks.iter().enumerate() {
        out.extend_from_slice(&text[at..tok.start]);
        match &replace[t] {
            Some(new) => {
                out.extend_from_slice(new);
                changed = true;
            }
            None => out.extend_from_slice(&text[tok.start..tok.end]),
        }
        at = tok.end;
    }
    out.extend_from_slice(&text[at..]);
    changed.then_some(out)
}

/// man/woman after "I'm a", "I am a", "you're a", "you are a" (adjectives
/// between): the speaker or the listener
fn noun_referent<'a>(t: usize, sc: &Scene, prev_word: &dyn Fn(usize) -> Option<usize>, word_at: &dyn Fn(usize) -> &'a [u16]) -> Option<usize> {
    let mut k = prev_word(t)?;
    let mut steps = 0;
    loop {
        let pw = lower(word_at(k));
        if pw == "a" || pw == "an" {
            break;
        }
        if steps >= 3 || !(KIN_ADJECTIVES.contains(&pw.as_str()) || pw.ends_with("ly") || pw.len() > 2) {
            return None;
        }
        k = prev_word(k)?;
        steps += 1;
    }
    let verb = prev_word(k)?;
    let vw = lower(word_at(verb));
    // I'm: "I" "'" "m" are separate tokens: m, re
    let subject = match vw.as_str() {
        "m" | "am" | "was" => "i",
        "re" | "are" | "were" => "you",
        _ => return None,
    };
    if vw == "m" || vw == "re" {
        // the word before the apostrophe
        let s = verb.checked_sub(2)?;
        let sw = lower(word_at(s));
        if sw != subject {
            return None;
        }
    } else {
        let s = prev_word(verb)?;
        if lower(word_at(s)) != subject {
            return None;
        }
    }
    if subject == "i" {
        sc.speaker
    } else {
        sc.listener()
    }
}

/// rando4: words between a determiner and the word ("a fine young man")
fn is_modifier(w: &str) -> bool {
    KIN_ADJECTIVES.contains(&w) || (w.ends_with("ly") && w.len() > 3) || MODIFIERS.contains(&w)
}

/// rando4: words before a comma that make what follows an exclamation, not
/// someone spoken to ("Aw, man", "Hoo, boy", "Here, boy" to an animal)
const INTERJECTIONS: &[&str] = &[
    "aw", "ah", "oh", "ohh", "hoo", "boy", "man", "but", "and", "gods", "geez", "jeez", "wow", "whoa", "um", "uh", "yeah", "so", "well", "ugh",
    "argh", "huh", "hmm", "eh", "here", "there", "right", "okay", "ok", "now", "yes", "no", "sheesh", "whew", "phew", "ha", "haha", "heh",
];

/// rando4: a word that can stand between a determiner and the word: an
/// adjective (or adverb) as far as can be told, not a function word
fn is_adjective_like(w: &str) -> bool {
    is_modifier(w)
        || (w.len() > 2
            && !matches!(
                w,
                "of" | "to" | "and" | "or" | "but" | "that" | "who" | "which" | "in" | "on" | "at" | "for" | "with" | "as" | "than" | "the"
                    | "you" | "your" | "his" | "her" | "him" | "she" | "our" | "their" | "they" | "not" | "are" | "was" | "were" | "is" | "has"
                    | "have" | "had" | "will" | "would" | "can" | "could" | "should" | "from" | "into" | "about" | "like" | "kind" | "sort"
                    | "type"
            ))
}

/// rando4: animals a man/boy/girl word may be said to (a mount, a pet)
const ANIMALS: &[&str] = &[
    "dragon", "dragons", "wyvern", "wyverns", "horse", "horses", "pegasus", "pegasi", "kinshi", "wolf", "wolves", "dog", "dogs", "puppy",
    "steed", "steeds", "mount", "beast", "beasts", "pony", "cat", "kitty", "bird", "falcon", "hawk", "critter", "creature", "animal", "animals",
];

/// rando4: more adjectives seen before the words
const MODIFIERS: &[&str] = &[
    "good", "fine", "young", "old", "strong", "proud", "great", "wise", "kind", "gentle", "fair", "lovely", "handsome", "pretty", "smart",
    "clever", "tough", "brave", "bold", "humble", "simple", "common", "ordinary", "real", "true", "grown", "big", "small", "tiny", "nice",
    "strange", "odd", "weird", "lucky", "happy", "sad", "lonely", "busy", "lazy", "honest", "decent", "worthy", "fierce", "loyal", "new",
    "first", "second", "eldest", "youngest", "oldest", "older", "younger", "elder", "dear", "sweet", "beloved", "crown", "foolish", "wonderful",
    "terrible", "horrible", "awful", "grand", "mighty", "powerful", "weak", "fragile", "delicate", "refined", "noble", "lowly", "mere",
];

/// rando4: the subject of a "to be" before the word: "I'm a (fine) man",
/// "you're my daughter", "Leo is a good man", "she's a girl", "I am no
/// lady", "I'm not your mother". The word has to come after a determiner
/// (a, an, the, no, my, your, his, her, our, their, such, Leo's).
fn predicate_referent<'a>(
    t: usize,
    prev_word: &dyn Fn(usize) -> Option<usize>,
    word_at: &dyn Fn(usize) -> &'a [u16],
    possessive: &dyn Fn(usize) -> bool,
    subject: &dyn Fn(usize) -> Option<usize>,
) -> Option<usize> {
    let mut k = prev_word(t)?;
    let mut steps = 0;
    while steps < 3 && is_adjective_like(&lower(word_at(k))) {
        k = prev_word(k)?;
        steps += 1;
    }
    // the determiner
    let det = lower(word_at(k));
    let mut verb = match det.as_str() {
        "a" | "an" | "the" | "no" | "my" | "your" | "his" | "her" | "our" | "their" | "such" => prev_word(k)?,
        "s" => {
            // Leo's daughter: the name's possessive is the determiner
            let name = k.checked_sub(2).filter(|&n| possessive(n))?;
            prev_word(name)?
        }
        _ => return None,
    };
    let mut skips = 0;
    loop {
        let vw = lower(word_at(verb));
        let skip = matches!(
            vw.as_str(),
            "not" | "just" | "only" | "truly" | "really" | "still" | "also" | "quite" | "such" | "half" | "merely" | "simply" | "nothing"
                | "more" | "than" | "always" | "never" | "ever" | "now" | "already" | "so" | "definitely" | "certainly" | "obviously"
        ) || (vw == "like" && matches!(det.as_str(), "a" | "an" | "the"));
        if !skip || skips >= 3 {
            break;
        }
        verb = prev_word(verb)?;
        skips += 1;
    }
    let vw = lower(word_at(verb));
    let s = match vw.as_str() {
        // I'm, you're, she's: the word before the apostrophe (a contraction:
        // no space between)
        "m" | "re" | "s" => {
            if prev_word(verb).is_some() {
                return None;
            }
            let s = verb.checked_sub(2)?;
            if !word_at(s).iter().all(|&u| is_letter(u)) {
                return None;
            }
            s
        }
        "am" | "are" | "is" | "was" | "were" => prev_word(verb)?,
        _ => return None,
    };
    let sw = lower(word_at(s));
    // "You are not your father", "I'm my own man": the subject's own
    // relative is someone else
    if matches!((sw.as_str(), det.as_str()), ("i", "my") | ("you", "your") | ("he", "his") | ("she", "her")) {
        return None;
    }
    // "Are you a man?" has no subject before its verb
    match (vw.as_str(), sw.as_str()) {
        ("m" | "am", "i") | ("re" | "are" | "were", "you") | ("s" | "is", _) | ("was", _) => {}
        _ => return None,
    }
    if matches!(sw.as_str(), "it" | "that" | "this" | "there" | "what" | "who" | "here" | "which" | "one" | "let") {
        return None;
    }
    subject(s)
}

/// rando4: "I am Rinkah! Daughter of the Flame Tribe's chieftain." and
/// "Hinoka, daughter of Sumeragi": a family word or title that starts its
/// sentence or follows a comma, followed by "of", means the one named last
/// in what the speaker is saying, close before it
fn appositive_referent<'a>(
    t: usize,
    mentions: &[Mention],
    utt: usize,
    next_word: &dyn Fn(usize) -> Option<usize>,
    word_at: &dyn Fn(usize) -> &'a [u16],
    before_char: &dyn Fn(usize) -> u16,
) -> Option<usize> {
    let of = next_word(t)?;
    if lower(word_at(of)) != "of" {
        return None;
    }
    let before = before_char(t);
    if ![b'.', b'!', b',', b'?'].iter().any(|&c| before == c as u16) {
        return None;
    }
    let last = mentions.iter().filter(|m| m.utterance == utt && m.tok < t && !m.vocative).last()?;
    if t - last.tok > 12 {
        return None;
    }
    match last.who {
        Who::Person(p) => Some(p),
        Who::Other(_) => None,
    }
}

/// rando4: "As a woman, I...", "a man like me", "a girl like you", and a
/// word said to someone: "Come here, my boy!", "Good work, young man."
fn phrase_referent<'a>(
    t: usize,
    sc: &Scene,
    prev_word: &dyn Fn(usize) -> Option<usize>,
    next_word: &dyn Fn(usize) -> Option<usize>,
    word_at: &dyn Fn(usize) -> &'a [u16],
    after: &dyn Fn(usize) -> Option<u16>,
    before_char: &dyn Fn(usize) -> u16,
    prev_word_across: &dyn Fn(usize) -> Option<usize>,
    next_word_across: &dyn Fn(usize) -> Option<usize>,
) -> Option<usize> {
    // the modifiers before it, and what is before them
    let mut first = t;
    let mut k = prev_word(t);
    let mut steps = 0;
    let mut has_my = false;
    while let Some(p) = k {
        let pw = lower(word_at(p));
        if steps < 3 && (is_modifier(&pw) || pw == "my") {
            has_my |= pw == "my";
            first = p;
            k = prev_word(p);
            steps += 1;
            continue;
        }
        break;
    }
    // a man like me / a girl like you
    if let Some(like) = next_word(t).filter(|&n| lower(word_at(n)) == "like") {
        if let (Some(obj), Some(a)) = (next_word(like), k) {
            if matches!(lower(word_at(a)).as_str(), "a" | "an") {
                match lower(word_at(obj)).as_str() {
                    "me" => return sc.speaker,
                    "you" => return sc.listener(),
                    _ => {}
                }
            }
        }
    }
    // As a woman, ...: the speaker
    if let Some(a) = k.filter(|&a| matches!(lower(word_at(a)).as_str(), "a" | "an")) {
        if let Some(as_) = prev_word(a).filter(|&x| lower(word_at(x)) == "as") {
            let b = before_char(as_);
            let starts = b == 0 || b == b'|' as u16 || [b'.', b'!', b'?'].iter().any(|&c| b == c as u16);
            if !starts && b == b'w' as u16 {
                // "you as a sister": the listener; "me as a": the speaker;
                // "I ... as a man": the speaker
                let before = prev_word(as_).map(|x| lower(word_at(x)));
                match before.as_deref() {
                    Some("you") => return sc.listener(),
                    Some("me") => return sc.speaker,
                    Some("him" | "her" | "them" | "us" | "it") => return None,
                    _ => {}
                }
                let mut x = prev_word_across(as_);
                let mut n = 0;
                while let Some(xi) = x {
                    let xw = lower(word_at(xi));
                    if xw == "i" {
                        return sc.speaker;
                    }
                    if matches!(xw.as_str(), "he" | "she" | "they" | "we") || n > 12 {
                        return None;
                    }
                    let bc = before_char(xi);
                    if bc == 0 || bc == b'|' as u16 || [b'.', b'!', b'?'].iter().any(|&c| bc == c as u16) {
                        return None;
                    }
                    x = prev_word_across(xi);
                    n += 1;
                }
                return None;
            }
            if starts && after(t) == Some(b',' as u16) {
                // the subject after the comma: "As a woman, I...", "As a
                // prince, you should..."
                return match next_word_across(t).map(|n| lower(word_at(n))).as_deref() {
                    Some("i") => sc.speaker,
                    Some("you") => sc.listener(),
                    _ => None,
                };
            }
        }
        return None;
    }
    // said to someone: the words stand alone between commas or a sentence's
    // edges ("Thank you, my dear girl.", "Boy! Come here.")
    if k.is_some() || steps == 0 && !has_my && before_char(first) != b',' as u16 {
        // a bare word ("Girl!") only after a comma ("Come here, girl!")
        return None;
    }
    let b = before_char(first);
    let a = after(t).unwrap_or(0);
    // "Aw, man...", "Hoo, boy.", "Here, boy!" (to an animal): not someone
    // addressed
    if b == b',' as u16 && steps == 0 && !has_my {
        if let Some(w) = prev_word_across(first) {
            if INTERJECTIONS.contains(&lower(word_at(w)).as_str()) {
                return None;
            }
        }
        if lower(word_at(t)) == "man" {
            return None;
        }
    }
    let edge_before = b == 0 || b == b'|' as u16 || [b'.', b'!', b'?', b','].iter().any(|&c| b == c as u16);
    let edge_after = [b'.', b'!', b'?', b','].iter().any(|&c| a == c as u16) || a == 0x2026;
    if edge_before && edge_after {
        return sc.listener();
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn kin_referent<'a>(
    cast: &Cast,
    t: usize,
    rel: Rel,
    female: bool,
    sc: &Scene,
    utt: usize,
    mentions: &[Mention],
    prev_word: &dyn Fn(usize) -> Option<usize>,
    word_at: &dyn Fn(usize) -> &'a [u16],
    possessive: &dyn Fn(usize) -> bool,
    person_named: &dyn Fn(usize) -> Option<usize>,
) -> Option<usize> {
    if rel == Rel::Spouse || rel == Rel::Other {
        return None;
    }
    // the owner: my/our -> the speaker, your -> the listener, Leo's -> Leo;
    // his/her/their -> unknown; none (dear little brother) -> the speaker
    let mut k = prev_word(t);
    let mut owner = sc.speaker;
    let mut steps = 0;
    while let Some(p) = k {
        let pw = lower(word_at(p));
        if KIN_ADJECTIVES.contains(&pw.as_str()) && steps < 3 {
            k = prev_word(p);
            steps += 1;
            continue;
        }
        match pw.as_str() {
            "my" | "our" => owner = sc.speaker,
            "your" => owner = sc.listener(),
            "his" | "her" | "their" | "its" | "whose" | "a" | "an" | "the" | "no" | "any" | "every" | "each" | "other" | "another" | "this" | "that" => return None,
            "s" => {
                // Leo's brother
                let name = p.checked_sub(2).filter(|&n| possessive(n));
                owner = name.and_then(person_named);
                if owner.is_none() {
                    return None;
                }
            }
            _ => {}
        }
        break;
    }
    let owner = owner?;
    let o = &cast.people[owner];
    // who is around: on screen, or named in this line or the one before
    let mut around: Vec<usize> = sc.on_screen();
    for m in mentions.iter().filter(|m| m.utterance + 1 >= utt) {
        if let Who::Person(p) = m.who {
            around.push(p);
        }
    }
    around.retain(|&p| p != owner);
    around.sort_unstable();
    around.dedup();
    let fits = |p: usize| cast.people[p].either || cast.people[p].female == female;
    let mut candidates: Vec<usize> = match rel {
        Rel::Sibling => around.iter().copied().filter(|p| o.siblings.contains(p)).collect(),
        Rel::Parent => {
            let mut c: Vec<usize> = around.iter().copied().filter(|p| o.parents.contains(p)).collect();
            if o.child {
                // a child's parents: the fixed one and whoever married them
                c.extend(around.iter().copied().filter(|&p| !cast.people[p].child && !o.parents.contains(&p)));
            }
            if c.iter().all(|&p| !fits(p)) {
                // the one parent of that gender the table knows (a Nohr
                // royal's Father), on screen or not
                let known: Vec<usize> = o.parents.iter().copied().filter(|&p| fits(p)).collect();
                if known.len() == 1 {
                    c = known;
                }
            }
            c
        }
        Rel::Child => {
            let mut c: Vec<usize> = around.iter().copied().filter(|p| o.children.contains(p)).collect();
            if !o.child {
                c.extend(around.iter().copied().filter(|&p| cast.people[p].child && !o.children.contains(&p)));
            }
            c
        }
        Rel::Spouse | Rel::Other => Vec::new(),
    };
    candidates.retain(|&p| fits(p));
    candidates.sort_unstable();
    candidates.dedup();
    (candidates.len() == 1).then(|| candidates[0])
}

/// he/she: the one character of that gender named in this line and the one
/// before, or on screen, other than the speaker and the listener
fn pronoun_referent(cast: &Cast, female: bool, sc: &Scene, utt: usize, mentions: &[Mention], one_gender: bool) -> Option<usize> {
    let speaker = sc.speaker;
    let listener = sc.listener();
    let mut people: Vec<usize> = Vec::new();
    let mut others = 0;
    // a line written for one gender of Corrin: Corrin on screen (not
    // speaking, not spoken to) could be the one meant
    if one_gender && sc.on_screen().iter().any(|&p| cast.people[p].either && Some(p) != speaker && Some(p) != listener) {
        others += 1;
    }
    for m in mentions.iter().filter(|m| m.utterance + 1 >= utt && !(m.vocative && m.utterance == utt)) {
        match m.who {
            Who::Person(p) => people.push(p),
            Who::Other(g) => {
                if g.is_none_or(|g| g == female) {
                    others += 1;
                }
            }
        }
    }
    people.extend(sc.on_screen());
    // the ones spoken to by name in this utterance are not he or she
    let addressed: Vec<usize> = mentions
        .iter()
        .filter(|m| m.vocative && m.utterance == utt)
        .filter_map(|m| if let Who::Person(p) = m.who { Some(p) } else { None })
        .collect();
    people.retain(|&p| Some(p) != speaker && Some(p) != listener && !addressed.contains(&p));
    // Corrin is never a plain he or she: the game says $Ghe,she| for Corrin
    people.retain(|&p| !cast.people[p].either && cast.people[p].female == female);
    people.sort_unstable();
    people.dedup();
    (others == 0 && people.len() == 1).then(|| people[0])
}

/// the other gender's word, for this use: her -> his or him, his -> her or
/// hers, by what follows
fn swap_word(low: &str, other: &str, next: Option<&str>, after: Option<u16>) -> Vec<u16> {
    let follows_word = next.is_some();
    let new = match low {
        "her" => {
            if follows_word && !AFTER_OBJECT.contains(&next.unwrap()) {
                "his"
            } else {
                "him"
            }
        }
        "his" => {
            if follows_word && !AFTER_OBJECT.contains(&next.unwrap()) || after == Some(b' ' as u16) && follows_word && next == Some("own") {
                "her"
            } else {
                "hers"
            }
        }
        _ => other,
    };
    w(new)
}

/// the new word in the old word's case: Leo -> Effie, BROTHER -> SISTER
fn match_case(old: &[u16], new: &[u16]) -> Vec<u16> {
    let upper = |u: u16| (b'A' as u16..=b'Z' as u16).contains(&u);
    let all_upper = old.len() > 1 && old.iter().all(|&u| upper(u));
    let mut out = new.to_vec();
    for (i, u) in out.iter_mut().enumerate() {
        let is_lower = (b'a' as u16..=b'z' as u16).contains(u);
        if is_lower && (all_upper || i == 0 && upper(old[0])) {
            *u -= 32;
        }
    }
    out
}

fn context(text: &[u16], start: usize, end: usize) -> String {
    let mut a = start.saturating_sub(40);
    let mut b = (end + 30).min(text.len());
    while a > 0 && text[a] != b'|' as u16 && text[a] != b'\n' as u16 && start - a < 60 {
        a -= 1;
    }
    while b < text.len() && text[b] != b'$' as u16 && b - end < 40 {
        b += 1;
    }
    String::from_utf16_lossy(&text[a..b]).replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(name: &str, key: &str, female: bool, shown: &str, shown_female: bool) -> TextPerson {
        TextPerson { name: w(name), key: w(key), female, shown_name: w(shown), shown_female, ..Default::default() }
    }

    fn cast() -> Cast {
        let mut c = Cast::default();
        let camilla = c.add(person("Camilla", "カミラ", true, "Arete", true));
        let leo = c.add(person("Leo", "レオン", false, "Effie", true));
        let elise = c.add(person("Elise", "エリーゼ", true, "Fuga", false));
        let xander = c.add(person("Xander", "マークス", false, "Subaki", false));
        let garon = c.add(person("Garon", "ガロン", false, "Zola", false));
        let fam = [camilla, leo, elise, xander];
        for &a in &fam {
            c.people[a].siblings = fam.iter().copied().filter(|&b| b != a).collect();
            c.people[a].parents = vec![garon];
        }
        c.people[garon].children = fam.to_vec();
        let corrin = c.add(TextPerson { name: w("Corrin"), key: w("username"), either: true, shown_name: w("Corrin"), ..Default::default() });
        c.corrin = Some(corrin);
        for &a in &fam {
            c.people[a].siblings.push(corrin);
            c.people[corrin].siblings.push(a);
        }
        c
    }

    fn run(c: &Cast, s: &str) -> (String, Vec<Change>) {
        let mut ch = Vec::new();
        let out = rewrite(c, &w(s), &mut ch).map_or(s.to_string(), |o| String::from_utf16(&o).unwrap());
        (out, ch)
    }

    #[test]
    fn the_users_line() {
        // Chapter 2 (A000 EV ENEMY 1): Leo asks, Camilla answers; Elise has left
        let c = cast();
        let line = "$Wmエリーゼ|3$w0|$Wsエリーゼ|$WaOh, no!$k\n$Wmカミラ|7$w0|$Wsカミラ|$WaNot to worry, Elise.$k\n$Wsエリーゼ|$Wd$w0|$Wmレオン|3$w0|$Wsレオン|$WaHave you spotted a Dragon Vein, Camilla?$k\n$Wsカミラ|$Wa$Eキメ,|You know me too well, dear little brother!$k";
        let (out, ch) = run(&c, line);
        assert!(out.contains("You know me too well, dear little sister!"), "{out}");
        assert!(out.contains("Not to worry, Fuga."), "{out}");
        assert!(out.contains("Dragon Vein, Arete?"), "{out}");
        assert_eq!(ch.len(), 1);
    }

    #[test]
    fn titles_and_pronouns() {
        let c = cast();
        let (out, _) = run(&c, "$Wmカミラ|3$Wsカミラ|$WaPrince Leo is late. Is he hurt? I'll find him and his horse.$k");
        assert_eq!(out, "$Wmカミラ|3$Wsカミラ|$WaPrincess Effie is late. Is she hurt? I'll find her and her horse.$k");
        // two men named: he stays
        let (out, _) = run(&c, "$Wmカミラ|3$Wsカミラ|$WaLeo and Xander left. He said so.$k");
        assert!(out.ends_with("Effie and Subaki left. He said so.$k"), "{out}");
        // her as an object
        let c2 = {
            let mut c = cast();
            c.people[0].shown_female = false;
            c
        };
        let (out, _) = run(&c2, "$Wmレオン|3$Wsレオン|$WaCamilla is here. Tell her to come to me. Her axe is ready.$k");
        assert!(out.contains("Tell him to come to me. His axe is ready."), "{out}");
    }

    #[test]
    fn corrin_keeps_words_ambiguous() {
        let c = cast();
        // Corrin on screen: "brother" could be Corrin
        let (out, ch) = run(&c, "$Wmカミラ|7$Wmレオン|3$Wmusername|5$Wsカミラ|$WaMy dear brother!$k");
        assert_eq!(ch.len(), 0, "{out}");
    }

    #[test]
    fn vocatives_and_corrin() {
        let c = cast();
        // Leo is spoken to: the he is someone else (Xander), not Leo
        let (out, ch) = run(&c, "$Wmusername|3$Wmレオン|7$Wmマークス|5$Wsマークス|$WaNo.$k$Wsusername|$WaLeo, he didn't mean it.$k");
        assert_eq!(ch.len(), 0, "{out}");
        // a line for one gender of Corrin: his may be Corrin's
        let (out, ch) = run(&c, "$Wmエリーゼ|3$Wsエリーゼ|$WaLeo, don't scare $Nu before his first mission!$k");
        assert_eq!(ch.len(), 0, "{out}");
    }

    #[test]
    fn self_and_listener_words() {
        // rando4: the user's line (Rinkah shown as Kiragi)
        let mut c = cast();
        let rinkah = c.add(person("Rinkah", "リンカ", true, "Kiragi", false));
        let _ = rinkah;
        let (out, _) = run(&c, "$Wmリンカ|3$Wsリンカ|$WaI am Rinkah! Daughter of the Flame Tribe's honorable chieftain.$k");
        assert!(out.contains("I am Kiragi! Son of the Flame Tribe's"), "{out}");
        // I'm a..., you're my..., as a...
        let (out, _) = run(&c, "$Wmリンカ|3$Wmレオン|7$Wsリンカ|$WaI'm a proud woman. You're my little brother, Leo. As a woman, I know.$k");
        assert!(out.contains("I'm a proud man. You're my little sister, Effie. As a man, I know."), "{out}");
        // not the subject's own relative; not a possessive
        let (out, ch) = run(&c, "$Wmリンカ|3$Wmレオン|7$Wsリンカ|$WaYou are not your father. I'm my brother's equal.$k");
        assert_eq!(ch.len(), 0, "{out}");
        // an exclamation is not someone spoken to
        let (out, ch) = run(&c, "$Wmリンカ|3$Wmカミラ|7$Wsリンカ|$WaAw, man... Hoo, boy.$k");
        assert_eq!(ch.len(), 0, "{out}");
    }

    #[test]
    fn father_is_garon() {
        let mut c = cast();
        c.people[4].shown_female = true;
        c.people[4].shown_name = w("Sakura");
        let (out, _) = run(&c, "$Wmレオン|3$Wsレオン|$WaFather will be furious.$k");
        assert!(out.contains("Mother will be furious."), "{out}");
    }
}
