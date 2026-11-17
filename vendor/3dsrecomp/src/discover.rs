//! finding the code in a program. functions are found from the entry points
//! by following calls and branches, and from words elsewhere that point into
//! the code, which is where vtables and callbacks live.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::arm::{self, Flow, Instruction};
use crate::image::Segment;
use crate::thumb;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    Entry,
    Call,
    /// a code address stored in a literal pool or in the data segments.
    Pointer,
    /// a symbol the program exports to other modules.
    Export,
    /// a code address a module's relocations store, in its vtables, jump
    /// tables and literal pools.
    Relocation,
    /// a code address another module takes from this one without a name.
    Import,
    /// the start of a gap nothing led to, or a push in one, which is how
    /// functions reached only through tables nobody can see begin.
    Scan,
    /// an address a function written by hand replaces, which its author
    /// says is code.
    Override,
    /// an address Zakuro had to interpret with the title's last library,
    /// code nothing else led to.
    Hint,
    /// coverage: a place the code goes to directly, falling through,
    /// branching or calling, that no function had as an entry. see
    /// close_gaps.
    Gap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Mode {
    Arm,
    Thumb,
}

/// the bit of Discovery::modes for code in mode.
fn mode_bit(mode: Mode) -> u8 {
    match mode {
        Mode::Arm => 1,
        Mode::Thumb => 2,
    }
}

/// coverage: the bits of Discovery::modes saying what made a byte a literal,
/// a load in ARM code, a load in Thumb code, or the relocations.
const LITERAL_ARM: u8 = 4;
const LITERAL_THUMB: u8 = 8;
const LITERAL_SLOT: u8 = 16;
const LITERAL_ANY: u8 = LITERAL_ARM | LITERAL_THUMB | LITERAL_SLOT;

/// coverage: the bit of Discovery::modes for a literal code in mode loads.
fn literal_bit(mode: Mode) -> u8 {
    match mode {
        Mode::Arm => LITERAL_ARM,
        Mode::Thumb => LITERAL_THUMB,
    }
}

/// coverage: how many rounds close_gaps takes at most. each round makes
/// entries of the gaps the last one left, the code there can lead on to
/// more, and every title looked at needs a handful.
const CLOSING_ROUNDS: usize = 64;

impl Mode {
    fn other(self) -> Mode {
        match self {
            Mode::Arm => Mode::Thumb,
            Mode::Thumb => Mode::Arm,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Function {
    pub source: Source,
    pub mode: Mode,
    /// the address of every instruction, in order.
    pub instructions: Vec<u32>,
    /// where execution can enter the function's code, its entry, the
    /// targets of its branches and the instructions after calls and svc.
    pub labels: BTreeSet<u32>,
}

/// what each byte of the text segment turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Byte {
    Unknown,
    Code,
    Literal,
}

pub struct Analysis {
    pub functions: BTreeMap<u32, Function>,
    pub map: Vec<Byte>,
    pub indirect_sites: usize,
    pub jump_tables: usize,
    pub svc_sites: usize,
    /// paths abandoned because they ran into something that cannot be code.
    pub dead_ends: usize,
    /// cpu: jumps and calls through registers whose target the pc gave.
    pub computed_jumps: usize,
    /// cpu: labels added where the code computes an address from the pc.
    pub computed_labels: usize,
    /// cpu: labels added where a pointer in data or a literal pool points.
    pub pointer_labels: usize,
    /// coverage: what close_gaps did, the labels it added where the code
    /// goes into code a function holds, the functions it made where no
    /// function held the code, the guesses in the other mode it took out
    /// for them, and the gaps left, places that are no instruction.
    pub gap_labels: usize,
    pub gap_functions: usize,
    pub gap_evictions: usize,
    pub gaps_left: usize,
}

impl Analysis {
    pub fn count(&self, kind: Byte) -> usize {
        self.map.iter().filter(|&&byte| byte == kind).count()
    }

    /// the longest runs of text nobody reached, as (start, length).
    pub fn largest_gaps(&self, base: u32, count: usize) -> Vec<(u32, usize)> {
        let mut gaps = Vec::new();
        let mut start = None;
        for (offset, &byte) in self.map.iter().chain(std::iter::once(&Byte::Code)).enumerate() {
            match (byte == Byte::Unknown, start) {
                (true, None) => start = Some(offset),
                (false, Some(s)) => {
                    gaps.push((base + s as u32, offset - s));
                    start = None;
                }
                _ => {}
            }
        }
        gaps.sort_by_key(|gap| std::cmp::Reverse(gap.1));
        gaps.truncate(count);
        gaps
    }
}

/// a piece of code to analyze and what is known about it up front.
pub struct Program {
    pub text: Segment,
    /// addresses that may start functions and how each is known, odd for
    /// Thumb.
    pub seeds: Vec<(u32, Source)>,
    /// the words in the code that hold addresses, when relocations say
    /// exactly which ones they are. without them any literal that lands in
    /// the code is taken for a pointer.
    pub slots: Option<BTreeSet<u32>>,
}

/// instructions past which a function grew by following tail calls into
/// other functions rather than by what one function holds. compilers make
/// few near it, Monster Hunter 3 Ultimate's tail calls led one function
/// through 62,000 instructions of others, which a C compiler then spent
/// most of a build on.
const HUGE: usize = 10_000;

/// how far a branch of a huge function goes before it is taken for a tail
/// call, a function's own branches stay closer.
const FAR: u32 = 0x4000;

/// the shortest code a guess leads to that is judged on whether it looks
/// like code, for ARM and for Thumb. shorter is cheap to keep either way.
const JUDGED_ARM: usize = 16;
const JUDGED_THUMB: usize = 9;

/// how much of a guess's ARM code may be conditional, four fifths, over all
/// of it and over its first instructions from the entry. what a compiler
/// made stays under it in every title looked at, other bytes taken for ARM
/// have a condition in whatever their top four bits are, and code that is
/// really Thumb or text is nearly all of it conditional.
const MOST_CONDITIONAL: usize = 4;
const ARM_START: usize = 32;

/// how many of a guess's first Thumb instructions are looked at for a push, a
/// branch, a call or a return, which text taken for Thumb never has.
const THUMB_FLOW_WITHIN: usize = 64;

/// somewhere to explore, how it was found, and whether something sure led
/// there rather than a guess.
type Lead = (u32, Mode, Source, bool);

/// what exploring a guess did, held until it turns out to be code: the bytes
/// it marked, with what each was before, where it leads, where it joined
/// other code, and the tail calls split_huge noted on the way.
#[derive(Default)]
struct Trial {
    marked: Vec<(usize, Byte, u8)>,
    leads: Vec<Lead>,
    joins: Vec<(u32, Mode)>,
    entries: Vec<u32>,
}

struct Discovery<'a> {
    text: &'a Segment,
    slots: Option<&'a BTreeSet<u32>>,
    analysis: Analysis,
    /// function entries a branch goes to as a tail call rather than as
    /// more of the function, while huge ones are explored again.
    entries: Option<BTreeSet<u32>>,
    queue: VecDeque<Lead>,
    /// pointers found in data, followed only once nothing surer is left.
    guesses: VecDeque<Lead>,
    /// whether the function being explored came from a guess, so that what
    /// it leads to is only a guess too.
    guessing: bool,
    /// set while a guess is explored for the first time.
    trial: Option<Trial>,
    /// the guesses that came to nothing, by entry and mode.
    rejected: BTreeSet<(u32, Mode)>,
    /// the functions that came from guesses.
    guessed: BTreeSet<u32>,
    /// for each byte of the text, whether some function decoded it as ARM
    /// code, bit 0, and as Thumb code, bit 1.
    modes: Vec<u8>,
    /// where guesses ran into code other functions hold, or pointed into it,
    /// which become labels of those functions, so that the host goes on there
    /// in recompiled code, and places split_huge may cut at.
    joins: BTreeSet<(u32, Mode)>,
    /// where the paths of a function that came from a guess stop, because
    /// the code there is another function's, kept for exploring it again.
    cuts: BTreeMap<u32, BTreeSet<u32>>,
    /// coverage: set while close_gaps explores, which goes on through bytes
    /// taken for literals: the code goes there, so the CPU would run them.
    forcing: bool,
}

pub fn analyze(program: &Program) -> Analysis {
    let mut discovery = Discovery {
        text: &program.text,
        slots: program.slots.as_ref(),
        analysis: Analysis {
            functions: BTreeMap::new(),
            map: vec![Byte::Unknown; program.text.bytes.len()],
            indirect_sites: 0,
            jump_tables: 0,
            svc_sites: 0,
            dead_ends: 0,
            computed_jumps: 0,
            computed_labels: 0,
            pointer_labels: 0,
            gap_labels: 0,
            gap_functions: 0,
            gap_evictions: 0,
            gaps_left: 0,
        },
        entries: None,
        queue: VecDeque::new(),
        guesses: VecDeque::new(),
        guessing: false,
        trial: None,
        rejected: BTreeSet::new(),
        guessed: BTreeSet::new(),
        modes: vec![0; program.text.bytes.len()],
        joins: BTreeSet::new(),
        cuts: BTreeMap::new(),
        forcing: false,
    };
    // where relocations say the code holds data, it cannot be code
    for &slot in program.slots.iter().flatten() {
        discovery.mark_literal(slot, 4, LITERAL_SLOT);
    }
    for &(address, source) in &program.seeds {
        discovery.code_pointer(address, source);
    }
    discovery.run();
    // once nothing surer is left, the gaps get looked at for how functions
    // begin, each round of them can lead to more
    loop {
        let starts = discovery.gap_starts();
        if starts.is_empty() {
            break;
        }
        discovery.queue.extend(starts.into_iter().map(|address| (address, Mode::Arm, Source::Scan, false)));
        discovery.run();
    }
    discovery.split_huge();
    discovery.label_joins();
    discovery.label_computed();
    let pointers: Vec<u32> = program.seeds.iter().filter(|(_, source)| *source == Source::Pointer).map(|&(value, _)| value).collect();
    discovery.label_pointers(&pointers);
    // coverage: whatever the code goes to directly is an entry
    if discovery.close_gaps() {
        // the code close_gaps added, labelled the same way
        discovery.label_joins();
        discovery.label_computed();
        discovery.label_pointers(&pointers);
    }
    discovery.analysis
}

impl Discovery<'_> {
    /// explores the huge functions again, now that every function is known,
    /// their branches to other functions' entries and far away going to
    /// functions there as tail calls instead of taking their code in, and
    /// the functions those are explored the same way.
    fn split_huge(&mut self) {
        let huge: Vec<(u32, Mode)> =
            self.analysis.functions.iter().filter(|(_, f)| f.instructions.len() > HUGE).map(|(&entry, f)| (entry, f.mode)).collect();
        if huge.is_empty() {
            return;
        }
        // what exploring counts was counted the first time
        let counts = self.counts();
        // and where guesses showed other code begins
        self.entries = Some(self.analysis.functions.keys().copied().chain(self.joins.iter().map(|&(address, _)| address)).collect());
        let mut huge = huge;
        for _ in 0..4 {
            for &(entry, mode) in &huge {
                self.guessing = self.guessed.contains(&entry);
                let (instructions, labels) = self.explore(entry, mode);
                self.guessing = false;
                if let Some(function) = self.analysis.functions.get_mut(&entry) {
                    function.instructions = instructions;
                    function.labels = labels;
                }
            }
            // the functions the tail calls lead to, explored the same way
            self.run();
            let still: Vec<(u32, Mode)> =
                self.analysis.functions.iter().filter(|(_, f)| f.instructions.len() > HUGE).map(|(&entry, f)| (entry, f.mode)).collect();
            if still.is_empty() || still == huge {
                break;
            }
            huge = still;
        }
        self.entries = None;
        self.set_counts(counts);
    }

    fn run(&mut self) {
        while let Some((entry, mode, source, sure)) = self.queue.pop_front().or_else(|| self.guesses.pop_front()) {
            if self.analysis.functions.contains_key(&entry) {
                continue;
            }
            // a guess is tried before it counts, but the pieces a huge
            // function is split into, the tail calls it is cut at, are its
            // own code, already taken
            let piece = self.entries.as_ref().is_some_and(|entries| entries.contains(&entry));
            let judged = !sure && !piece;
            // a guess into a literal is data, and one into code a function
            // holds in the same mode would only copy that code from there
            // on. code in the other mode is left to looks_like_code, as text
            // or a wrong guess may have been taken for it
            let held = self.holds(entry, mode);
            let taken = self.stops_at_literal(entry, mode) || held;
            if judged && (taken || self.rejected.contains(&(entry, mode))) {
                if held {
                    // in case the pointer is real, the host can go on there
                    self.joins.insert((entry, mode));
                }
                self.rejected.insert((entry, mode));
                continue;
            }
            self.guessing = !sure;
            self.trial = judged.then(Trial::default);
            let counts = self.counts();
            let (instructions, labels) = self.explore(entry, mode);
            self.guessing = false;
            if let Some(trial) = self.trial.take() {
                if !self.looks_like_code(entry, mode, &instructions, !trial.joins.is_empty()) {
                    // not code, so what exploring it marked and counted goes
                    // back, and where it led is never looked at
                    for (offset, byte, modes) in trial.marked.into_iter().rev() {
                        self.analysis.map[offset] = byte;
                        self.modes[offset] = modes;
                    }
                    if let Some(entries) = self.entries.as_mut() {
                        for target in trial.entries {
                            entries.remove(&target);
                        }
                    }
                    self.set_counts(counts);
                    self.rejected.insert((entry, mode));
                    continue;
                }
                for lead in trial.leads {
                    self.follow(lead);
                }
                if !trial.joins.is_empty() {
                    self.cuts.entry(entry).or_default().extend(trial.joins.iter().map(|&(address, _)| address));
                }
                self.joins.extend(trial.joins);
            }
            if !sure {
                self.guessed.insert(entry);
            }
            self.analysis.functions.insert(entry, Function { source, mode, instructions, labels });
        }
    }

    /// whether what a guess led to looks like code. ARM code a compiler made
    /// runs most of its instructions unconditionally, and Thumb code pushes,
    /// branches, calls or returns within its first few dozen instructions,
    /// or goes on into code another function holds, while text taken for
    /// Thumb is all loads, stores, moves and arithmetic.
    fn looks_like_code(&self, entry: u32, mode: Mode, instructions: &[u32], joins: bool) -> bool {
        let from_entry = instructions.partition_point(|&address| address < entry);
        match mode {
            Mode::Arm => {
                // 0xF counts too: blx to Thumb and pld have it there, but so
                // do the halves of Thumb's bl taken for ARM, far more often
                let mostly_unconditional = |instructions: &[u32]| {
                    let conditional = instructions
                        .iter()
                        .filter(|&&address| self.text.read32(address).is_some_and(|word| word >> 28 != arm::ALWAYS))
                        .count();
                    conditional * (MOST_CONDITIONAL + 1) <= instructions.len() * MOST_CONDITIONAL
                };
                let start = &instructions[from_entry..(from_entry + ARM_START).min(instructions.len())];
                instructions.len() < JUDGED_ARM || (mostly_unconditional(instructions) && (start.len() < JUDGED_ARM || mostly_unconditional(start)))
            }
            Mode::Thumb => {
                instructions.len() < JUDGED_THUMB
                    || joins
                    || instructions[from_entry..].iter().take(THUMB_FLOW_WITHIN).any(|&address| {
                        let push = self.text.read16(address).is_some_and(|halfword| halfword & 0xFE00 == 0xB400 || halfword == 0xE92D);
                        let flows = self.decode(address, Mode::Thumb).is_some_and(|(instruction, _)| {
                            !matches!(instruction.flow, Flow::Other | Flow::LiteralLoad { .. } | Flow::Undefined)
                        });
                        push || flows
                    })
            }
        }
    }

    /// coverage: makes every place the code of the functions goes to
    /// directly an entry, whatever left it out, a literal mark in the way, a
    /// start that cut a function short, a guess in the other mode at the
    /// same address, so that recompiled code never goes where the host finds
    /// nothing: a place some function holds in that mode becomes a label of
    /// the functions holding it, and anywhere else a function starts, its
    /// code followed through literals and up to code other functions hold,
    /// which the next round makes a label. what is left is no instruction in
    /// that mode, an undefined encoding or the middle of one, which the CPU
    /// could not run either. returns whether it changed anything.
    fn close_gaps(&mut self) -> bool {
        let mut changed = false;
        for _ in 0..CLOSING_ROUNDS {
            let found = gaps(self.text, &self.analysis);
            if found.is_empty() {
                break;
            }
            let targets: BTreeSet<(u32, Mode)> = found.iter().map(|gap| (gap.to, gap.mode)).collect();
            let mut progress = false;
            // held in that mode: a label of each function holding it
            let mut held = BTreeSet::new();
            for (&entry, function) in self.analysis.functions.iter_mut() {
                if !function.labels.contains(&entry) {
                    continue;
                }
                let hits: Vec<u32> =
                    function.instructions.iter().copied().filter(|&at| targets.contains(&(at, function.mode))).collect();
                for at in hits {
                    held.insert((at, function.mode));
                    if function.labels.insert(at) {
                        self.analysis.gap_labels += 1;
                        progress = true;
                    }
                }
            }
            // held by none: a function of its own
            for &(to, mode) in targets.difference(&held) {
                let instruction = self.text.contains(to).then(|| self.decode(to, mode)).flatten();
                if instruction.is_none_or(|(instruction, _)| instruction.flow == Flow::Undefined) {
                    continue;
                }
                if let Some(existing) = self.analysis.functions.get(&to) {
                    // a guess in the other mode at the same address gives way
                    // to code that is gone to; one that is no guess is left,
                    // and so is a function that close_gaps made
                    let guess = self.guessed.contains(&to) || matches!(existing.source, Source::Pointer | Source::Scan);
                    if existing.mode != mode && (!guess || existing.source == Source::Gap) {
                        continue;
                    }
                    if existing.mode != mode {
                        self.analysis.gap_evictions += 1;
                    }
                }
                self.forcing = true;
                self.guessing = false;
                self.trial = Some(Trial::default());
                let (instructions, labels) = self.explore(to, mode);
                self.forcing = false;
                let trial = self.trial.take().unwrap_or_default();
                if !labels.contains(&to) {
                    continue;
                }
                // pointers its literals hold are guesses as ever; its calls
                // are gaps of the next round, which joins them to code
                // already held the same way
                for lead in trial.leads {
                    if lead.2 == Source::Pointer {
                        self.follow(lead);
                    }
                }
                self.joins.extend(trial.joins);
                self.guessed.remove(&to);
                self.analysis.functions.insert(to, Function { source: Source::Gap, mode, instructions, labels });
                self.analysis.gap_functions += 1;
                progress = true;
            }
            if !progress {
                break;
            }
            changed = true;
            self.run();
            self.label_joins();
        }
        self.analysis.gaps_left = gaps(self.text, &self.analysis).len();
        changed
    }

    /// makes each place a guess joined other code a label of the functions
    /// that hold the code there.
    fn label_joins(&mut self) {
        if self.joins.is_empty() {
            return;
        }
        let joins = std::mem::take(&mut self.joins);
        for function in self.analysis.functions.values_mut() {
            for &address in &function.instructions {
                if joins.contains(&(address, function.mode)) {
                    function.labels.insert(address);
                }
            }
        }
    }

    /// cpu: makes every address the code computes from the pc (add/sub rd,
    /// pc, #imm in ARM, adr in Thumb) a label of the functions holding the
    /// code there, in the mode the address says (odd for Thumb), so that a
    /// callback or a jump target made that way has an entry wherever it is
    /// used. an address of data holds no code and adds nothing.
    fn label_computed(&mut self) {
        let mut targets = Vec::new();
        for function in self.analysis.functions.values() {
            for &address in &function.instructions {
                let target = match function.mode {
                    Mode::Arm => self.text.read32(address).and_then(|word| pc_relative_arm(word, address)),
                    Mode::Thumb => self.text.read16(address).and_then(|halfword| {
                        (halfword & 0xF800 == 0xA000).then(|| ((address + 4) & !3) + (halfword as u32 & 0xFF) * 4)
                    }),
                };
                // Thumb code takes the address adr makes and sets bit 0, so
                // the instruction there in either mode may be what it is for
                if let Some(target) = target.filter(|t| self.text.contains(t & !1)) {
                    targets.push(target);
                    targets.push(target | 1);
                }
            }
        }
        if targets.is_empty() {
            return;
        }
        let wanted: std::collections::HashSet<u32> = targets.into_iter().collect();
        let mut added = 0;
        for function in self.analysis.functions.values_mut() {
            let thumb = (function.mode == Mode::Thumb) as u32;
            let hits: Vec<u32> =
                function.instructions.iter().copied().filter(|&at| wanted.contains(&(at | thumb))).collect();
            for at in hits {
                if function.labels.insert(at) {
                    added += 1;
                }
            }
        }
        self.analysis.computed_labels += added;
    }

    /// cpu: makes every code address a pointer holds, in the data segments
    /// or in the literal pools of the code, a label of the functions holding
    /// an instruction there in the mode the pointer says, so that a call or
    /// jump through any table of code pointers lands on an entry. a value
    /// that only looks like a pointer adds a harmless label at most.
    fn label_pointers(&mut self, data: &[u32]) {
        let mut wanted: std::collections::HashSet<u32> = data.iter().copied().collect();
        for (offset, chunk) in self.text.bytes.chunks_exact(4).enumerate() {
            let offset = offset * 4;
            if self.analysis.map[offset..offset + 4].iter().all(|&byte| byte == Byte::Literal) {
                wanted.insert(u32::from_le_bytes(chunk.try_into().unwrap()));
            }
        }
        wanted.retain(|&value| self.text.contains(value & !1) && (value & 1 != 0 || value & 3 == 0));
        let mut added = 0;
        for function in self.analysis.functions.values_mut() {
            let thumb = (function.mode == Mode::Thumb) as u32;
            let hits: Vec<u32> =
                function.instructions.iter().copied().filter(|&at| wanted.contains(&(at | thumb))).collect();
            for at in hits {
                if function.labels.insert(at) {
                    added += 1;
                }
            }
        }
        self.analysis.pointer_labels += added;
    }

    /// what exploring counts, to put back after a guess that came to nothing.
    fn counts(&self) -> (usize, usize, usize, usize) {
        (self.analysis.indirect_sites, self.analysis.jump_tables, self.analysis.svc_sites, self.analysis.dead_ends)
    }

    fn set_counts(&mut self, counts: (usize, usize, usize, usize)) {
        (self.analysis.indirect_sites, self.analysis.jump_tables, self.analysis.svc_sites, self.analysis.dead_ends) = counts;
    }

    fn mark(&mut self, address: u32, size: u32, kind: Byte) {
        for byte in address..address + size {
            if let Some(offset) = byte.checked_sub(self.text.base) {
                if let Some(slot) = self.analysis.map.get_mut(offset as usize) {
                    // code wins over a literal guess, but code close_gaps
                    // goes through leaves the literals it runs over as they
                    // were, for what reads them
                    if *slot != Byte::Code && !(self.forcing && *slot == Byte::Literal) {
                        if let Some(trial) = &mut self.trial {
                            trial.marked.push((offset as usize, *slot, self.modes[offset as usize]));
                        }
                        *slot = kind;
                    }
                }
            }
        }
    }

    /// coverage: marks a literal that code in a mode loads, or the
    /// relocations name, by is its LITERAL_ bit.
    fn mark_literal(&mut self, address: u32, size: u32, by: u8) {
        self.mark(address, size, Byte::Literal);
        for byte in address..address + size {
            let Some(offset) = byte.checked_sub(self.text.base).map(|offset| offset as usize) else { continue };
            if offset < self.modes.len() && self.analysis.map[offset] == Byte::Literal && self.modes[offset] & by == 0 {
                if let Some(trial) = &mut self.trial {
                    trial.marked.push((offset, self.analysis.map[offset], self.modes[offset]));
                }
                self.modes[offset] |= by;
            }
        }
    }

    /// coverage: whether code in mode stops at a literal at address. ARM
    /// code does not stop at a literal only Thumb code loads: ARM code and
    /// text read as Thumb far more often than a literal pool of Thumb code
    /// sits in the way of ARM code, since a guess in ARM has to be mostly
    /// unconditional while one in Thumb only has to branch somewhere, and
    /// such Thumb guesses cut ARM functions short (zlib's deflate_fast,
    /// castle::MaterialWindow::Draw). Thumb code stops at every literal, as
    /// it did: that way round the guess in Thumb is the likely one to be
    /// wrong. close_gaps goes on through every literal.
    fn stops_at_literal(&self, address: u32, mode: Mode) -> bool {
        if self.forcing || self.byte(address) != Byte::Literal {
            return false;
        }
        let made = self.modes[(address - self.text.base) as usize] & LITERAL_ANY;
        mode == Mode::Thumb || made == 0 || made & (LITERAL_SLOT | LITERAL_ARM) != 0
    }

    fn byte(&self, address: u32) -> Byte {
        self.analysis.map[(address - self.text.base) as usize]
    }

    /// marks the bytes of an instruction as code, decoded in mode.
    fn mark_code(&mut self, address: u32, size: u32, mode: Mode) {
        self.mark(address, size, Byte::Code);
        let bit = mode_bit(mode);
        for byte in address..address + size {
            if let Some(offset) = byte.checked_sub(self.text.base).map(|offset| offset as usize).filter(|&offset| offset < self.modes.len()) {
                if self.modes[offset] & bit == 0 {
                    if let Some(trial) = &mut self.trial {
                        trial.marked.push((offset, self.analysis.map[offset], self.modes[offset]));
                    }
                    self.modes[offset] |= bit;
                }
            }
        }
    }

    /// whether some function holds the code at address in mode.
    fn holds(&self, address: u32, mode: Mode) -> bool {
        self.modes[(address - self.text.base) as usize] & mode_bit(mode) != 0
    }

    /// likely ARM functions in the text nothing reached, the first word of
    /// each gap when it is an unconditional instruction, and every push of
    /// registers onto the stack. Thumb is left alone, too much data looks
    /// like it.
    fn gap_starts(&self) -> Vec<u32> {
        let mut starts = Vec::new();
        let mut in_gap = false;
        let mut address = self.text.base.next_multiple_of(4);
        while let Some(word) = self.text.read32(address) {
            let offset = (address - self.text.base) as usize;
            let unknown = self.analysis.map[offset..offset + 4].iter().all(|&byte| byte == Byte::Unknown);
            if unknown && !self.analysis.functions.contains_key(&address) && !self.rejected.contains(&(address, Mode::Arm)) {
                let instruction = arm::decode(word, address);
                let push = word & 0xFFFF_0000 == 0xE92D_0000 && word & 0xFFFF != 0;
                let starts_gap = !in_gap && instruction.condition == arm::ALWAYS && instruction.flow != Flow::Undefined;
                if push || starts_gap {
                    starts.push(address);
                }
            }
            in_gap = unknown;
            address += 4;
        }
        starts
    }

    /// whether the word at address may hold a code address.
    fn may_point(&self, address: u32) -> bool {
        self.slots.is_none_or(|slots| slots.contains(&address))
    }

    /// a value that may be the address of a function, odd for Thumb.
    fn code_pointer(&mut self, value: u32, source: Source) {
        let (address, mode) = if value & 1 != 0 {
            (value & !1, Mode::Thumb)
        } else if value & 3 == 0 {
            (value, Mode::Arm)
        } else {
            return;
        };
        if self.text.contains(address) && !self.analysis.functions.contains_key(&address) {
            self.lead(address, mode, source);
        }
    }

    fn call(&mut self, target: u32, mode: Mode) {
        if self.text.contains(target) && !self.analysis.functions.contains_key(&target) {
            self.lead(target, mode, Source::Call);
        }
    }

    /// somewhere the function being explored leads. what a guess leads to is
    /// only a guess too, and waits for the guess to turn out to be code.
    fn lead(&mut self, address: u32, mode: Mode, source: Source) {
        let lead = (address, mode, source, !self.guessing && source != Source::Pointer);
        match &mut self.trial {
            Some(trial) => trial.leads.push(lead),
            None => self.follow(lead),
        }
    }

    /// queues a lead, pointers behind everything surer.
    fn follow(&mut self, lead: Lead) {
        match lead.2 {
            Source::Pointer => self.guesses.push_back(lead),
            _ => self.queue.push_back(lead),
        }
    }

    fn decode(&self, address: u32, mode: Mode) -> Option<(Instruction, u32)> {
        match mode {
            Mode::Arm => Some((arm::decode(self.text.read32(address)?, address), 4)),
            Mode::Thumb => {
                let halfword = self.text.read16(address)?;
                Some(thumb::decode(halfword, self.text.read16(address + 2), address))
            }
        }
    }

    /// follows every path through one function, returning its instructions
    /// and labels.
    fn explore(&mut self, entry: u32, mode: Mode) -> (Vec<u32>, BTreeSet<u32>) {
        // where a guess stopped the first time, which it does again
        let cuts = self.cuts.get(&entry).cloned().unwrap_or_default();
        let mut blocks = vec![entry];
        let mut seen = BTreeSet::new();
        let mut code = BTreeSet::new();
        let mut labels = BTreeSet::new();

        while let Some(start) = blocks.pop() {
            labels.insert(start);
            let mut address = start;
            // cpu: registers holding addresses computed from the pc on the
            // way through this block, for jumps through them
            let mut known = Known::default();
            while self.text.contains(address) && seen.insert(address) {
                if self.stops_at_literal(address, mode) {
                    // ran into a literal pool, the path is wrong or it ended
                    break;
                }
                if cuts.contains(&address) {
                    break;
                }
                if self.holds(address, mode) && !code.contains(&address) {
                    if let Some(trial) = &mut self.trial {
                        // a guess that runs into code another function holds
                        // in the same mode joins that code, which is there
                        // already
                        trial.joins.push((address, mode));
                        break;
                    }
                }
                let Some((instruction, size)) = self.decode(address, mode) else { break };
                if instruction.flow == Flow::Undefined {
                    self.analysis.dead_ends += 1;
                    break;
                }
                self.mark_code(address, size, mode);
                code.insert(address);
                let conditional = instruction.is_conditional();
                // add lr, pc, #n and mov lr, pc before a branch make a call by
                // hand, which comes back where lr points
                if let Some(back) = (mode == Mode::Arm).then(|| self.text.read32(address).and_then(|word| link_return(word, address))).flatten() {
                    if self.text.contains(back) && back > address && back - address <= FAR {
                        blocks.push(back);
                    }
                }
                // cpu: a jump or call through a register that holds an address
                // computed from the pc (add rX, pc, #a; add rX, rX, #b; mov pc,
                // rX, or add r12, pc, #1; bx r12 into Thumb, or Thumb bx pc)
                // goes somewhere known, which has to be an entry
                let computed = match mode {
                    Mode::Arm => self.text.read32(address).and_then(|word| known.arm(word, address)),
                    Mode::Thumb => self.text.read16(address).and_then(|halfword| known.thumb(halfword, address)),
                };
                if let Some((target, target_mode, link)) = computed {
                    let at = target & !1;
                    let aligned = target_mode == Mode::Thumb || at & 3 == 0;
                    if self.text.contains(at) && aligned {
                        self.analysis.computed_jumps += 1;
                        if target_mode == mode && !link && at.abs_diff(address) <= FAR {
                            blocks.push(at);
                        } else {
                            self.call(at, target_mode);
                        }
                    }
                }

                let returns = matches!(
                    instruction.flow,
                    Flow::Branch { link: true, .. }
                        | Flow::CallOtherMode { .. }
                        | Flow::BranchRegister { link: true, .. }
                        | Flow::Svc
                );
                if returns {
                    // execution comes back after a call or an svc
                    labels.insert(address + size);
                }
                match instruction.flow {
                    Flow::Branch { target, link: true } => self.call(target, mode),
                    Flow::CallOtherMode { target } => self.call(target, mode.other()),
                    Flow::Branch { target, link: false } => {
                        let far = target < entry || target.abs_diff(address) > FAR;
                        let tail_call = target != entry
                            && self.entries.as_ref().is_some_and(|entries| entries.contains(&target) || far);
                        if tail_call && self.text.contains(target) {
                            if let Some(entries) = self.entries.as_mut() {
                                if entries.insert(target) {
                                    if let Some(trial) = &mut self.trial {
                                        trial.entries.push(target);
                                    }
                                }
                            }
                            // a piece stops where the function it was cut from
                            // does
                            if !cuts.is_empty() {
                                self.cuts.entry(target).or_default().extend(cuts.iter().copied());
                            }
                            self.call(target, mode);
                        }
                        if self.text.contains(target) && !tail_call {
                            blocks.push(target);
                        }
                        if !conditional {
                            break;
                        }
                    }
                    Flow::BranchRegister { link: true, .. } => self.analysis.indirect_sites += 1,
                    Flow::BranchRegister { link: false, .. } | Flow::IndirectJump => {
                        self.analysis.indirect_sites += 1;
                        if !conditional {
                            break;
                        }
                    }
                    Flow::Return => {
                        if !conditional {
                            break;
                        }
                    }
                    Flow::JumpTable => {
                        self.branch_table(address, &mut blocks);
                        break;
                    }
                    Flow::AddressTable { index } => {
                        self.address_table(address, index, &mut blocks);
                        if !conditional {
                            break;
                        }
                    }
                    Flow::LoadPcLiteral { literal } => {
                        self.mark_literal(literal, 4, literal_bit(mode));
                        if let Some(target) = self.text.read32(literal).filter(|_| self.may_point(literal)) {
                            self.code_pointer(target, Source::Pointer);
                        }
                        if !conditional {
                            break;
                        }
                    }
                    Flow::LiteralLoad { literal, size } => {
                        // a load from something already decoded as code is
                        // left alone
                        if self.text.contains(literal) && self.byte(literal) != Byte::Code {
                            self.mark_literal(literal, size, literal_bit(mode));
                            if size == 4 && self.may_point(literal) {
                                if let Some(value) = self.text.read32(literal) {
                                    self.code_pointer(value, Source::Pointer);
                                }
                            }
                        }
                    }
                    Flow::Svc => self.analysis.svc_sites += 1,
                    Flow::Undefined | Flow::Other => {}
                }
                address += size;
            }
        }
        labels.retain(|label| code.contains(label));
        (code.into_iter().collect(), labels)
    }

    /// add pc, pc, rm lsl 2 jumps into a run of branches that starts with
    /// the default case right after it.
    fn branch_table(&mut self, address: u32, blocks: &mut Vec<u32>) {
        self.analysis.jump_tables += 1;
        let mut entry = address + 4;
        while let Some(word) = self.text.read32(entry) {
            match arm::decode(word, entry) {
                Instruction { condition: arm::ALWAYS, flow: Flow::Branch { link: false, .. } } => {
                    blocks.push(entry);
                    entry += 4;
                }
                _ => break,
            }
        }
    }

    /// ldr pc, [pc, rm lsl 2] loads its target from the table of addresses
    /// right after the default branch. its size comes from the cmp just
    /// before it, or failing that from how far the entries keep pointing
    /// into the code.
    fn address_table(&mut self, address: u32, index: u32, blocks: &mut Vec<u32>) {
        self.analysis.jump_tables += 1;
        let entries = address
            .checked_sub(4)
            .and_then(|previous| self.text.read32(previous))
            .and_then(arm::compare_immediate)
            .filter(|&(register, _)| register == index)
            .map(|(_, limit)| limit as usize + 1);
        let table = address + 8;
        for i in 0..entries.unwrap_or(256) {
            let slot = table + i as u32 * 4;
            let Some(target) = self.text.read32(slot) else { break };
            if !self.text.contains(target & !1) || !self.may_point(slot) {
                break;
            }
            self.mark_literal(slot, 4, LITERAL_ARM);
            if target & 3 == 0 {
                blocks.push(target);
            } else {
                self.code_pointer(target, Source::Pointer);
            }
        }
    }
}

/// cpu: the address add/sub rd, pc, #imm (rd not pc) makes.
fn pc_relative_arm(word: u32, address: u32) -> Option<u32> {
    if word >> 28 == 0xF || (word >> 12) & 0xF == 15 {
        return None;
    }
    let immediate = (word & 0xFF).rotate_right(((word >> 8) & 0xF) * 2);
    match word & 0x0FEF_0000 {
        0x028F_0000 => Some(address.wrapping_add(8).wrapping_add(immediate)),
        0x024F_0000 => Some(address.wrapping_add(8).wrapping_sub(immediate)),
        _ => None,
    }
}

/// cpu: the registers that hold an address computed from the pc, as one
/// block of code is followed. anything else written to a register forgets
/// it. what it finds only adds entries, so a value it gets wrong costs a
/// label at most.
#[derive(Default)]
struct Known([Option<u32>; 16]);

impl Known {
    /// follows an ARM instruction, and gives the target of a jump or call
    /// through a register it knows, with the mode it goes to and whether it
    /// links.
    fn arm(&mut self, w: u32, a: u32) -> Option<(u32, Mode, bool)> {
        if w >> 28 == 0xF {
            // blx immediate and the like: a call
            self.call();
            return None;
        }
        let rd = ((w >> 12) & 0xF) as usize;
        let rn = ((w >> 16) & 0xF) as usize;
        let rm = (w & 0xF) as usize;
        let value = |known: &Known, r: usize| if r == 15 { Some(a.wrapping_add(8)) } else { known.0[r] };
        // bx / blx register
        if w & 0x0FFF_FFD0 == 0x012F_FF10 {
            let link = w & 0x20 != 0;
            let target = if rm == 14 { None } else { value(self, rm) };
            if link {
                self.call();
            }
            return target.map(|t| (t & !1, if t & 1 != 0 { Mode::Thumb } else { Mode::Arm }, link));
        }
        // add / sub rd, rn, #imm
        let immediate = (w & 0xFF).rotate_right(((w >> 8) & 0xF) * 2);
        let add = w & 0x0FE0_0000 == 0x0280_0000;
        if add || w & 0x0FE0_0000 == 0x0240_0000 {
            let result = value(self, rn).map(|v| if add { v.wrapping_add(immediate) } else { v.wrapping_sub(immediate) });
            if rd == 15 {
                return result.map(|t| (t & !3, Mode::Arm, false));
            }
            self.0[rd] = result;
            return None;
        }
        // mov rd, rm
        if w & 0x0FEF_0FF0 == 0x01A0_0000 {
            let result = if rm == 14 { None } else { value(self, rm) };
            if rd == 15 {
                return result.map(|t| (t & !3, Mode::Arm, false));
            }
            self.0[rd] = result;
            return None;
        }
        // what else writes registers forgets them
        match (w >> 25) & 7 {
            0 | 1 => {
                let opcode = (w >> 21) & 0xF;
                let test = (8..=11).contains(&opcode) && w & 0x0010_0000 != 0;
                if w & 0x0E00_0090 == 0x0000_0090 {
                    // multiplies, extra loads and stores
                    self.0[rd] = None;
                    self.0[rn] = None;
                } else if !test {
                    self.0[rd] = None;
                }
            }
            2 | 3 => {
                if w & 0x0010_0000 != 0 {
                    self.0[rd] = None;
                }
                if w & 0x0020_0000 != 0 || w & 0x0100_0000 == 0 {
                    self.0[rn] = None;
                }
            }
            4 => {
                if w & 0x0010_0000 != 0 {
                    for r in 0..16 {
                        if w & (1 << r) != 0 {
                            self.0[r] = None;
                        }
                    }
                }
                if w & 0x0020_0000 != 0 {
                    self.0[rn] = None;
                }
            }
            5 => {
                if w & 0x0100_0000 != 0 {
                    self.call();
                }
            }
            _ => {
                // coprocessor moves into a core register, svc
                self.0[rd] = None;
                self.call();
            }
        }
        None
    }

    /// the same for a Thumb instruction: adr, adds rd, #imm, mov rd, rm,
    /// bx/blx rm and bx pc. anything else forgets everything.
    fn thumb(&mut self, h: u16, a: u32) -> Option<(u32, Mode, bool)> {
        let pc = a.wrapping_add(4);
        if h & 0xF800 == 0xA000 {
            self.0[((h >> 8) & 7) as usize] = Some((pc & !3) + (h as u32 & 0xFF) * 4);
            return None;
        }
        if h & 0xF800 == 0x3000 {
            let r = ((h >> 8) & 7) as usize;
            self.0[r] = self.0[r].map(|v| v.wrapping_add(h as u32 & 0xFF));
            return None;
        }
        if h & 0xFF00 == 0x4600 {
            let rd = ((h & 7) | ((h >> 4) & 8)) as usize;
            let rm = ((h >> 3) & 0xF) as usize;
            let value = if rm == 15 { Some(pc) } else if rm == 14 { None } else { self.0[rm] };
            if rd == 15 {
                return value.map(|t| (t & !1, Mode::Thumb, false));
            }
            self.0[rd] = value;
            return None;
        }
        if h & 0xFF07 == 0x4700 {
            let rm = ((h >> 3) & 0xF) as usize;
            let link = h & 0x80 != 0;
            let value = if rm == 15 { Some(pc & !3) } else if rm == 14 { None } else { self.0[rm] };
            if link {
                self.call();
            }
            return value.map(|t| (t & !1, if t & 1 != 0 { Mode::Thumb } else { Mode::Arm }, link));
        }
        // compares and stores keep what is known, the rest forgets
        let keeps = h & 0xF800 == 0x2800 || h & 0xFFC0 == 0x4280 || (h & 0xF000 == 0x6000 && h & 0x0800 == 0) || h & 0xF800 == 0x9000;
        if !keeps {
            self.0 = [None; 16];
        }
        None
    }

    /// a call clobbers the registers the procedure call standard lets it.
    fn call(&mut self) {
        for r in [0, 1, 2, 3, 12, 14] {
            self.0[r] = None;
        }
    }
}

/// where an ARM instruction that puts an address in the code into lr points
/// it, add lr, pc, #n, sub lr, pc, #n and mov lr, pc, which hand-written code
/// uses to call with a plain branch.
fn link_return(word: u32, address: u32) -> Option<u32> {
    if word >> 28 == 0xF {
        return None;
    }
    let pc = address.wrapping_add(8);
    if word & 0x0FEF_FFFF == 0x01A0_E00F {
        return Some(pc);
    }
    let immediate = || (word & 0xFF).rotate_right(((word >> 8) & 0xF) * 2);
    let target = match word & 0x0FEF_F000 {
        0x028F_E000 => pc.wrapping_add(immediate()),
        0x024F_E000 => pc.wrapping_sub(immediate()),
        _ => return None,
    };
    (target & 3 == 0).then_some(target)
}

/// coverage: how the generated code gets to a place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Way {
    /// the instruction before it goes on into it.
    FallThrough,
    /// a branch without link goes there.
    Branch,
    /// a call returns there.
    Return,
    /// a call goes there.
    Call,
}

/// coverage: a place the generated code goes to directly that no function
/// holds as an entry in the mode it goes there in, so that the host finds no
/// recompiled code for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Gap {
    /// where it goes, the mode it goes in.
    pub to: u32,
    pub mode: Mode,
    /// the instruction that goes there, and the function holding it.
    pub from: u32,
    pub function: u32,
    pub way: Way,
}

/// coverage: whether the bytes at address are an instruction in mode, one
/// the CPU runs rather than takes the undefined instruction exception at, as
/// far as following the code needs to know: a lone half of a Thumb bl or
/// blx pair is none.
pub fn decodes(text: &Segment, address: u32, mode: Mode) -> bool {
    let instruction = match mode {
        Mode::Arm => text.read32(address).map(|word| arm::decode(word, address)),
        Mode::Thumb => text.read16(address).map(|halfword| thumb::decode(halfword, text.read16(address + 2), address).0),
    };
    instruction.is_some_and(|instruction| instruction.flow != Flow::Undefined)
}

/// coverage: every place the code functions hold goes to directly, by
/// falling through, branching, calling or returning from a call, that is no
/// label of any function in that mode. the generated code goes to such a
/// place through the host, which has nothing for it there.
pub fn gaps(text: &Segment, analysis: &Analysis) -> Vec<Gap> {
    let mut entries = std::collections::HashSet::new();
    for (entry, function) in &analysis.functions {
        if function.labels.contains(entry) {
            entries.extend(function.labels.iter().map(|&label| (label, function.mode)));
        }
    }
    let mut gaps = BTreeSet::new();
    for (&entry, function) in &analysis.functions {
        if !function.labels.contains(&entry) {
            continue;
        }
        for transfer in transfers(text, function) {
            let held_next = transfer.local && transfer.next_held;
            if !held_next && !(transfer.local && function.labels.contains(&transfer.to)) && !entries.contains(&(transfer.to, transfer.mode)) {
                gaps.insert(Gap { to: transfer.to, mode: transfer.mode, from: transfer.from, function: entry, way: transfer.way });
            }
        }
    }
    gaps.into_iter().collect()
}

/// codemods: a place a function's code goes to directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transfer {
    /// where it goes, the mode it goes in.
    pub to: u32,
    pub mode: Mode,
    /// the instruction that goes there.
    pub from: u32,
    pub way: Way,
    /// whether the function's own code can take it there, by one of its
    /// labels or its next instruction (falling through, returns, branches),
    /// rather than going to another function (calls, ldr pc).
    pub local: bool,
    /// whether the place is the instruction the function holds after from.
    pub next_held: bool,
}

/// codemods: every place a function's code goes to directly, by falling
/// through, branching, calling or returning from a call: the walk gaps
/// makes, for one function.
pub fn transfers(text: &Segment, function: &Function) -> Vec<Transfer> {
    let decode = |address: u32, mode: Mode| -> Option<(Instruction, u32)> {
        match mode {
            Mode::Arm => Some((arm::decode(text.read32(address)?, address), 4)),
            Mode::Thumb => Some(thumb::decode(text.read16(address)?, text.read16(address + 2), address)),
        }
    };
    let mode = function.mode;
    let mut found = Vec::new();
    for (i, &address) in function.instructions.iter().enumerate() {
        let Some((instruction, size)) = decode(address, mode) else { continue };
        let next = address + size;
        let mut add = |to: u32, mode: Mode, way: Way, local: bool| {
            let next_held = function.instructions.get(i + 1) == Some(&to);
            found.push(Transfer { to, mode, from: address, way, local, next_held });
        };
        let ends = !instruction.is_conditional()
            && matches!(
                instruction.flow,
                Flow::Branch { link: false, .. }
                    | Flow::BranchRegister { link: false, .. }
                    | Flow::IndirectJump
                    | Flow::Return
                    | Flow::JumpTable
                    | Flow::AddressTable { .. }
                    | Flow::LoadPcLiteral { .. }
            );
        let call = matches!(instruction.flow, Flow::Branch { link: true, .. } | Flow::CallOtherMode { .. } | Flow::BranchRegister { link: true, .. } | Flow::Svc);
        if !ends && !matches!(instruction.flow, Flow::JumpTable) {
            add(next, mode, if call { Way::Return } else { Way::FallThrough }, true);
        }
        match instruction.flow {
            Flow::Branch { target, link: false } if text.contains(target) => add(target, mode, Way::Branch, true),
            Flow::Branch { target, link: true } if text.contains(target) => add(target, mode, Way::Call, false),
            Flow::CallOtherMode { target } if text.contains(target) => add(target, mode.other(), Way::Call, false),
            // ldr pc, =target
            Flow::LoadPcLiteral { literal } => {
                let value = text.read32(literal).unwrap_or(0);
                let (to, to_mode) = if value & 1 != 0 { (value & !1, Mode::Thumb) } else { (value, Mode::Arm) };
                if text.contains(to) && (to_mode == Mode::Thumb || to & 3 == 0) {
                    add(to, to_mode, Way::Branch, false);
                }
            }
            _ => {}
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// a call made by hand with add lr, pc and a branch comes back to a
    /// label, so the host can go on there in recompiled code.
    #[test]
    fn a_call_made_by_hand_returns_to_a_label() {
        let analysis = analyze_words(&[
            0xE28F_E004, // add lr, pc, #4 (lr = BASE + 12)
            0x0A00_0001, // beq BASE + 0x10
            0x1A00_0000, // bne BASE + 0x10
            0xE12F_FF1E, // bx lr, where the call comes back
            0xE12F_FF1E, // bx lr, the subroutine
        ]);
        assert!(analysis.functions[&BASE].labels.contains(&(BASE + 12)));
        assert_eq!(link_return(0xE1A0_E00F, BASE), Some(BASE + 8));
        assert_eq!(link_return(0xE24F_E004, BASE), Some(BASE + 4));
    }

    const BASE: u32 = 0x0010_0000;

    /// add r4, pc, #a; add r4, r4, #b; mov pc, r4 jumps somewhere known,
    /// which becomes a label; add r12, pc, #1; bx r12 goes on in Thumb.
    #[test]
    fn jumps_through_addresses_the_pc_gave_are_followed() {
        let analysis = analyze_words(&[
            0xE28F_4004, // add r4, pc, #4 (BASE + 12)
            0xE284_4004, // add r4, r4, #4 (BASE + 16)
            0xE1A0_F004, // mov pc, r4
            0xE12F_FF1E, // bx lr, not reached from the jump
            0xE12F_FF1E, // bx lr, the target
        ]);
        assert!(analysis.functions[&BASE].labels.contains(&(BASE + 16)));
        let analysis = analyze_words(&[
            0xE28F_C001, // add r12, pc, #1 (BASE + 9, Thumb)
            0xE12F_FF1C, // bx r12
            0x4770_4770, // bx lr; bx lr in Thumb
        ]);
        assert!(analysis.functions.get(&(BASE + 8)).is_some_and(|f| f.mode == Mode::Thumb));
    }

    fn analyze_words(words: &[u32]) -> Analysis {
        analyze(&program(words))
    }

    fn program(words: &[u32]) -> Program {
        Program {
            text: Segment { base: BASE, bytes: words.iter().flat_map(|w| w.to_le_bytes()).collect() },
            seeds: vec![(BASE, Source::Entry)],
            slots: None,
        }
    }

    #[test]
    fn follows_calls_and_marks_literals() {
        let analysis = analyze_words(&[
            0xEB00_0002, // bl 0x100010
            0xEF00_0000, // svc 0
            0xEAFF_FFFE, // b .
            0xDEAD_BEEF, // never reached
            0xE59F_0000, // ldr r0, [pc] (literal at 0x100018)
            0xE12F_FF1E, // bx lr
            0x0010_0000, // the literal
        ]);
        assert_eq!(analysis.functions.keys().copied().collect::<Vec<_>>(), [BASE, BASE + 0x10]);
        assert_eq!(analysis.functions[&BASE].labels, BTreeSet::from([BASE, BASE + 4, BASE + 8]));
        assert_eq!(analysis.functions[&(BASE + 0x10)].source, Source::Call);
        assert_eq!(analysis.count(Byte::Code), 5 * 4);
        assert_eq!(analysis.count(Byte::Literal), 4);
        assert_eq!(analysis.largest_gaps(BASE, 1), [(BASE + 0xC, 4)]);
        assert_eq!(analysis.svc_sites, 1);
    }

    /// code reached only through branches far apart, the way tail calls
    /// reach other functions, is one function while it is small, as it
    /// always was. past what a function holds, each piece of it becomes a
    /// function of its own, its branch a tail call.
    #[test]
    fn a_huge_chain_of_tail_calls_is_split_into_functions() {
        const SPACING: u32 = 2 * FAR;
        let chain = |links: u32, length: u32| {
            let mut words = vec![0u32; (links * SPACING / 4) as usize];
            for link in 0..links {
                let start = link * SPACING / 4;
                for i in 0..length {
                    words[(start + i) as usize] = 0xE1A0_0000; // mov r0, r0
                }
                words[(start + length) as usize] = if link + 1 == links {
                    0xE12F_FF1E // bx lr
                } else {
                    0xEA00_0000 | ((SPACING - length * 4 - 8) / 4) // b to the next piece
                };
            }
            analyze_words(&words)
        };
        let small = chain(3, 100);
        assert_eq!(small.functions.len(), 1);
        assert_eq!(small.functions[&BASE].instructions.len(), 3 * 101);
        let huge = chain(30, 400);
        assert_eq!(huge.functions.len(), 30);
        assert!(huge.functions.values().all(|f| f.instructions.len() == 401));
        assert!(huge.functions.values().skip(1).all(|f| f.source == Source::Call));
    }

    #[test]
    fn a_switch_table_leads_to_every_case() {
        let analysis = analyze_words(&[
            0xE350_0001, // cmp r0, 1
            0x908F_F100, // addls pc, pc, r0 lsl 2
            0xEA00_0002, // b default (0x100018)
            0xEA00_0002, // b case 0 (0x10001c)
            0xEA00_0002, // b case 1 (0x100020)
            0x0000_0000, // data between the table and the cases
            0xE12F_FF1E, // default
            0xE12F_FF1E, // case 0
            0xE12F_FF1E, // case 1
        ]);
        assert_eq!(analysis.jump_tables, 1);
        assert_eq!(analysis.count(Byte::Code), 8 * 4);
        let labels: Vec<_> = analysis.functions[&BASE].labels.iter().map(|l| l - BASE).collect();
        assert_eq!(labels, [0, 8, 0xC, 0x10, 0x18, 0x1C, 0x20]);
    }

    #[test]
    fn an_address_table_is_sized_by_its_compare() {
        let analysis = analyze_words(&[
            0xE350_0001, // cmp r0, 1
            0x979F_F100, // ldrls pc, [pc, r0 lsl 2]
            0xE12F_FF1E, // bx lr, the default case
            BASE + 0x14, // case 0
            BASE + 0x18, // case 1
            0xE12F_FF1E, // case 0
            0xE12F_FF1E, // case 1
        ]);
        assert_eq!(analysis.jump_tables, 1);
        assert_eq!(analysis.count(Byte::Literal), 8);
        assert_eq!(analysis.count(Byte::Code), 5 * 4);
    }

    #[test]
    fn blx_leads_into_thumb_code() {
        let analysis = analyze_words(&[
            0xFA00_0000, // blx 0x100008
            0xE12F_FF1E, // bx lr
            0x4770_2001, // movs r0, 1 then bx lr, in Thumb
        ]);
        let thumb = &analysis.functions[&(BASE + 8)];
        assert_eq!(thumb.mode, Mode::Thumb);
        assert_eq!(thumb.instructions, [BASE + 8, BASE + 10]);
    }

    #[test]
    fn pointer_seeds_become_functions() {
        let mut program = program(&[0xE12F_FF1E, 0x4770_4770, 0xE12F_FF1E]);
        program.seeds.extend([(BASE + 8, Source::Pointer), (BASE + 5, Source::Pointer)]);
        let analysis = analyze(&program);
        assert_eq!(analysis.functions[&(BASE + 8)].source, Source::Pointer);
        assert_eq!(analysis.functions[&(BASE + 4)].mode, Mode::Thumb);
    }

    #[test]
    fn with_slots_only_relocated_literals_are_pointers() {
        let words = [
            0xE59F_0000, // ldr r0, [pc] (literal at 0x100008)
            0xE12F_FF1E, // bx lr
            BASE + 0xC,  // looks like a code address
            0xE12F_FF1E, // bx lr
        ];
        assert_eq!(analyze_words(&words).functions[&(BASE + 0xC)].source, Source::Pointer);
        let mut program = program(&words);
        program.slots = Some(BTreeSet::new());
        // the code is still there for the scan to find, only not through
        // the literal
        assert_eq!(analyze(&program).functions[&(BASE + 0xC)].source, Source::Scan);
    }

    /// code nothing leads to still becomes functions where a gap begins
    /// with an instruction or a push saves registers, data does not.
    #[test]
    fn gaps_are_scanned_for_functions() {
        let analysis = analyze_words(&[
            0xE12F_FF1E, // bx lr, the entry
            0xE3A0_0001, // mov r0, #1, the start of a gap
            0xE12F_FF1E, // bx lr
            0x1234_5678, // data
            0xE92D_4010, // push {r4, lr}
            0xE8BD_8010, // pop {r4, pc}
        ]);
        assert_eq!(analysis.functions[&(BASE + 4)].source, Source::Scan);
        assert_eq!(analysis.functions[&(BASE + 16)].source, Source::Scan);
        assert!(!analysis.functions.contains_key(&(BASE + 12)));
    }

    /// a guess whose code is mostly conditional is not ARM a compiler made,
    /// so it is no function, the bytes it decoded are not code, and a call
    /// in it leads nowhere.
    #[test]
    fn a_guess_that_is_not_code_leads_nowhere() {
        let mut words = vec![0xE12F_FF1E]; // bx lr, the entry
        let garbage = 1;
        for i in 0..24u32 {
            words.push((i % 14) << 28 | 0x0081_0002); // add<cond> r0, r1, r2
        }
        // blne to the bx lr after the garbage
        let call = garbage + 10;
        let after = garbage + 24;
        words[call] = 0x1B00_0000 | (after - call - 2) as u32;
        words.push(0xE12F_FF1E);
        let mut program = program(&words);
        program.seeds.push((BASE + 4 * garbage as u32, Source::Pointer));
        let analysis = analyze(&program);
        assert_eq!(analysis.functions.keys().copied().collect::<Vec<_>>(), [BASE]);
        assert_eq!(analysis.count(Byte::Code), 4);
    }

    /// text taken for Thumb has no push, branch, call or return, so it is no
    /// function.
    #[test]
    fn text_taken_for_thumb_is_no_function() {
        let text = b"abcdefghijklmnopqrstuvwxyzabcdefghijklmn";
        let mut words = vec![0xE12F_FF1E]; // bx lr, the entry
        words.extend(text.chunks(4).map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap())));
        let mut program = program(&words);
        program.seeds.push((BASE + 4 + 1, Source::Pointer));
        let analysis = analyze(&program);
        assert_eq!(analysis.functions.keys().copied().collect::<Vec<_>>(), [BASE]);
    }

    /// a guess that runs into code a function holds stops there, the place
    /// becoming a label of that function, and a guess into the middle of
    /// such code is no function, rather than each copying the code.
    #[test]
    fn guesses_join_code_instead_of_copying_it() {
        let mut program = program(&[
            0xEB00_0002, // bl 0x100010, the entry
            0xE12F_FF1E, // bx lr
            0xE3A0_0001, // mov r0, 1, a guess
            0xEA00_0000, // b 0x100014, into the called function
            0xE3A0_2003, // mov r2, 3, the called function
            0xE3A0_3004, // mov r3, 4
            0xE12F_FF1E, // bx lr
        ]);
        program.seeds.extend([(BASE + 8, Source::Pointer), (BASE + 0x14, Source::Pointer)]);
        let analysis = analyze(&program);
        assert_eq!(analysis.functions.keys().copied().collect::<Vec<_>>(), [BASE, BASE + 8, BASE + 0x10]);
        assert_eq!(analysis.functions[&(BASE + 8)].instructions, [BASE + 8, BASE + 0xC]);
        assert!(analysis.functions[&(BASE + 0x10)].labels.contains(&(BASE + 0x14)));
    }

    /// a pointer into the middle of code a function holds, where it falls
    /// through, becomes a label of that function, so that the host can go on
    /// there in recompiled code if the pointer is real.
    #[test]
    fn a_pointer_into_held_code_becomes_a_label() {
        let mut program = program(&[
            0xEB00_0000, // bl 0x100008, the entry
            0xE12F_FF1E, // bx lr
            0xE3A0_2000, // mov r2, 0, the called function, falling through
            0xE3A0_0001, // mov r0, 1, where a vtable points
            0xE12F_FF1E, // bx lr
        ]);
        program.seeds.push((BASE + 0xC, Source::Pointer));
        let analysis = analyze(&program);
        assert_eq!(analysis.functions.keys().copied().collect::<Vec<_>>(), [BASE, BASE + 8]);
        assert!(analysis.functions[&(BASE + 8)].labels.contains(&(BASE + 0xC)));
    }

    /// a huge chain of near tail calls whose links vtables point at splits at
    /// the links, as it did when each pointer made a function of its own.
    #[test]
    fn a_huge_near_chain_splits_where_pointers_point() {
        let (links, length, spacing) = (30u32, 400u32, 0x1000u32);
        let mut words = vec![0u32; (links * spacing / 4) as usize];
        for link in 0..links {
            let start = (link * spacing / 4) as usize;
            for i in 0..length as usize {
                words[start + i] = 0xE1A0_0000; // mov r0, r0
            }
            words[start + length as usize] =
                if link + 1 == links { 0xE12F_FF1E } else { 0xEA00_0000 | ((spacing - length * 4 - 8) / 4) };
        }
        let mut program = program(&words);
        program.seeds.extend((1..links).map(|link| (BASE + link * spacing, Source::Pointer)));
        let analysis = analyze(&program);
        assert_eq!(analysis.functions.len(), links as usize);
        assert!(analysis.functions.values().all(|f| f.instructions.len() == length as usize + 1));
    }

    /// a huge function a guess led to splits into pieces that keep all of
    /// its code, as one the entry leads to does.
    #[test]
    fn a_huge_guess_splits_without_losing_code() {
        const SPACING: u32 = 2 * FAR;
        let (links, length) = (30, 400);
        let mut words = vec![0xE12F_FF1E]; // bx lr, the entry
        let start = words.len();
        words.resize(start + (links * SPACING / 4) as usize, 0);
        for link in 0..links {
            let piece = start + (link * SPACING / 4) as usize;
            for i in 0..length {
                words[piece + i as usize] = 0xE1A0_0000; // mov r0, r0
            }
            words[piece + length as usize] =
                if link + 1 == links { 0xE12F_FF1E } else { 0xEA00_0000 | ((SPACING - length * 4 - 8) / 4) };
        }
        let mut program = program(&words);
        program.seeds.push((BASE + 4, Source::Pointer));
        let analysis = analyze(&program);
        assert_eq!(analysis.functions.len(), 1 + links as usize);
        assert!(analysis.functions.values().skip(1).all(|f| f.instructions.len() == length as usize + 1));
    }

    /// coverage: a guess in Thumb over ARM code, whose load marks a word in
    /// the middle of an ARM function a literal, does not cut that function
    /// short: ARM code goes on through literals only Thumb code loads.
    /// castle::MaterialWindow::Draw and zlib's deflate_fast were cut so.
    #[test]
    fn a_thumb_guess_does_not_cut_arm_code_with_its_literal() {
        let mut program = program(&[
            0xE12F_FF1E, // bx lr, the entry
            0x4770_4801, // in Thumb: ldr r0, [pc, #4] (BASE + 12), bx lr
            0xE3A0_0001, // mov r0, 1, an ARM function a pointer leads to
            0xE3A0_1002, // mov r1, 2, the word the Thumb load reads
            0xE12F_FF1E, // bx lr
        ]);
        program.seeds.extend([(BASE + 5, Source::Pointer), (BASE + 8, Source::Pointer)]);
        let analysis = analyze(&program);
        assert_eq!(analysis.functions[&(BASE + 4)].mode, Mode::Thumb);
        assert_eq!(analysis.functions[&(BASE + 8)].instructions, [BASE + 8, BASE + 12, BASE + 16]);
        assert!(gaps(&program.text, &analysis).is_empty());
        assert_eq!(analysis.gap_functions, 0);
    }

    /// coverage: code a literal mark of a wrong guess in the same mode cuts
    /// short still falls through to an entry: close_gaps makes a function
    /// where it goes on, through the literal, up to code other functions
    /// hold, which becomes a label. the literal stays one for what reads it.
    #[test]
    fn a_function_cut_short_falls_through_to_an_entry() {
        let mut program = program(&[
            0xE12F_FF1E, // bx lr, the entry
            0xE59F_0004, // ldr r0, [pc, #4] (BASE + 16), a guess
            0xE12F_FF1E, // bx lr
            0xE3A0_0001, // mov r0, 1, a function a pointer leads to
            0xE3A0_1002, // mov r1, 2, the word the guess loads
            0xE12F_FF1E, // bx lr
        ]);
        program.seeds.extend([(BASE + 4, Source::Pointer), (BASE + 12, Source::Pointer)]);
        let analysis = analyze(&program);
        assert_eq!(analysis.functions[&(BASE + 12)].instructions, [BASE + 12]);
        let made = &analysis.functions[&(BASE + 16)];
        assert_eq!(made.source, Source::Gap);
        assert!(made.labels.contains(&(BASE + 16)));
        // every instruction is held, and every way into it is an entry
        let held: BTreeSet<u32> = analysis.functions.values().flat_map(|f| f.instructions.iter().copied()).collect();
        assert_eq!(held, BTreeSet::from([BASE, BASE + 4, BASE + 8, BASE + 12, BASE + 16, BASE + 20]));
        assert!(gaps(&program.text, &analysis).is_empty());
        assert_eq!(analysis.map[16], Byte::Literal);
        assert_eq!((analysis.gap_functions, analysis.gaps_left), (1, 0));
    }

    /// coverage: a function start in the middle of a loop, a bogus start
    /// that is no function, leaves nothing out: the code before it, reached
    /// only from the loop's back edge, belongs to a function, and every way
    /// in is an entry.
    #[test]
    fn a_start_in_the_middle_of_a_function_leaves_nothing_out() {
        let mut program = program(&[
            0xE12F_FF1E, // bx lr, the entry
            0xE3A0_0000, // mov r0, 0, a function a pointer leads to
            0xEA00_0000, // b BASE + 16, to the test
            0xE280_0001, // add r0, r0, 1, the loop's body
            0xE350_000A, // cmp r0, 10, where a hint says a function starts
            0xBAFF_FFFC, // blt BASE + 12
            0xE12F_FF1E, // bx lr
        ]);
        program.seeds.extend([(BASE + 16, Source::Hint), (BASE + 4, Source::Pointer)]);
        let analysis = analyze(&program);
        let held: BTreeSet<u32> = analysis.functions.values().flat_map(|f| f.instructions.iter().copied()).collect();
        assert_eq!(held, (0..7).map(|i| BASE + 4 * i).collect());
        assert!(gaps(&program.text, &analysis).is_empty());
        // the start the hint gave stays an entry, for whatever keys on it
        assert!(analysis.functions[&(BASE + 16)].labels.contains(&(BASE + 16)));
    }

    /// coverage: a call in ARM to where a guess in Thumb already starts a
    /// function gets a function in ARM there: the guess gives way.
    #[test]
    fn a_call_takes_its_address_from_a_guess_in_the_other_mode() {
        let mut program = program(&[
            0xE12F_FF1E, // bx lr, the entry
            0xE3A0_0001, // mov r0, 1, called in ARM, pointed at in Thumb
            0xE12F_FF1E, // bx lr
            0xEBFF_FFFC, // bl BASE + 4, a guess
            0xE12F_FF1E, // bx lr
        ]);
        program.seeds.extend([(BASE + 5, Source::Pointer), (BASE + 12, Source::Pointer)]);
        let analysis = analyze(&program);
        let called = &analysis.functions[&(BASE + 4)];
        assert_eq!((called.mode, called.source), (Mode::Arm, Source::Gap));
        // it goes on into the bx lr the scan of the gaps made a function of
        assert_eq!(called.instructions, [BASE + 4]);
        assert_eq!(analysis.functions[&(BASE + 8)].source, Source::Scan);
        assert_eq!(analysis.gap_evictions, 1);
        assert!(gaps(&program.text, &analysis).is_empty());
    }

    /// coverage: what is left is no instruction, here the half of a bl that
    /// the upper half of an ARM word reads as in Thumb.
    #[test]
    fn only_places_that_are_no_instruction_are_left() {
        let mut program = program(&[
            0xE12F_FF1E, // bx lr, the entry
            0xE92D_2001, // in Thumb: movs r0, 1, then the second half of a bl
        ]);
        program.seeds.push((BASE + 5, Source::Pointer));
        let analysis = analyze(&program);
        let left = gaps(&program.text, &analysis);
        assert_eq!(left.len(), 1);
        assert_eq!((left[0].to, left[0].mode, left[0].way), (BASE + 6, Mode::Thumb, Way::FallThrough));
        assert!(!decodes(&program.text, BASE + 6, Mode::Thumb));
        assert_eq!(analysis.gaps_left, 1);
    }

    /// coverage: the programs of the other tests leave no gaps.
    #[test]
    fn plain_code_has_no_gaps() {
        let words = [0xEB00_0002, 0xEF00_0000, 0xEAFF_FFFE, 0xDEAD_BEEF, 0xE59F_0000, 0xE12F_FF1E, 0x0010_0000];
        let analysis = analyze_words(&words);
        assert!(gaps(&program(&words).text, &analysis).is_empty());
        assert_eq!(analysis.gap_functions, 0);
    }
}
