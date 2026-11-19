//! jpn2: Haiku's own lines in English on the Japanese game (Fire Emblem if)
//! under an English translation mod (Team If, the Uncensored Patch).
//!
//! On the Japanese game Haiku adds lines to the script in Japanese (the
//! builtin layer from derived-jpn.pack: same-gender S supports, Kana's
//! chosen gender, the family variants, Kana's question). Under a mod that
//! puts the script in English, those lines are made in English when the
//! game starts, from the translation's own English lines, by the English
//! rules Haiku made the USA game's lines with (dev/supports/make_text.py:
//! its reviewed edits, which apply where the translation words a line as
//! the USA game does, and its word rules: kinship words, titles, he and
//! she). Which line comes from which, and by which rule, is the plan
//! (jpn_english_plan.tsv, written by dev/region/jpn2/english_plan.py from
//! the Japanese game's labels: no text), so the labels are exactly the ones
//! the Japanese builtin layer has and the host asks for.
//!
//! Where the translation words a line differently from the USA game, the
//! reviewed edits don't apply; a few word rules that always name Corrin
//! there (Lady $Nu, milady) are added for the S supports, and Kana's
//! words for a parent take the translation's Mom and Dad as well as the
//! USA game's Mama and Papa.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::OnceLock;

use regex::{Captures, Regex};
use zakuro_fs::msgarc::Message;

const PLAN: &str = include_str!("jpn_english_plan.tsv");

static ACTIVE: AtomicBool = AtomicBool::new(false);
static WORD: AtomicPtr<u32> = AtomicPtr::new(std::ptr::null_mut());

/// whether the game's script is English on the Japanese game (a translation
/// mod is on): Haiku's own menus and its entries in the game's menus are
/// then English too (fates_ui::ja, overrides/jpn_text.c).
#[inline]
pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// the word overrides/jpn_text.c reads (haiku_jpn_english).
///
/// # Safety
/// `word` is a plain u32 that lives for the program and that the game's
/// code only reads.
pub unsafe fn set_word(word: *mut u32) {
    WORD.store(word, Ordering::Relaxed);
    set_active(active());
}

/// set once per start, when the mods' text is laid (haiku_fanpatch::lay_text)
pub fn set_active(on: bool) {
    ACTIVE.store(on, Ordering::Relaxed);
    let word = WORD.load(Ordering::Relaxed);
    if !word.is_null() {
        // SAFETY: set_word's contract
        unsafe { std::ptr::write_volatile(word, on as u32) };
    }
}

/// the Sunglasses' name in the avatar creator (sunglasses.c): MID_FACEACCE_サングラス
pub const SUNGLASSES_LABEL: &str = "4d49445f46414345414343455f83548393834f83898358";
/// the map menu's End (qol5's End Turn): MID_MENU_終了
pub const END_LABEL: &str = "4d49445f4d454e555f8f4997b9";

/// Kana's name in the script: MPID_カンナ
const KANA_NAME_LABEL: &str = "4d5049445f834a83938369";

/// Kana's question when the paralogue starts (langs.py ASK, English)
const ASK: [&str; 4] = ["Kana is your child. Will Kana be\nyour son or your daughter?", "Son", "Daughter", "Leave it to fate"];

fn hex(text: &str) -> Vec<u8> {
    (0..text.len() / 2).filter_map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).ok()).collect()
}

/// a label written as Shift JIS in hex
pub fn label_of(hex_text: &str) -> Vec<u8> {
    hex(hex_text)
}

struct Row {
    part: &'static str,
    dst: &'static str,
    label: Vec<u8>,
    src: &'static str,
    source: Vec<u8>,
    param: &'static str,
}

fn plan() -> &'static [Row] {
    static PLAN_ROWS: OnceLock<Vec<Row>> = OnceLock::new();
    PLAN_ROWS.get_or_init(|| {
        PLAN.lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .filter_map(|line| {
                let f: Vec<&'static str> = line.split('\t').collect();
                (f.len() == 6).then(|| Row { part: f[0], dst: f[1], label: hex(f[2]), src: f[3], source: hex(f[4]), param: f[5] })
            })
            .collect()
    })
}

/// the archives the plan reads from (RomFS paths)
pub fn sources() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = plan().iter().filter(|r| !r.src.is_empty()).map(|r| r.src).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// whether a message archive's lines are English: far more Latin letters
/// than kana and kanji (the talk codes name characters in katakana). None
/// when it has too few letters to tell.
pub fn is_english(messages: &[Message]) -> Option<bool> {
    let (mut latin, mut japanese) = (0usize, 0usize);
    for message in messages {
        for &unit in &message.text {
            match unit {
                0x41..=0x5A | 0x61..=0x7A => latin += 1,
                0x3040..=0x30FF | 0x4E00..=0x9FFF => japanese += 1,
                _ => {}
            }
        }
    }
    (latin + japanese >= 40).then_some(latin > 3 * japanese)
}

// -- the rules (dev/supports/regender.py) --------------------------------------

type Rep = Box<dyn Fn(&Captures, &str) -> String + Send + Sync>;

struct Rule {
    re: Regex,
    rep: Rep,
}

fn rule(pattern: &str, rep: impl Fn(&Captures, &str) -> String + Send + Sync + 'static) -> Rule {
    Rule { re: Regex::new(pattern).expect("jpn2: a rule's pattern"), rep: Box::new(rep) }
}

/// a regex replaced by a template (${1} for a group)
fn sub(pattern: &str, template: &'static str) -> Rule {
    rule(pattern, move |c, _| {
        let mut out = String::new();
        c.expand(template, &mut out);
        out
    })
}

/// Python's str.isupper(): a cased letter and none lower case
fn is_upper(text: &str) -> bool {
    text.chars().any(char::is_uppercase) && !text.chars().any(char::is_lowercase)
}

/// regender.case_like: the word with the case of the text it replaces
fn case_like(src: &str, word: &str) -> String {
    if is_upper(src) && src.chars().count() > 1 {
        return word.to_uppercase();
    }
    let mut chars = word.chars();
    let Some(first) = chars.next() else { return String::new() };
    let first: String = if src.chars().next().is_some_and(char::is_uppercase) { first.to_uppercase().collect() } else { first.to_lowercase().collect() };
    first + chars.as_str()
}

/// regender.words: whole words, case kept
fn words(pairs: &[(&str, &'static str)]) -> Vec<Rule> {
    pairs.iter().map(|&(old, new)| rule(&format!(r"(?i)\b{}\b", regex::escape(old)), move |c, _| case_like(&c[0], new))).collect()
}

/// the words after which "her" is an object (regender.OBJECT_AFTER)
const OBJECT_WORDS: &[&str] = &[
    "to", "and", "that", "about", "for", "with", "in", "up", "so", "too", "again", "anymore", "now", "more", "back", "out", "off", "down", "like", "as", "at", "from", "or", "be",
    "happy", "feel", "know", "alone", "either", "very", "by", "on", "before", "when", "if", "than", "even", "someday", "myself", "yourself", "since", "is", "was", "after", "all",
    "well", "sometime", "anything", "really", "better", "would", "will", "can", "could", "should", "might", "must", "does", "did", "has", "had", "a", "an", "the", "i", "she", "he",
    "we", "you", "they", "it", "tomorrow", "today", "tonight", "yet", "though", "anyway", "enough", "much", "how", "what", "why", "where", "who", "smile", "cry", "proud", "sad",
    "worry", "laugh",
];

fn object_after(rest: &str) -> bool {
    let rest = rest.trim_start();
    let Some(first) = rest.chars().next() else { return true };
    // jpn2: and the translations' curly quotes
    if ".,!?;:-—…)\"'’”".contains(first) {
        return true;
    }
    let word: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    !word.is_empty() && OBJECT_WORDS.contains(&word.to_lowercase().as_str())
}

const OBJECT_VERBS: &str = "let|lets|make|makes|made|help|helps|helped|see|saw|watch|hear|heard|tell|told|ask|asked|show|showed|give|gave|thank|thanked|bring|brings|brought|teach|teaches|taught|brew|brews|bake|bakes|buy|buys|bought|get|gets|getting|got|send|sent|sing|sang|write|wrote|offer|offered|pour|poured";

fn she_to_he() -> Vec<Rule> {
    vec![
        // she's (and the translations' she’s)
        rule(r"(?i)\bshe(['’])([a-z])", |c, _| format!("{}{}{}", case_like(&c[0].chars().take(3).collect::<String>(), "he"), &c[1], &c[2])),
        rule(r"(?i)\bherself\b", |c, _| case_like(&c[0], "himself")),
        rule(r"(?i)\bhers\b", |c, _| case_like(&c[0], "his")),
        rule(&format!(r"(?i)\b({OBJECT_VERBS})(\s+)her\b"), |c, _| format!("{}{}{}", &c[1], &c[2], case_like(&c[0].chars().rev().take(3).collect::<Vec<char>>().into_iter().rev().collect::<String>(), "him"))),
        rule(r"(?i)\bher\b", |c, hay| {
            let end = c.get(0).map_or(0, |m| m.end());
            if object_after(&hay[end..]) {
                case_like(&c[0], "him")
            } else {
                c[0].to_string()
            }
        }),
        rule(r"(?i)\bher\b", |c, _| case_like(&c[0], "his")),
        rule(r"(?i)\bshe\b", |c, _| case_like(&c[0], "he")),
    ]
}

fn he_to_she() -> Vec<Rule> {
    vec![
        rule(r"(?i)\bhe(['’])([a-z])", |c, _| format!("{}{}{}", case_like(&c[0].chars().take(2).collect::<String>(), "she"), &c[1], &c[2])),
        rule(r"(?i)\bhimself\b", |c, _| case_like(&c[0], "herself")),
        rule(r"(?i)\bhim\b", |c, _| case_like(&c[0], "her")),
        rule(r"(?i)\bhis\b", |c, hay| {
            let end = c.get(0).map_or(0, |m| m.end());
            let rest = hay[end..].trim_start();
            if rest.is_empty() || rest.starts_with(['.', ',', '!', '?', ';', ':']) {
                case_like(&c[0], "hers")
            } else {
                c[0].to_string()
            }
        }),
        rule(r"(?i)\bhis\b", |c, _| case_like(&c[0], "her")),
        rule(r"(?i)\bhe\b", |c, _| case_like(&c[0], "she")),
    ]
}

/// the talk codes (regender.CODE): their text is never changed
fn codes() -> &'static Regex {
    static CODE: OnceLock<Regex> = OnceLock::new();
    CODE.get_or_init(|| {
        Regex::new(r"\$(?:G[^|]*\||Wm[^|]*\|\d?|Ws[^|]*\||E[^|]*\||w\d+\||S[a-z]{2}[^|]*\||F[a-z]\d*\||l[^|]*\||VF[^|]*\||VNM[^|]*\||Wa|Wd|Wc|Nu|t\d|k|p|a)")
            .expect("jpn2: codes")
    })
}

fn apply_piece(piece: &str, rules: &[Rule]) -> String {
    let mut piece = piece.to_string();
    for r in rules {
        let replaced = r.re.replace_all(&piece, |c: &Captures| (r.rep)(c, &piece)).into_owned();
        piece = replaced;
    }
    piece
}

/// regender.apply: the rules in order over each piece of text between codes
fn apply(text: &str, rules: &[Rule]) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut at = 0;
    for m in codes().find_iter(text) {
        if m.start() > at {
            out.push_str(&apply_piece(&text[at..m.start()], rules));
        }
        out.push_str(m.as_str());
        at = m.end();
    }
    if at < text.len() {
        out.push_str(&apply_piece(&text[at..], rules));
    }
    out
}

/// the text between the codes
fn plain(text: &str) -> String {
    codes().replace_all(text, "").into_owned()
}

/// jpn2: "Lady $Nu" and "Lord $Nu" over the whole text: $Nu is a talk
/// code, so regender's rule for it never met it between the codes
fn nu(text: &str, to_male: bool) -> String {
    if to_male {
        raw_replace(text, &[("Lady $Nu", "Lord $Nu"), ("Princess $Nu", "Prince $Nu"), ("Madam $Nu", "Lord $Nu"), ("Mistress $Nu", "Lord $Nu")])
    } else {
        raw_replace(text, &[("Lord $Nu", "Lady $Nu"), ("Prince $Nu", "Princess $Nu"), ("Master $Nu", "Lady $Nu")])
    }
}

/// jpn2: the rules over the lines the partner speaks ($Ws<speaker>|, not
/// Corrin's username), the rest as it is
fn partner_lines(text: &str, rules: &[Rule]) -> String {
    static SPEAKER: OnceLock<Regex> = OnceLock::new();
    let speaker = SPEAKER.get_or_init(|| Regex::new(r"\$Ws([^|]*)\|").expect("jpn2"));
    let mut out = String::with_capacity(text.len() + 16);
    let (mut at, mut partner) = (0, false);
    for c in speaker.captures_iter(text) {
        let m = c.get(0).expect("match");
        let piece = &text[at..m.start()];
        out.push_str(&if partner { apply(piece, rules) } else { piece.to_string() });
        out.push_str(m.as_str());
        partner = &c[1] != "username";
        at = m.end();
    }
    let piece = &text[at..];
    out.push_str(&if partner { apply(piece, rules) } else { piece.to_string() });
    out
}

fn raw_replace(text: &str, pairs: &[(&str, &str)]) -> String {
    pairs.iter().fold(text.to_string(), |text, (a, b)| text.replace(a, b))
}

const KANA_CODES_F2M: &[(&str, &str)] = &[("カンナ女", "カンナ男"), ("VOICE_KANNA_F_", "VOICE_KANNA_M_"), ("SE_KANNA_SCREAM_F", "SE_KANNA_SCREAM_M")];
const KANA_CODES_M2F: &[(&str, &str)] = &[("カンナ男", "カンナ女"), ("VOICE_KANNA_M_", "VOICE_KANNA_F_"), ("SE_KANNA_SCREAM_M", "SE_KANNA_SCREAM_F")];

/// make_text.py's reviewed edits, tolerant as the UK English profile is
/// (an edit whose words aren't there is left out)
fn edits(text: &str, list: &[(&str, &str, u32)], missed: &mut usize) -> String {
    let mut text = text.to_string();
    for &(old, new, count) in list {
        let n = text.matches(old).count();
        if n == 0 || (count != 0 && n as u32 != count) {
            *missed += 1;
            if n == 0 {
                continue;
            }
        }
        text = text.replace(old, new);
    }
    text
}

fn pair_edits(text: &str, list: &[(&str, &str)], missed: &mut usize) -> String {
    let with_count: Vec<(&str, &str, u32)> = list.iter().map(|&(a, b)| (a, b, 1)).collect();
    edits(text, &with_count, missed)
}

// -- the rule sets (dev/supports/make_text.py) ---------------------------------

struct Sets {
    x002_m: Vec<Rule>,
    x002_f: Vec<Rule>,
    to_papa: Vec<Rule>,
    to_mama: Vec<Rule>,
    kana_corrin_m: Vec<Rule>,
    kana_corrin_f: Vec<Rule>,
    die_f: Vec<Rule>,
    die_m: Vec<Rule>,
    kana_m2f: Vec<Rule>,
    kana_f2m: Vec<Rule>,
    corrin_f2m: Vec<Rule>,
    corrin_m2f: Vec<Rule>,
    s_f2m: Vec<Rule>,
    s_m2f: Vec<Rule>,
    sib_m2f: Vec<Rule>,
    sib_f2m: Vec<Rule>,
    x014: Vec<Rule>,
    child: HashMap<(bool, &'static str), Vec<Rule>>,
    both_father: Regex,
    both_mother: Regex,
}

fn stutter() -> Rule {
    rule(r"\b[A-Z]-(Father|Mother|Dad|Mom|Mama|Papa|Daddy|Mommy)\b", |c, _| format!("{}-{}", &c[1][..1], &c[1]))
}

/// a child's words for a female Corrin, for a male Corrin (child_f2m)
fn child_f2m(term: &'static str) -> Vec<Rule> {
    let mut rules = vec![sub(r"(?i)\b(?:mom|mother|mama)-(son|daughter)\b", "father-${1}"), sub(r"\bmother\b", "father")];
    rules.extend(words(&[("milady", "milord"), ("Mother", term), ("Mama", term), ("Mommy", term), ("Mom", term), ("woman", "man")]));
    rules.extend(she_to_he());
    rules.push(stutter());
    rules
}

/// a child's words for a male Corrin, for a female Corrin (child_m2f)
fn child_m2f(term: &'static str) -> Vec<Rule> {
    let mut rules = vec![sub(r"(?i)\b(?:dad|father|papa)-(son|daughter)\b", "mother-${1}"), sub(r"\bfather\b", "mother")];
    rules.extend(words(&[("milord", "milady"), ("Father", term), ("Papa", term), ("Daddy", term), ("Dad", term)]));
    rules.extend(he_to_she());
    rules.push(stutter());
    rules
}

fn sets() -> &'static Sets {
    static SETS: OnceLock<Sets> = OnceLock::new();
    SETS.get_or_init(|| {
        let cat = |a: Vec<Rule>, b: Vec<Rule>| a.into_iter().chain(b).collect::<Vec<Rule>>();
        // Kana's words for a parent: the USA game's Mama and Papa, and the
        // translations' Mom and Dad (TO_PAPA, TO_MAMA)
        let papa_words = || words(&[("Mama", "Papa"), ("Mommy", "Daddy"), ("Mom", "Dad"), ("mother", "father")]);
        let mama_words = || words(&[("Papa", "Mama"), ("Daddy", "Mommy"), ("Dad", "Mom"), ("father", "mother")]);
        let corrin_f2m = || {
            let mut r = Vec::new();
            r.extend(words(&[("milady", "milord"), ("Mama", "Dad"), ("Mommy", "Daddy"), ("Mom", "Dad"), ("mother", "father"), ("wife", "husband"), ("woman", "man"), ("princess", "prince")]));
            r.extend(she_to_he());
            r
        };
        let corrin_m2f = || {
            let mut r = Vec::new();
            r.extend(words(&[("milord", "milady"), ("Papa", "Mom"), ("Daddy", "Mommy"), ("Dad", "Mom"), ("father", "mother"), ("husband", "wife"), ("prince", "princess")]));
            r.extend(he_to_she());
            r
        };
        let kana_m2f = || {
            words(&[
                ("Thattaboy", "Thattagirl"),
                ("little man", "little lady"),
                ("young man", "young lady"),
                ("great man", "great woman"),
                ("son", "daughter"),
                ("sons", "daughters"),
                ("boy", "girl"),
                ("boys", "girls"),
                ("lad", "lass"),
                ("prince", "princess"),
            ])
        };
        let kana_f2m = || {
            words(&[
                ("Thattagirl", "Thattaboy"),
                ("little lady", "little man"),
                ("young lady", "young man"),
                ("great woman", "great man"),
                ("daughter", "son"),
                ("daughters", "sons"),
                ("girl", "boy"),
                ("girls", "boys"),
                ("lass", "lad"),
                ("princess", "prince"),
            ])
        };
        // jpn2: the words of an S conversation that name Corrin, after the
        // reviewed edits: in the partner's lines (the partner is of the
        // other gender, so a word of Corrin's old gender there is about
        // Corrin: "As a woman", "his madam", "your wife"), and the titles.
        // Sibling words are left alone (Ryoma's "my sister Hinoka").
        let s_f2m = || {
            words(&[
                ("milady", "milord"),
                ("my lady", "my lord"),
                ("madam", "master"),
                ("princess", "prince"),
                ("queen", "king"),
                ("wife", "husband"),
                ("woman", "man"),
            ])
        };
        let s_m2f = || words(&[("milord", "milady"), ("my lord", "my lady"), ("prince", "princess"), ("king", "queen"), ("husband", "wife")]);
        let mut child = HashMap::new();
        for term in ["Papa", "Dad", "Mom"] {
            child.insert((true, term), child_f2m(term));
            child.insert((false, term), child_m2f(term));
        }
        Sets {
            x002_m: cat(she_to_he(), words(&[("girl", "boy"), ("daughter", "son")])),
            x002_f: cat(he_to_she(), words(&[("boy", "girl"), ("son", "daughter")])),
            to_papa: cat(papa_words(), she_to_he()),
            to_mama: cat(mama_words(), he_to_she()),
            kana_corrin_m: papa_words(),
            kana_corrin_f: cat(vec![sub(r"P-papa", "M-mama"), sub(r"D-dad", "M-mom")], mama_words()),
            die_f: words(&[("papa", "mama"), ("daddy", "mommy"), ("dad", "mom")]),
            die_m: words(&[("mama", "papa"), ("mommy", "daddy"), ("mom", "dad")]),
            kana_m2f: kana_m2f(),
            kana_f2m: kana_f2m(),
            corrin_f2m: corrin_f2m(),
            corrin_m2f: corrin_m2f(),
            s_f2m: s_f2m(),
            s_m2f: s_m2f(),
            sib_m2f: vec![
                sub(r"little(\s+)brother", "little${1}sister"),
                sub(r"my(\s+)brother(\s+)grow", "my${1}sister${2}grow"),
                sub(r"young(\s+)man\b", "young${1}woman"),
                sub(r"We(\s+)are(\s+)brothers", "We${1}are${2}siblings"),
                sub(r"I ask him", "I ask her"),
                sub(r"He's flying away", "She's flying away"),
                sub(r"little guy", "little lady"),
            ],
            sib_f2m: vec![sub(r"my(\s+)adorable(\s+)sister", "my${1}adorable${2}brother")],
            x014: vec![sub(r"\bMother\b", "Dad"), sub(r"\bmother\b", "dad")],
            child,
            both_father: Regex::new(r"(?i)papa|\bdad(?:dy)?\b").expect("jpn2"),
            both_mother: Regex::new(r"(?i)mama|\bmom(?:my)?\b").expect("jpn2"),
        }
    })
}

// -- the tables (dev/supports/s_edits.py and make_text.py) ------------------------

const S_F2M: &[(&str, &[(&str, &str, u32)])] = &[
    ("ギュンター", &[("milady", "milord", 0), ("Lady $Nu", "Lord $Nu", 0)]),
    ("ジョーカー", &[("my lady", "my lord", 0), ("Lady $Nu", "Lord $Nu", 0), ("with her cheeks", "with his cheeks", 1), ("his mistress", "his master", 1)]),
    ("スズカゼ", &[("Lady $Nu", "Lord $Nu", 0), ("cruel mistress, milady", "cruel master, milord", 1), ("milady", "milord", 0)]),
    ("ツバキ", &[("Lady $Nu", "Lord $Nu", 0)]),
    ("サイラス", &[("into quite the lady", "into quite the gentleman", 1)]),
    ("ヒナタ", &[("Lady $Nu", "Lord $Nu", 0), ("you're a princess, aren't you?", "you're a prince, aren't you?", 1), ("lovely princess", "noble prince", 1)]),
    ("リョウマ", &[("only woman I have ever loved", "only one I have ever loved", 1), ("As your queen. As your wife.", "As your consort. As your husband.", 1)]),
    (
        "ハロルド",
        &[("milady", "milord", 0), ("Lady $Nu", "Lord $Nu", 0), ("Nay, my wife!", "Nay, my husband!", 1), ("delighted to be your wife", "delighted to be your husband", 1), ("Lady... I mean", "Lord... I mean", 1)],
    ),
    ("オーディン", &[("Lady $Nu", "Lord $Nu", 0)]),
    ("ラズワルド", &[("Lady $Nu", "Lord $Nu", 0), ("milady", "milord", 0)]),
    ("ブノワ", &[("Lady $Nu", "Lord $Nu", 0)]),
    (
        "マークス",
        &[
            ("truly brother and sister...", "truly brothers...", 1),
            ("the little sister", "the little brother", 1),
            ("brother\nand sister.", "brothers.", 1),
            ("very good sister...", "very good brother...", 1),
            ("being my wife?", "being my husband?", 1),
            ("Your wife?!", "Your husband?!", 1),
            ("as a woman...", "as a man...", 1),
            ("little\nprincess.", "little\nprince.", 1),
        ],
    ),
    ("イザナ", &[("Lady $Nu", "Lord $Nu", 0)]),
    ("ユキムラ", &[("the woman you", "the man you", 1), ("Lady $Nu", "Lord $Nu", 0), ("a princess and her advisor", "a prince and his advisor", 1)]),
    ("シグレ", &[("Lady $Nu", "Lord $Nu", 0)]),
    ("ディーア", &[("for husbands and wives?", "for a married couple?", 1)]),
    ("フォレオ", &[("my very own princess?", "my very own prince?", 1), ("on the outside, which isn't usually my type.", "on the outside, which I never expected.", 1)]),
];

const S_M2F: &[(&str, &[(&str, &str, u32)])] = &[
    ("フェリシア", &[("milord", "milady", 0), ("A prince\ncannot", "A princess\ncannot", 1), ("happiest man alive", "happiest woman alive", 1)]),
    ("サクラ", &[("not as my brother.", "not as my sister.", 1), ("you're not his son.", "you're not his daughter.", 1), ("weren't his son", "weren't his daughter", 1)]),
    ("ヒノカ", &[("you're not my brother.", "you're not my sister.", 1), ("My blood brother, anyway.", "My blood sister, anyway.", 1)]),
    ("セツナ", &[("Lord $Nu", "Lady $Nu", 0)]),
    ("オボロ", &[("Lord $Nu", "Lady $Nu", 0)]),
    ("カゲロウ", &[("Lord $Nu", "Lady $Nu", 0), ("milord", "milady", 0)]),
    ("ユウギリ", &[("milord", "milady", 0)]),
    ("エリーゼ", &[("for my hubby!", "for my wifey!", 1)]),
    ("エルフィ", &[("Lord $Nu", "Lady $Nu", 0), ("You're a prince!", "You're a princess!", 1)]),
    ("カミラ", &[("a brother marrying his sister?", "a sister marrying her sister?", 1), ("lived as brother and sister for", "lived as sisters for", 1)]),
    ("ベルカ", &[("milord", "milady", 0)]),
    ("ピエリ", &[("Lord $Nu", "Lady $Nu", 0)]),
    ("シャーロッテ", &[("Milord", "Milady", 0)]),
    ("モズメ", &[("Lord $Nu", "Lady $Nu", 0)]),
    ("ミタマ", &[("By baring his soul.", "By baring her soul.", 1)]),
    ("ゾフィー", &[("You're a great guy, $Nu.", "You're a great gal, $Nu.", 1)]),
    ("マトイ", &[("fall in love with men who", "fall in love with people who", 1)]),
    ("ベロア", &[("the best\nhusband I can", "the best\nwife I can", 1)]),
    (
        "ソレイユ",
        &[
            ("since\nI'm only a woman in your imagination.", "since\nI'm not the girl from your imagination.", 1),
            ("I like men\njust fine.", "I like girls\njust fine.", 1),
            ("if I knew him well enough.", "if I knew her well enough.", 1),
            ("not being\na cute girl?", "not being\nthat cute girl?", 1),
        ],
    ),
    (
        "エポニーヌ",
        &[("Nina...the young man you described... He's\nasking for your hand in marriage.", "Nina...that lonely soul you described... It's me.\nI'm asking for your hand in marriage.", 1)],
    ),
];

/// make_text.py CHILD_FIX, in its order (the plan names them by index)
const CHILD_FIX: &[&[(&str, &str)]] = &[
    &[
        ("He wanted to know if I had raised the\ncharming girl he'd seen", "She wanted to know if I had raised the\ncharming girl she'd seen"),
        ("He saw you help that old man, and he\nwas", "She saw you help that old man, and she\nwas"),
    ],
    &[("overheard, her work seems", "overheard, his work seems")],
];

/// make_text.py HKC: a child's lines naming the other parent, for a
/// same-gender Corrin
const HKC: &[&[(&str, &str)]] = &[
    &[("like Mom and Dad", "like Papa and Dad")],
    &[("This is how my mother\ntaught me", "This is how my dad\ntaught me")],
    &[("the same color as my\nmommy's hair", "the same color as my\npapa's hair")],
    &[("more like you than her\nmother—right down", "more like you than\nLord $Nu—right down")],
    &[("Your mother said I might find you here.", "$Nu said I might find you here.")],
    &[("I never resented you or Mother for it.", "I never resented you or Papa for it.")],
    &[("Is that how you got Mom to fall for you?", "Is that how you got Papa to fall for you?"), ("how exactly did you propose\nto her?", "how exactly did you propose\nto him?")],
    &[
        ("Was it Mom?\nOr some other girl you dated?", "Was it Papa?\nOr some girl you dated?"),
        ("I'm not so sure Mom would like you", "I'm not so sure Papa would like you"),
        ("Ha ha, she wouldn't mind at all.", "Ha ha, he wouldn't mind at all."),
    ],
    &[("than my real mother and father.", "than my real parents.")],
];

/// make_text.py HKS and HKK: My Castle's family chatter
const HKS: &[&[(&str, &str)]] = &[&[("Mama, can", "Papa, can")], &[("Papa, can", "Mama, can")]];
const HKK: &[&[(&str, &str)]] = &[&[("mother-daughter", "mother-son")], &[("mother-daughter", "mother-son")]];

fn lookup<'a>(table: &'a [(&str, &'a [(&'a str, &'a str, u32)])], key: &str) -> &'a [(&'a str, &'a str, u32)] {
    table.iter().find(|(k, _)| *k == key).map_or(&[], |(_, v)| v)
}

// -- making the lines -----------------------------------------------------------

/// what [`make`] did
#[derive(Debug, Default, Clone, Copy)]
pub struct Made {
    pub lines: usize,
    /// rows whose source line the translation (and the game) lacks
    pub no_source: usize,
    /// reviewed edits whose words the translation doesn't have
    pub edits_missed: usize,
    /// lines made the same as their source (no rule applied)
    pub same: usize,
}

fn text_of(message: &Message) -> String {
    String::from_utf16_lossy(&message.text)
}

/// Haiku's lines in English, by archive (RomFS path, as the plan writes
/// it), made from `read`: the archive at a RomFS path as the game reads it
/// under the mods (the translation's, else the game's own).
pub fn make(read: &mut dyn FnMut(&str) -> Option<std::sync::Arc<Vec<Message>>>) -> (HashMap<&'static str, Vec<Message>>, Made) {
    let sets = sets();
    let kana_label = hex(KANA_NAME_LABEL);
    let kana_name = read("m/GameData.bin.lz")
        .and_then(|messages| messages.iter().find(|m| m.label == kana_label).map(text_of))
        .filter(|name| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphabetic() || c == ' ' || c == '-'))
        .unwrap_or_else(|| "Kana".to_string());
    let mut out: HashMap<&'static str, Vec<Message>> = HashMap::new();
    let mut made = Made::default();
    for row in plan() {
        let text = if row.src.is_empty() {
            None
        } else {
            match read(row.src).and_then(|messages| messages.iter().find(|m| m.label == row.source).map(text_of)) {
                Some(text) => Some(text),
                None => {
                    made.no_source += 1;
                    continue;
                }
            }
        };
        let src = text.as_deref().unwrap_or("");
        let mut missed = 0;
        let mut p = row.param.split('|');
        let (a, b, c) = (p.next().unwrap_or(""), p.next().unwrap_or(""), p.next().unwrap_or(""));
        let new = match row.part {
            // the question names Kana as the translation does (Team If and
            // the Uncensored Patch write "Kanna")
            "ask" if a == "0" => ASK[0].replace("Kana", &kana_name),
            "s_supports" => {
                let (table, generic) = if a == "F2M" { (S_F2M, &sets.s_f2m) } else { (S_M2F, &sets.s_m2f) };
                partner_lines(&nu(&edits(src, lookup(table, b), &mut missed), a == "F2M"), generic)
            }
            "x002" => match a {
                "M" => apply(&raw_replace(src, KANA_CODES_F2M), &sets.x002_m),
                "F" => apply(&raw_replace(src, KANA_CODES_M2F), &sets.x002_f),
                "dieF" => apply(src, &sets.die_f),
                _ => apply(src, &sets.die_m),
            },
            "kana_corrin" => apply(src, if a == "M" { &sets.kana_corrin_m } else { &sets.kana_corrin_f }),
            "kana_lines" => {
                // both parents named: right for either Kana
                let plain = plain(src);
                if sets.both_father.is_match(&plain) && sets.both_mother.is_match(&plain) {
                    src.to_string()
                } else {
                    apply(src, if a == "M" { &sets.to_papa } else { &sets.to_mama })
                }
            }
            "ask" => a.parse::<usize>().ok().and_then(|i| ASK.get(i)).map_or(String::new(), |s| s.to_string()),
            "kana_parent" => {
                let rules: Vec<&Rule> = match a {
                    "A" => sets.kana_m2f.iter().collect(),
                    "C" => sets.kana_m2f.iter().chain(&sets.corrin_f2m).collect(),
                    "B" => sets.corrin_f2m.iter().collect(),
                    "D" => sets.kana_f2m.iter().collect(),
                    "F" => sets.kana_f2m.iter().chain(&sets.corrin_m2f).collect(),
                    _ => sets.corrin_m2f.iter().collect(),
                };
                // Corrin's title for the variants where Corrin's gender changes
                let src = match a {
                    "B" | "C" => nu(src, true),
                    "E" | "F" => nu(src, false),
                    _ => src.to_string(),
                };
                let new = apply_refs(&src, &rules);
                match b {
                    "M2F" => raw_replace(&new, KANA_CODES_M2F),
                    "F2M" => raw_replace(&new, KANA_CODES_F2M),
                    _ => new,
                }
            }
            "child_corrin" => {
                let term: &'static str = match b {
                    "Papa" => "Papa",
                    "Mom" => "Mom",
                    _ => "Dad",
                };
                let src = nu(src, a == "F2M");
                let mut new = sets.child.get(&(a == "F2M", term)).map_or_else(|| src.clone(), |rules| apply(&src, rules));
                if let Some(fixes) = c.parse::<usize>().ok().and_then(|i| CHILD_FIX.get(i)) {
                    for (old, rep) in fixes.iter() {
                        if new.contains(old) {
                            new = new.replace(old, rep);
                        } else {
                            missed += 1;
                        }
                    }
                }
                new
            }
            "x014" => apply(src, &sets.x014),
            "siblings" => {
                if a == "M2F" {
                    apply(&raw_replace(src, KANA_CODES_M2F), &sets.sib_m2f)
                } else {
                    apply(&raw_replace(src, KANA_CODES_F2M), &sets.sib_f2m)
                }
            }
            "mark" => String::new(),
            "hkc" => a.parse::<usize>().ok().and_then(|i| HKC.get(i)).map_or_else(|| src.to_string(), |list| pair_edits(src, list, &mut missed)),
            "chatter" => {
                let table = if a.starts_with('S') { HKS } else { HKK };
                a[1..].parse::<usize>().ok().and_then(|i| table.get(i)).map_or_else(|| src.to_string(), |list| pair_edits(src, list, &mut missed))
            }
            _ => continue,
        };
        made.edits_missed += missed;
        if !row.src.is_empty() && new == src {
            made.same += 1;
        }
        made.lines += 1;
        out.entry(row.dst).or_default().push(Message { label: row.label.clone(), text: new.encode_utf16().collect() });
    }
    out.entry("m/MyUnitEdit.bin.lz").or_default().push(Message { label: hex(SUNGLASSES_LABEL), text: "Sunglasses".encode_utf16().collect() });
    (out, made)
}

fn apply_refs(text: &str, rules: &[&Rule]) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut at = 0;
    let piece = |piece: &str| {
        let mut piece = piece.to_string();
        for r in rules {
            let replaced = r.re.replace_all(&piece, |c: &Captures| (r.rep)(c, &piece)).into_owned();
            piece = replaced;
        }
        piece
    };
    for m in codes().find_iter(text) {
        if m.start() > at {
            out.push_str(&piece(&text[at..m.start()]));
        }
        out.push_str(m.as_str());
        at = m.end();
    }
    if at < text.len() {
        out.push_str(&piece(&text[at..]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_follow_regender() {
        let s = sets();
        assert_eq!(apply("She gave her the book and she's happy for her.", &she_to_he()), "He gave him the book and he's happy for him.");
        assert_eq!(apply("Her sword is hers.", &she_to_he()), "His sword is his.");
        assert_eq!(apply("I saw him with his sword. It's his.", &he_to_she()), "I saw her with her sword. It's hers.");
        assert_eq!(apply("$Wsカンナ女|$WaMama! Mom, look!", &s.to_papa), "$Wsカンナ女|$WaPapa! Dad, look!");
        assert_eq!(partner_lines(&nu("$Wsリョウマ|$WaLady $Nu, milady. As a woman.$k$Wsusername|$WaA woman?", true), &s.s_f2m), "$Wsリョウマ|$WaLord $Nu, milord. As a man.$k$Wsusername|$WaA woman?");
        assert_eq!(apply("M-Mother...", &child_f2m("Papa")), "P-Papa...");
        assert_eq!(case_like("MOM", "dad"), "DAD");
        assert_eq!(apply("She’s here.", &she_to_he()), "He’s here.");
    }

    #[test]
    fn the_plan_reads() {
        assert!(plan().len() > 1200);
        assert!(plan().iter().all(|r| !r.label.is_empty()));
    }
}
