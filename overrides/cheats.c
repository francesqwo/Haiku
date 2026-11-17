/* cheats: the PC Options menu's Cheats tab (USA Special Edition v2).
 *
 * Every override here runs only the game's original when its cheat is off:
 * no budget change, no write, so a run with every cheat off is the game's
 * own (the gate's from-boot replay stays byte-identical).
 *
 * The host (zakuro/src/cheats.rs) writes cheats_words: the switches, and a
 * request for a one-shot action that has to call the game's own functions
 * (a class change, a join, a support level, a chapter jump, the convoy).
 * The request runs at the top of Proc::Exec (0x001052B8), on the game's main
 * thread between procs, with direct guest calls (info_ui.c's pattern).
 *
 * What the switches change, all in the game's own calculations:
 *
 * - Battle (map::BattleInfo, 4 sides of 0x200 bytes: +4 the Unit, +0x2C the
 *   damage of one hit, +0x30 hit, +0x34 crit). CalculateSimple (0x00347C3C)
 *   fills them for the forecast and the battle reads the same fields
 *   (BattleCalculator::CalculateAttackSingle rolls RandomCalculateHit(+0x30)
 *   and Random(100) < +0x34), so the forecast shows what the battle does.
 *   A tail patches hit and crit by the side's force. Damage comes from
 *   BattleInfo::Side::GetSimplePower (0x005253A8, also asked again when a
 *   skill fires), whose tail sets 0 (no damage to the player's units) or the
 *   target's HP (one hit kills). The rolls still draw the same random numbers.
 * - Movement: Unit::GetMovePower (0x0052D5E4), the player's units.
 * - Growth: Unit::LevelUp (0x003D8154) asks Unit::GetGrow (0x005330B4) for
 *   each stat; while it runs for the player's unit, a rate is answered as
 *   100 (exactly one point a stat, up to the stat's cap).
 * - EXP: map::ExpFactorCalculator::Calculate (0x0036BF98) leaves the EXP
 *   factor (a float at +8, the unit at +0); the tail multiplies it.
 * - Every weapon: Unit::GetWeaponExpLimit (0x0052F824) is the class's cap
 *   per weapon type (0 = can't use); the player's units get S for every type.
 * - Dragon Veins: Unit::CanDragonVein (0x0052DFE8), the player's units.
 * - Support Log, everything: the Support Log (ProcMuseum's support recall)
 *   lists the conversations the player has seen, which
 *   PersonEnumerator_Reliance::IsExclusion (0x002220B8) asks of the global
 *   seen flags (VariableSizeFlagManager::Get, 0x00220F60). Asked from there,
 *   Get answers "seen", so the log lists every pair and rank the game has,
 *   and plays them with its own talk player, Corrin's other gender included
 *   (ProcMuseum::BeforeReliance makes that Corrin from the saved avatar).
 * - Seals: unit::Item::Expend (0x004199BC, an item in a unit's hands used
 *   once) and Transporter::Sub (0x001A7460, the convoy's) skip the seals
 *   the host names.
 * - Turn freeze: map::Situation::TurnEnd (0x003A6850) counts the turn up
 *   after the last phase; the tail puts it back.
 * - Extra skills: Unit::IsEquipSkill (0x0052DD04) answers yes for a
 *   (person, skill) pair the host lists.
 * - Move again: once the free cursor is back in the player phase, a player
 *   unit that has acted (Unit +8 bit 0... the "done" bits) is made ready
 *   again (see cheats_refresh).
 */
#include <string.h>
#include "overrides.h"

#define CW_COUNT 1024
uint32_t cheats_words[CW_COUNT];

/* the words, by index (zakuro/src/cheats.rs has the same list) */
#define CW_FLAGS 0        /* switches, bits below */
#define CW_CRIT 1         /* 0 off, 1 the player's units, 2 everyone */
#define CW_EXP_MULT 2     /* EXP multiplier x100 (100 = off) */
#define CW_MOVE 3         /* movement for the player's units with unlimited move */
#define CW_REQ 4          /* a request: op, the host sets it last */
#define CW_ARG 5          /* its arguments, 5..12 */
#define CW_RESULT 13      /* the request's result (the host reads it) */
#define CW_DONE 14        /* requests done, counts up */
#define CW_RUNS 15        /* Proc::Exec calls seen with a request */
#define CW_FRAME 16       /* the free cursor's last frame (set in info_ui.c's FreeCursorTick) */
#define CW_VIEW_UNIT 17   /* a unit whose stats the menu shows: computed each frame into CW_VIEW */
#define CW_VIEW_TICK 18   /* the host counts it up each frame it wants the view */
#define CW_VIEW 300       /* 300..307 stats (no bonuses), 308..315 their caps, 316 Mov, 317 level cap, 318 max HP, 319 the unit they are of */
#define CW_STATS 20       /* counters: 20 crits forced, 21 hits forced, 22 damage changed, 23 growths, 24 exp, 25 seals kept, 26 turns frozen, 27 log answers, 28 moves again */
#define CW_SEALS 40       /* seal item ids (u16), 40..47 */
#define CW_SKILL_COUNT 48 /* extra skills: count, then (person id << 16 | skill id) from 64 */
#define CW_CAPTURE_SKILL 49 /* cheats2: the two Capture skills' ids (49, 50), from the host */
#define CW_STATS2 52     /* cheats2 counters: 52 Capture target searches with the Jail's capacity lent */
#define CW_SKILLS 64      /* .. 64 + 256 */
#define CW_ITEMS 512      /* the convoy's list of item ids for OP_CONVOY: count at CW_ITEMS, ids from CW_ITEMS + 1 */

#define F_HIT 0x1         /* the player's units hit 100% */
#define F_MISS 0x2        /* enemies always miss */
#define F_GOD 0x4         /* the player's units take no damage */
#define F_OHKO 0x8        /* the player's units kill in one hit */
#define F_MOVE_AGAIN 0x10
#define F_MOVE 0x20       /* unlimited movement */
#define F_GROWTH 0x40     /* perfect level-ups */
#define F_WEAPONS 0x80    /* every class uses every weapon */
#define F_VEINS 0x100     /* every unit can use Dragon Veins */
#define F_LOG 0x200       /* the Support Log lists everything */
#define F_SEALS 0x400     /* unlimited seals */
#define F_TURN 0x800      /* the turn counter stays */
#define F_EXP 0x1000      /* EXP multiplier */
#define F_VEIN_REUSE 0x2000 /* cheats2: a Dragon Vein used is ready again at the next player phase */
#define F_CAPTURE 0x4000  /* cheats2: the player's units can Capture; a captured unit joins at once */

enum {
    OP_NONE = 0,
    OP_CLASS = 1,     /* unit, job: Unit::ClassChange(job, 1) as a seal does, E rank in new weapons */
    OP_JOIN = 2,      /* person: My Castle's join (CreateForCastleJoin, Transfer(3), NormalizeForCastleJoinFriend) */
    OP_JOIN_GUEST = 3,/* person: event::JoinUniqueGuest (amiibo and DLC heroes, as their own scripts) */
    OP_SUPPORT = 4,   /* unit a, unit b, level: Unit::SetRelianceLevel both ways, UpdateLimitOffset */
    OP_LEVEL_UP = 5,  /* unit, count: Unit::LevelUp count times (the game's own rolls) */
    OP_CONVOY = 6,    /* count: each listed item topped up to count in the convoy */
    OP_CHAPTER = 7,   /* chapter, route: from My Castle, the chapter starts as Next Battle starts one */
    OP_ICON = 8,      /* unit: its map icon refreshed (after host-side changes) */
    OP_RECORDS = 9,   /* tests only: My Castle's Records Hall opened (MuseumSequence::Create), as a visit does */
    OP_ITEM = 11,     /* cheats2, tests: unit, item id: unit::Item::New and Unit::ItemAdd, as a gift gives one */
    OP_BUILDING = 10, /* cheats2: building (unique index): My Castle's own level-up sequence for it
                         (castle::CreateBuildingSequence::CreateForLevelUp), as the Build menu's Yes does */
};

/* addresses */
#define A_UNIT_POOL 0x006D9F40u
#define A_FORCES 0x006D9F44u
#define A_SITUATION 0x006D7ED0u
#define A_GAME_USER 0x006DA248u
#define A_CASTLE_MAIN 0x006D1B70u
#define A_FREE_MOVE 0x006D2A10u
#define A_TRANSPORTER 0x006D95E8u
#define A_FRAME 0x006DCAA0u
#define A_SUPPORT_POOL 0x006DA170u /* SupportPool's owner: +0x30 the Unit Logbook */

#define G_CLASS_CHANGE 0x003D40C8u
#define G_UPDATE_ACTOR 0x003D4FBCu
#define G_CREATE_CASTLE_JOIN 0x003D4450u
#define G_TRANSFER 0x003D8C00u
#define G_NORMALIZE_FRIEND 0x0041A650u
#define G_JOIN_UNIQUE_GUEST 0x00423E58u
#define G_EVENT_INSTANCE 0x004223E4u
#define G_TO_PACKET 0x00533ABCu
#define G_SUPPORT_SEARCH 0x00508E44u
#define G_SUPPORT_ENTRY 0x001A61D0u
#define G_SET_RELIANCE_LEVEL 0x003D680Cu
#define G_UPDATE_LIMIT 0x003D6C0Cu
#define G_LEVEL_UP 0x003D8154u
#define G_LIMIT_LEVEL 0x0052E5ECu
#define G_ITEM_NEW 0x004198F4u
#define G_TRANSPORTER_ADD 0x001A7050u
#define G_SET_RESULT 0x00494DDCu
#define G_JUMP_IMM 0x004EE200u
#define G_GET_FROM_PERSON 0x004F5D90u
#define G_MASK_SAME_FORCE 0x0041C6C4u
#define G_MHP 0x0052A364u
#define G_CAPABILITY 0x0052F7A0u /* Unit::GetCapabilityImpl(int, const unit::Item*, bool, bool, type) */
#define G_LIMIT 0x005337F0u
#define G_MOVE 0x0052D5E4u

/* cheats2 */
#define A_CASTLE 0x006D1998u        /* Castle*: +8 the castle's data (+0x14 the Jail) */
#define A_TRICKS 0x006D7FECu        /* map::Trick*: +0 the first trick::Data (+0 next) */
#define A_BUILD_SEQ 0x006D29ACu     /* the building sequence running (CreateBuildingSequence), or 0 */
#define G_TRICK_ENABLE 0x0039AB40u  /* map::trick::Data::Enable(ProcInst*) */
#define G_CREATE_FROM_PACKET 0x003D6258u
#define G_NORMALIZE_GUEST 0x0041A3C4u /* unit::util::NormalizeForCastleJoinGuest, the Prison's Scout */
#define G_JAIL_APPEND 0x0048C038u   /* castle::Jail::Append(const Unit*) */
#define G_LEVEL_UP_BUILDING 0x00462474u
#define G_ITEM_ADD 0x003D7B08u      /* Unit::ItemAdd(const unit::Item*) */
#define TRICK_VEIN 0x0B             /* map::trick::Type of a Dragon Vein */

#define LOG_FROM 0x002220B8u /* PersonEnumerator_Reliance::IsExclusion */
#define LOG_TO 0x00222978u

/* ---- the game's own functions, called from Proc::Exec --------------------
 *
 * A request (and the menu's view of a unit, and Move Again's refresh) runs
 * as a chain of real guest calls made from the top of Proc::Exec: each call
 * is entered with its return address set to JOB_RET (see there),
 * so when it returns the override there takes the next step,
 * as overrides/qol.c and mods.c chain theirs. A call may leave for the host
 * on the way (an svc that waits, the budget): the guest thread is simply
 * suspended and goes on later, exactly as for the game's own calls. The
 * calls run on a stack below the one Proc::Exec was entered with, and a
 * scratch area between the two holds what the steps share (a packet, an
 * item). When the chain ends, Proc::Exec runs as it was called.
 */

/* cheats2: where the job's calls return. Round 1 used Proc::Exec itself
   (0x001052B8), and that crashed: a call returning there is dispatched to
   0x001052B8 by whatever generated code is below it on the host's stack.
   After the main thread was suspended inside a proc (a cutscene waiting
   for its files, a class change say) and woke, the generated code of the
   ORIGINAL Proc::Exec is one of those callers (the woken chain returns
   through it), and to it 0x001052B8 is its own first instruction, a local
   label: the original ran at once with the call's result as its priority,
   the override was skipped, and the game jumped to garbage. The calls now
   return to LinkBitArray512::Dump (0x0050BBA0), which nothing in the game
   calls or refers to, so no generated code is ever below it and its
   address always reaches the override below. */
#define JOB_RET 0x0050BBA0u

enum { K_NONE, K_REQUEST, K_VIEW, K_AGAIN, K_VEINS, K_RECRUIT };

/* cheats2: the Dragon Veins the player's units used (map::trick::Data*),
   made ready again at the next player phase; and the captured units waiting
   to join (castle::Jail entries, game::packet::Unit at +8) */
#define VEIN_MAX 32
static uint32_t veins_used[VEIN_MAX], veins_count, veins_phase = 0xFF;
#define RECRUIT_MAX 8
static uint32_t recruits[RECRUIT_MAX], recruits_count;

typedef struct {
    int active;
    uint32_t tls, lr, sp, sp_entry, prio;
    uint32_t kind, op, step;
    uint32_t a[4];
    uint32_t v[4];
    uint32_t i, result;
} Job;
static Job job;

#define CALL_SP (job.sp - 0x400u)
#define SCRATCH (job.sp - 0x200u) /* 0x180 bytes no call reaches: above its stack */

static uint32_t cw(int i) { return *(volatile uint32_t *)&cheats_words[i]; }

static void ch_issue(Context *ctx, uint32_t fn, uint32_t a0, uint32_t a1, uint32_t a2, uint32_t a3) {
    ctx->r[0] = a0;
    ctx->r[1] = a1;
    ctx->r[2] = a2;
    ctx->r[3] = a3;
    ctx->r[13] = CALL_SP;
    ctx->r[14] = JOB_RET;
    ctx->r[15] = fn;
    ctx->thumb = 0;
    ctx->budget -= 4;
    CALL(recomp_call);
}

/* the same with two more arguments on the stack */
static void ch_issue6(Context *ctx, uint32_t fn, uint32_t a0, uint32_t a1, uint32_t a2, uint32_t a3, uint32_t a4, uint32_t a5) {
    mem_write32(ctx, CALL_SP, a4);
    mem_write32(ctx, CALL_SP + 4, a5);
    ch_issue(ctx, fn, a0, a1, a2, a3);
}

/* the force a unit belongs to (0 the player's on the map, 3 the army) */
static uint32_t ch_force(Context *ctx, uint32_t unit) {
    if (!unit)
        return 0xFF;
    uint32_t force = mem_read32(ctx, unit + 0xA4);
    return force ? mem_read8(ctx, force + 8) : 0xFF;
}

static int ch_player(Context *ctx, uint32_t unit) {
    uint32_t f = ch_force(ctx, unit);
    return f == 0 || f == 3;
}

/* an enemy (force 1) */
static int ch_enemy(Context *ctx, uint32_t unit) {
    return ch_force(ctx, unit) == 1;
}

/* the player's side in battle: the army (0, 3) and green allies (2), who
   fight beside it (Hinoka in Chapter 4, Ryoma in Chapter 5) */
static int ch_ours(Context *ctx, uint32_t unit) {
    uint32_t f = ch_force(ctx, unit);
    return f == 0 || f == 2 || f == 3;
}

/* online2: a unit the cheats changed, for online battles only (bit 31 of the
   state's upper word, Unit +0xC; see online.c). It stays once set. */
#define UNIT_CHEATED_WORD 0x0Cu
#define UNIT_CHEATED_BIT  0x80000000u
static void ch_mark(Context *ctx, uint32_t unit) {
    if (unit && ch_player(ctx, unit) && !(mem_read32(ctx, unit + UNIT_CHEATED_WORD) & UNIT_CHEATED_BIT))
        mem_write32(ctx, unit + UNIT_CHEATED_WORD, mem_read32(ctx, unit + UNIT_CHEATED_WORD) | UNIT_CHEATED_BIT);
}

static uint32_t ch_free_unit(Context *ctx) {
    uint32_t forces = mem_read32(ctx, A_FORCES);
    return forces ? mem_read32(ctx, forces + 9 * 12) : 0;
}

static int ch_valid_unit(Context *ctx, uint32_t unit) {
    uint32_t pool = mem_read32(ctx, A_UNIT_POOL);
    return pool && unit >= pool && unit < pool + 0x140 * 256 && (unit - pool) % 0x140 == 0 && mem_read32(ctx, unit + 0x9C) && mem_read32(ctx, unit + 0xA0);
}

/* the next step of the job, r the last call's result: 1 when a call was
   made (the override returns), 0 when the job is over */
static int ch_step(Context *ctx, uint32_t r) {
    for (;;) {
        uint32_t s = job.step;
        if (job.kind == K_VIEW) {
            uint32_t unit = job.a[0];
            job.step++;
            if (s >= 1 && s <= 8)
                cheats_words[CW_VIEW + s - 1] = r;
            else if (s >= 9 && s <= 16)
                cheats_words[CW_VIEW + 8 + s - 9] = r;
            else if (s == 17)
                cheats_words[CW_VIEW + 16] = r;
            else if (s == 18)
                cheats_words[CW_VIEW + 17] = r;
            else if (s == 19) {
                cheats_words[CW_VIEW + 18] = r;
                cheats_words[CW_VIEW + 19] = unit;
                return 0;
            }
            if (s < 8) {
                ch_issue6(ctx, G_CAPABILITY, unit, s, 0, 0, 0, 0);
                return 1;
            }
            if (s < 16) {
                ch_issue(ctx, G_LIMIT, unit, s - 8, 0, 0);
                return 1;
            }
            if (s == 16) {
                ch_issue(ctx, G_MOVE, unit, 0, 0, 0);
                return 1;
            }
            if (s == 17) {
                ch_issue(ctx, G_LIMIT_LEVEL, unit, 0, 0, 0);
                return 1;
            }
            ch_issue(ctx, G_MHP, unit, 1, 0, 0);
            return 1;
        }
        if (job.kind == K_AGAIN) {
            /* the next player unit that has acted: ready again, its icon redrawn */
            uint32_t unit = job.v[0];
            while (unit && !(mem_read32(ctx, unit + 8) & 1u))
                unit = mem_read32(ctx, unit + 0x14);
            if (!unit)
                return 0;
            mem_write32(ctx, unit + 8, mem_read32(ctx, unit + 8) & ~1u);
            cheats_words[CW_STATS + 8]++;
            job.v[0] = mem_read32(ctx, unit + 0x14);
            ch_issue(ctx, G_UPDATE_ACTOR, unit, 0, 0, 0);
            return 1;
        }
        if (job.kind == K_VEINS) {
            /* cheats2: each vein used is made ready again with the game's own
               Data::Enable (its glow comes back; the terrain it changed stays
               changed, since a vein only runs its map's event when used) */
            while (job.i < veins_count && job.i < VEIN_MAX) {
                uint32_t data = veins_used[job.i++], trick = mem_read32(ctx, A_TRICKS), d = trick ? mem_read32(ctx, trick) : 0;
                for (uint32_t guard = 0; d && d != data && guard < 512; guard++)
                    d = mem_read32(ctx, d);
                if (!d || mem_read8(ctx, data + 0xC) != TRICK_VEIN || (mem_read8(ctx, data + 0x16) & 7) != 2)
                    continue;
                cheats_words[CW_STATS + 13]++;
                ch_issue(ctx, G_TRICK_ENABLE, data, 0, 0, 0);
                return 1;
            }
            veins_count = 0;
            return 0;
        }
        if (job.kind == K_RECRUIT) {
            /* cheats2: a captured unit joins the army the way the Prison's
               Scout makes a prisoner join (ProcJail::InitAfterCreateUnit and
               JoinTargetPrisoner): a unit from the prisoner's packet,
               NormalizeForCastleJoinGuest, Transfer to the army, the Unit
               Logbook */
            uint32_t unit = job.v[0], entry = job.v[1];
            job.step++;
            switch (s) {
            case 0: {
                entry = recruits[0];
                for (uint32_t i = 1; i < recruits_count; i++)
                    recruits[i - 1] = recruits[i];
                recruits_count--;
                unit = ch_free_unit(ctx);
                job.v[1] = entry;
                if (!entry)
                    return 0;
                if (!unit) {
                    /* the army is full: the prisoner goes to the Jail, as without the cheat */
                    uint32_t castle = mem_read32(ctx, A_CASTLE), data = castle ? mem_read32(ctx, castle + 8) : 0;
                    uint32_t jail = data ? mem_read32(ctx, data + 0x14) : 0;
                    uint32_t count = jail ? mem_read32(ctx, jail + 8) : 10;
                    if (count < 10) {
                        mem_write32(ctx, mem_read32(ctx, jail + 0xC) + count * 4, entry);
                        mem_write32(ctx, jail + 8, count + 1);
                    }
                    cheats_words[CW_STATS + 15]++;
                    return 0;
                }
                job.v[0] = unit;
                ch_issue(ctx, G_CREATE_FROM_PACKET, unit, entry + 8, 0, 0);
                return 1;
            }
            case 1: ch_issue(ctx, G_NORMALIZE_GUEST, unit, 0, 0, 0); return 1;
            case 2: ch_issue(ctx, G_TRANSFER, unit, 3, 1, 0); return 1;
            case 3:
                ch_mark(ctx, unit); /* online2: joined through the cheat */
                for (uint32_t at = 0; at < 0x180; at += 4)
                    mem_write32(ctx, SCRATCH + at, 0);
                ch_issue(ctx, G_TO_PACKET, unit, SCRATCH, 0, 0);
                return 1;
            case 4: {
                uint32_t pool = mem_read32(ctx, A_SUPPORT_POOL);
                if (!pool) {
                    job.step = 6;
                    continue;
                }
                job.v[2] = pool + 0x30;
                ch_issue(ctx, G_SUPPORT_SEARCH, job.v[2], SCRATCH, 0, 0);
                return 1;
            }
            case 5:
                if (r)
                    continue;
                ch_issue(ctx, G_SUPPORT_ENTRY, job.v[2], SCRATCH, 0, 0);
                return 1;
            case 6: {
                /* the prisoner's entry, deleted as Jail::Remove does (its virtual destructor) */
                uint32_t vt = mem_read32(ctx, entry), dtor = vt ? mem_read32(ctx, vt + 4) : 0;
                cheats_words[CW_STATS + 14]++;
                cheats_words[CW_STATS + 16] = unit;
                if (!dtor)
                    return 0;
                ch_issue(ctx, dtor, entry, 0, 0, 0);
                return 1;
            }
            }
            return 0;
        }
        /* a request */
        uint32_t a0 = job.a[0], a1 = job.a[1], a2 = job.a[2];
        switch (job.op) {
        case OP_CLASS:
            if (s == 0) {
                if (!ch_valid_unit(ctx, a0) || !a1)
                    return 0;
                job.step = 1;
                ch_issue(ctx, G_CLASS_CHANGE, a0, a1, 1, 0);
                return 1;
            }
            if (s == 1) {
                /* a weapon type the new class uses but the unit never had:
                   rank E (1), as every unit's unused types read */
                for (int k = 0; k < 8; k++)
                    if (mem_read8(ctx, a1 + 0x44 + k) && !mem_read8(ctx, a0 + 0xE8 + k))
                        mem_write8(ctx, a0 + 0xE8 + k, 1);
                job.step = 2;
                ch_issue(ctx, G_UPDATE_ACTOR, a0, 0, 0, 0);
                return 1;
            }
            job.result = 1;
            return 0;
        case OP_JOIN:
        case OP_JOIN_GUEST:
            if (s == 0) {
                if (!a0)
                    return 0;
                job.step = 1;
                ch_issue(ctx, G_MASK_SAME_FORCE, 0, 0, 0, 0);
                return 1;
            }
            if (s == 1) {
                job.step = 2;
                ch_issue(ctx, G_GET_FROM_PERSON, a0, r, 0, 0);
                return 1;
            }
            if (s == 2) {
                if (r) {
                    job.result = 2; /* already in the army */
                    return 0;
                }
                job.v[0] = ch_free_unit(ctx);
                if (!job.v[0]) {
                    job.result = 3; /* no room */
                    return 0;
                }
                if (job.op == OP_JOIN_GUEST) {
                    job.step = 20;
                    ch_issue(ctx, G_EVENT_INSTANCE, 0, 0, 0, 0);
                    return 1;
                }
                job.step = 3;
                ch_issue(ctx, G_CREATE_CASTLE_JOIN, job.v[0], a0, 0, 0);
                return 1;
            }
            if (s == 3) {
                job.step = 4;
                ch_issue(ctx, G_TRANSFER, job.v[0], 3, 1, 0);
                return 1;
            }
            if (s == 4) {
                job.step = 5;
                ch_issue(ctx, G_NORMALIZE_FRIEND, job.v[0], 0, 0, 0);
                return 1;
            }
            if (s == 5) {
                /* the Unit Logbook, as JoinUniqueGuest enters a unit */
                for (uint32_t at = 0; at < 0x180; at += 4)
                    mem_write32(ctx, SCRATCH + at, 0);
                job.step = 6;
                ch_issue(ctx, G_TO_PACKET, job.v[0], SCRATCH, 0, 0);
                return 1;
            }
            if (s == 6) {
                uint32_t pool = mem_read32(ctx, A_SUPPORT_POOL);
                job.result = 1;
                if (!pool)
                    return 0;
                job.v[1] = pool + 0x30;
                job.step = 7;
                ch_issue(ctx, G_SUPPORT_SEARCH, job.v[1], SCRATCH, 0, 0);
                return 1;
            }
            if (s == 7) {
                if (r)
                    return 0;
                job.step = 8;
                ch_issue(ctx, G_SUPPORT_ENTRY, job.v[1], SCRATCH, 0, 0);
                return 1;
            }
            if (s == 20) {
                job.step = 21;
                ch_issue(ctx, G_JOIN_UNIQUE_GUEST, r, a0, 0, 0);
                return 1;
            }
            job.result = 1;
            return 0;
        case OP_SUPPORT:
            if (!ch_valid_unit(ctx, a0) || !ch_valid_unit(ctx, a1))
                return 0;
            job.step = s + 1;
            switch (s) {
            case 0: ch_issue(ctx, G_SET_RELIANCE_LEVEL, a0, a1, a2, 0); return 1;
            case 1: ch_issue(ctx, G_SET_RELIANCE_LEVEL, a1, a0, a2, 0); return 1;
            case 2: ch_issue(ctx, G_UPDATE_LIMIT, a0, 0, 0, 0); return 1;
            case 3: ch_issue(ctx, G_UPDATE_LIMIT, a1, 0, 0, 0); return 1;
            }
            job.result = 1;
            return 0;
        case OP_LEVEL_UP:
            if (!ch_valid_unit(ctx, a0))
                return 0;
            if (s == 0) {
                job.step = 1;
                ch_issue(ctx, G_LIMIT_LEVEL, a0, 0, 0, 0);
                return 1;
            }
            if (s == 1) {
                if (mem_read8(ctx, a0 + 0xF1) < r && job.i < a1 && job.i < 100) {
                    job.i++;
                    job.step = 0;
                    ch_issue(ctx, G_LEVEL_UP, a0, 0, 0, 0);
                    return 1;
                }
                if (mem_read8(ctx, a0 + 0xF1) >= r)
                    mem_write8(ctx, a0 + 0xF2, 0);
                job.step = 2;
                ch_issue(ctx, G_MHP, a0, 1, 0, 0);
                return 1;
            }
            if (s == 2) {
                /* full HP after the levels */
                mem_write8(ctx, a0 + 0xF3, (uint8_t)r);
                job.step = 3;
                ch_issue(ctx, G_UPDATE_ACTOR, a0, 0, 0, 0);
                return 1;
            }
            job.result = 1;
            return 0;
        case OP_CONVOY: {
            uint32_t table = mem_read32(ctx, A_TRANSPORTER);
            uint32_t want = a0 ? a0 : 99, count = cw(CW_ITEMS);
            if (!table)
                return 0;
            if (s == 0) {
                if (job.i >= count || job.i >= CW_COUNT - CW_ITEMS - 1) {
                    job.result = job.v[0] + 1;
                    return 0;
                }
                uint32_t id = cw(CW_ITEMS + 1 + job.i) & 0xFFFF;
                if (!id) {
                    job.i++;
                    continue;
                }
                mem_write32(ctx, SCRATCH, 0);
                job.step = 1;
                ch_issue(ctx, G_ITEM_NEW, SCRATCH, id, 0, 0);
                return 1;
            }
            if (s == 1) {
                uint32_t id = mem_read16(ctx, SCRATCH), have = 0;
                for (uint32_t k = 0; k < 500; k++)
                    if (mem_read16(ctx, table + k * 8) == id) {
                        have = mem_read32(ctx, table + k * 8 + 4);
                        break;
                    }
                if (id && have < want) {
                    job.step = 2;
                    ch_issue(ctx, G_TRANSPORTER_ADD, SCRATCH, want - have, 0, 0);
                    return 1;
                }
                job.i++;
                job.step = 0;
                continue;
            }
            if ((int32_t)r >= 0)
                job.v[0]++;
            job.i++;
            job.step = 0;
            continue;
        }
        case OP_CHAPTER:
            if (s == 0) {
                /* only from My Castle's free roam, where Next Battle starts one */
                uint32_t castle = mem_read32(ctx, A_CASTLE_MAIN);
                uint32_t free_move = mem_read32(ctx, A_FREE_MOVE);
                uint32_t user = mem_read32(ctx, A_GAME_USER);
                if (!castle || !free_move || !user || !a0 || mem_read32(ctx, A_SITUATION)) {
                    job.result = 2;
                    return 0;
                }
                mem_write32(ctx, user + 0xC, a0);
                if (a1 < 3)
                    mem_write8(ctx, user + 0x2E, (uint8_t)a1);
                job.v[0] = free_move;
                job.step = 1;
                ch_issue(ctx, G_SET_RESULT, 5, 0xFFFFFFFFu, 0, 0);
                return 1;
            }
            if (s == 1) {
                job.step = 2;
                ch_issue(ctx, G_JUMP_IMM, job.v[0], 6, 0, 0);
                return 1;
            }
            job.result = 1;
            return 0;
        case OP_RECORDS:
            if (s == 0) {
                uint32_t free_move = mem_read32(ctx, A_FREE_MOVE);
                if (!free_move || mem_read32(ctx, A_SITUATION)) {
                    job.result = 2;
                    return 0;
                }
                job.step = 1;
                ch_issue(ctx, 0x0018B8A4u, free_move, 0, a0 ? a0 : 1, 0);
                return 1;
            }
            job.result = 1;
            return 0;
        case OP_ITEM:
            if (!ch_valid_unit(ctx, a0) || !a1)
                return 0;
            if (s == 0) {
                mem_write32(ctx, SCRATCH, 0);
                job.step = 1;
                ch_issue(ctx, G_ITEM_NEW, SCRATCH, a1, 0, 0);
                return 1;
            }
            if (s == 1) {
                job.step = 2;
                ch_issue(ctx, G_ITEM_ADD, a0, SCRATCH, 0, 0);
                return 1;
            }
            job.result = 1;
            return 0;
        case OP_BUILDING:
            if (s == 0) {
                /* cheats2: from My Castle's free roam, one building at a time */
                uint32_t free_move = mem_read32(ctx, A_FREE_MOVE);
                if (!free_move || mem_read32(ctx, A_SITUATION) || mem_read32(ctx, A_BUILD_SEQ) || !mem_read32(ctx, A_CASTLE_MAIN)) {
                    job.result = 2;
                    return 0;
                }
                job.step = 1;
                cheats_words[CW_STATS + 17]++;
                ch_issue(ctx, G_LEVEL_UP_BUILDING, free_move, a0, 0, 0);
                return 1;
            }
            job.result = 1;
            return 0;
        case OP_ICON:
            if (s == 0 && ch_valid_unit(ctx, a0)) {
                job.step = 1;
                ch_issue(ctx, G_UPDATE_ACTOR, a0, 0, 0, 0);
                return 1;
            }
            job.result = 1;
            return 0;
        }
        return 0;
    }
}

/* the job is over: Proc::Exec runs as it was called */
static void ch_finish(Context *ctx) {
    if (job.kind == K_REQUEST) {
        cheats_words[CW_RESULT] = job.result;
        cheats_words[CW_DONE]++;
    }
    ctx->r[0] = job.prio;
    ctx->r[13] = job.sp_entry;
    ctx->r[14] = job.lr;
    job.active = 0;
}

/* which job to start now, if any */
static uint32_t ch_next_kind(Context *ctx) {
    if (cw(CW_REQ))
        return K_REQUEST;
    if (recruits_count)
        return K_RECRUIT;
    if (cw(CW_FLAGS) & F_VEIN_REUSE) {
        /* cheats2: the player phase begins (the phase byte goes back to 0) */
        uint32_t situation = mem_read32(ctx, A_SITUATION);
        uint32_t phase = situation ? mem_read8(ctx, situation + 8) : 0xFF;
        uint32_t was = veins_phase;
        veins_phase = phase;
        if (!situation)
            veins_count = 0;
        else if (phase == 0 && was != 0 && veins_count)
            return K_VEINS;
    }
    if (cw(CW_FLAGS) & F_MOVE_AGAIN) {
        uint32_t situation = mem_read32(ctx, A_SITUATION);
        uint32_t frame = mem_read32(ctx, A_FRAME), free_frame = cw(CW_FRAME);
        uint32_t forces = mem_read32(ctx, A_FORCES);
        if (situation && forces && mem_read8(ctx, situation + 8) == 0 && free_frame && frame - free_frame <= 1) {
            uint32_t guard = 0;
            for (uint32_t unit = mem_read32(ctx, forces); unit && guard < 256; unit = mem_read32(ctx, unit + 0x14), guard++)
                if (mem_read32(ctx, unit + 8) & 1u)
                    return K_AGAIN;
        }
    }
    static uint32_t viewed_tick;
    if (cw(CW_VIEW_UNIT) && cw(CW_VIEW_TICK) != viewed_tick && ch_valid_unit(ctx, cw(CW_VIEW_UNIT))) {
        viewed_tick = cw(CW_VIEW_TICK);
        return K_VIEW;
    }
    return K_NONE;
}

/* Proc::Exec(Proc::Priority): the jobs, before the procs */
RECOMP_OVERRIDE(0x001052B8) {
    if (!job.active && (cw(CW_REQ) || (cw(CW_FLAGS) & (F_MOVE_AGAIN | F_VEIN_REUSE)) || cw(CW_VIEW_UNIT) || recruits_count)) {
        uint32_t kind = ch_next_kind(ctx);
        if (kind) {
            memset(&job, 0, sizeof job);
            job.active = 1;
            job.tls = ctx->tls;
            job.lr = ctx->r[14];
            job.sp_entry = ctx->r[13];
            job.sp = ctx->r[13] & ~7u;
            job.prio = ctx->r[0];
            job.kind = kind;
            if (kind == K_REQUEST) {
                job.op = cw(CW_REQ);
                for (int i = 0; i < 4; i++)
                    job.a[i] = cw(CW_ARG + i);
                cheats_words[CW_REQ] = 0;
                cheats_words[CW_RUNS]++;
            } else if (kind == K_VIEW) {
                job.a[0] = cw(CW_VIEW_UNIT);
            } else {
                job.v[0] = mem_read32(ctx, mem_read32(ctx, A_FORCES));
            }
            if (ch_step(ctx, 0))
                return;
            ch_finish(ctx);
        }
    }
    CALL(RECOMP_ORIGINAL(0x001052B8));
}

/* LinkBitArray512::Dump(void) const: unreferenced in the game (a release
   build's empty debug dump), the job's return address (JOB_RET). One of
   the job's calls returned: the next step, or at the end Proc::Exec runs
   as it was called. Anything else runs the original. */
RECOMP_OVERRIDE(0x0050BBA0) {
    if (job.active && ctx->tls == job.tls && ctx->r[13] == CALL_SP) {
        if (ch_step(ctx, ctx->r[0]))
            return;
        ch_finish(ctx);
        CALL(RECOMP_ORIGINAL(0x001052B8));
        return;
    }
    CALL(RECOMP_ORIGINAL(0x0050BBA0));
}

/* ---- battle --------------------------------------------------------------- */

/* the AI's battle simulations (AIBattleSimulator::Calculate and
   AIInterferenceSimulator::Calculate, the only direct callers of
   BattleInfo::Calculate besides the arena) run on the AI's thread while it
   decides; while one runs on a thread, that thread's numbers are left as
   the game has them */
static uint32_t ch_sim_tls, ch_sim_depth;

static int ch_simulating(Context *ctx) {
    return ch_sim_depth && ctx->tls == ch_sim_tls;
}

static void ch_sim_done(Context *ctx, const uint32_t *words) {
    (void)ctx;
    (void)words;
    if (ch_sim_depth)
        ch_sim_depth--;
}

static int ch_battle_on(void) {
    return cw(CW_CRIT) || (cw(CW_FLAGS) & (F_HIT | F_MISS | F_GOD | F_OHKO));
}

/* AIBattleSimulator::Calculate(Unit*, Unit*, int, int, int, int) */
RECOMP_OVERRIDE(0x001F7F90) {
    if (!ch_battle_on()) {
        CALL(RECOMP_ORIGINAL(0x001F7F90));
        return;
    }
    ch_sim_tls = ctx->tls;
    ch_sim_depth++;
    CALL_THEN(RECOMP_ORIGINAL(0x001F7F90), ch_sim_done, 0);
}

/* AIInterferenceSimulator::Calculate(Unit*, Unit*, int, int, int, int) */
RECOMP_OVERRIDE(0x0021F0AC) {
    if (!ch_battle_on()) {
        CALL(RECOMP_ORIGINAL(0x0021F0AC));
        return;
    }
    ch_sim_tls = ctx->tls;
    ch_sim_depth++;
    CALL_THEN(RECOMP_ORIGINAL(0x0021F0AC), ch_sim_done, 0);
}

static void ch_battle_patch(Context *ctx, const uint32_t *words) {
    uint32_t info = words[0];
    /* the real battle and its forecast only: the AI's own simulations
       see the game's numbers, so enemies still choose to attack */
    if (ch_simulating(ctx))
        return;
    uint32_t flags = cw(CW_FLAGS), crit = cw(CW_CRIT);
    for (int s = 0; s < 4; s++) {
        uint32_t side = info + s * 0x200;
        uint32_t unit = mem_read32(ctx, side + 4);
        if (!unit)
            continue;
        int player = ch_ours(ctx, unit);
        /* only a side that attacks (it has a weapon in hand: +0x10) */
        if (!mem_read16(ctx, side + 0x10))
            continue;
        if (crit == 2 || (crit == 1 && player)) {
            mem_write32(ctx, side + 0x34, 100);
            cheats_words[CW_STATS]++;
        }
        if (player && (flags & F_HIT)) {
            mem_write32(ctx, side + 0x30, 100);
            cheats_words[CW_STATS + 1]++;
        }
        if (ch_enemy(ctx, unit) && (flags & F_MISS)) {
            mem_write32(ctx, side + 0x30, 0);
            cheats_words[CW_STATS + 1]++;
        }
    }
}

/* map::BattleInfo::CalculateSimple(void) */
RECOMP_OVERRIDE(0x00347C3C) {
    if (!cw(CW_CRIT) && !(cw(CW_FLAGS) & (F_HIT | F_MISS))) {
        CALL(RECOMP_ORIGINAL(0x00347C3C));
        return;
    }
    CALL_THEN(RECOMP_ORIGINAL(0x00347C3C), ch_battle_patch, ctx->r[0]);
}

static void ch_power_patch(Context *ctx, const uint32_t *words) {
    if (ch_simulating(ctx))
        return;
    uint32_t mine = mem_read32(ctx, words[0] + 4), theirs = mem_read32(ctx, words[1] + 4);
    uint32_t flags = cw(CW_FLAGS);
    if (!mine || !theirs)
        return;
    if ((flags & F_GOD) && ch_ours(ctx, theirs) && ch_enemy(ctx, mine)) {
        ctx->r[0] = 0;
        cheats_words[CW_STATS + 2]++;
    } else if ((flags & F_OHKO) && ch_ours(ctx, mine) && ch_enemy(ctx, theirs)) {
        uint32_t hp = mem_read8(ctx, theirs + 0xF3);
        if ((int32_t)ctx->r[0] < (int32_t)hp)
            ctx->r[0] = hp;
        cheats_words[CW_STATS + 2]++;
    }
}

/* map::BattleInfo::Side::GetSimplePower(const Side*, int, int) const */
RECOMP_OVERRIDE(0x005253A8) {
    if (!(cw(CW_FLAGS) & (F_GOD | F_OHKO))) {
        CALL(RECOMP_ORIGINAL(0x005253A8));
        return;
    }
    CALL_THEN(RECOMP_ORIGINAL(0x005253A8), ch_power_patch, ctx->r[0], ctx->r[1]);
}

/* ---- units ---------------------------------------------------------------- */

static void ch_move_patch(Context *ctx, const uint32_t *words) {
    uint32_t move = cw(CW_MOVE) ? cw(CW_MOVE) : 30;
    if (ch_player(ctx, words[0]) && (int32_t)ctx->r[0] < (int32_t)move)
        ctx->r[0] = move;
}

/* Unit::GetMovePower(bool, EnhanceDefinition::Type) const */
RECOMP_OVERRIDE(0x0052D5E4) {
    if (!(cw(CW_FLAGS) & F_MOVE)) {
        CALL(RECOMP_ORIGINAL(0x0052D5E4));
        return;
    }
    CALL_THEN(RECOMP_ORIGINAL(0x0052D5E4), ch_move_patch, ctx->r[0]);
}

static uint32_t ch_levelling; /* the unit Unit::LevelUp runs for */

static void ch_level_done(Context *ctx, const uint32_t *words) {
    (void)ctx;
    (void)words;
    ch_levelling = 0;
}

/* Unit::LevelUp(void) */
RECOMP_OVERRIDE(0x003D8154) {
    if (!(cw(CW_FLAGS) & F_GROWTH) || !ch_player(ctx, ctx->r[0])) {
        CALL(RECOMP_ORIGINAL(0x003D8154));
        return;
    }
    ch_levelling = ctx->r[0];
    ch_mark(ctx, ctx->r[0]); /* online2: its stats grow by the cheat */
    CALL_THEN(RECOMP_ORIGINAL(0x003D8154), ch_level_done, ctx->r[0]);
}

static void ch_grow_patch(Context *ctx, const uint32_t *words) {
    if (ch_levelling && words[0] == ch_levelling) {
        ctx->r[0] = 100;
        cheats_words[CW_STATS + 3]++;
    }
}

/* Unit::GetGrow(int) const */
RECOMP_OVERRIDE(0x005330B4) {
    if (!ch_levelling) {
        CALL(RECOMP_ORIGINAL(0x005330B4));
        return;
    }
    CALL_THEN(RECOMP_ORIGINAL(0x005330B4), ch_grow_patch, ctx->r[0]);
}

static void ch_exp_patch(Context *ctx, const uint32_t *words) {
    uint32_t calc = words[0];
    uint32_t unit = mem_read32(ctx, calc);
    if (!ch_player(ctx, unit))
        return;
    uint32_t bits = mem_read32(ctx, calc + 8);
    float factor;
    memcpy(&factor, &bits, 4);
    factor *= (float)cw(CW_EXP_MULT) / 100.0f;
    ch_mark(ctx, unit); /* online2: it gains EXP by the cheat */
    memcpy(&bits, &factor, 4);
    mem_write32(ctx, calc + 8, bits);
    cheats_words[CW_STATS + 4]++;
}

/* map::ExpFactorCalculator::Calculate(void) */
RECOMP_OVERRIDE(0x0036BF98) {
    if (!(cw(CW_FLAGS) & F_EXP) || cw(CW_EXP_MULT) <= 100) {
        CALL(RECOMP_ORIGINAL(0x0036BF98));
        return;
    }
    CALL_THEN(RECOMP_ORIGINAL(0x0036BF98), ch_exp_patch, ctx->r[0]);
}

static void ch_weapon_patch(Context *ctx, const uint32_t *words) {
    /* S rank for a type the class can't use; 251 is S's experience */
    if (words[1] < 8 && ch_player(ctx, words[0]) && ctx->r[0] < 251)
        ctx->r[0] = 251;
}

/* Unit::GetWeaponExpLimit(ItemKind::Type) const */
RECOMP_OVERRIDE(0x0052F824) {
    if (!(cw(CW_FLAGS) & F_WEAPONS)) {
        CALL(RECOMP_ORIGINAL(0x0052F824));
        return;
    }
    CALL_THEN(RECOMP_ORIGINAL(0x0052F824), ch_weapon_patch, ctx->r[0], ctx->r[1]);
}

static void ch_vein_patch(Context *ctx, const uint32_t *words) {
    if (ch_player(ctx, words[0]))
        ctx->r[0] = 1;
}

/* Unit::CanDragonVein(void) const */
RECOMP_OVERRIDE(0x0052DFE8) {
    if (!(cw(CW_FLAGS) & F_VEINS)) {
        CALL(RECOMP_ORIGINAL(0x0052DFE8));
        return;
    }
    CALL_THEN(RECOMP_ORIGINAL(0x0052DFE8), ch_vein_patch, ctx->r[0]);
}

static int ch_capture_skill(uint32_t skill) {
    skill &= 0xFFFF;
    return skill && (skill == (cw(CW_CAPTURE_SKILL) & 0xFFFF) || skill == (cw(CW_CAPTURE_SKILL + 1) & 0xFFFF));
}

static void ch_skill_patch(Context *ctx, const uint32_t *words) {
    if (ctx->r[0])
        return;
    /* cheats2: Capture for the player's units, as the Prison's skill gives it */
    if ((cw(CW_FLAGS) & F_CAPTURE) && ch_capture_skill(words[1]) && ch_player(ctx, words[0])) {
        ctx->r[0] = 1;
        return;
    }
    uint32_t person = mem_read32(ctx, words[0] + 0x9C);
    if (!person)
        return;
    uint32_t id = mem_read16(ctx, person + 0x24);
    uint32_t want = id << 16 | (words[1] & 0xFFFF);
    uint32_t count = cw(CW_SKILL_COUNT);
    for (uint32_t i = 0; i < count && i < 256; i++)
        if (cw(CW_SKILLS + i) == want) {
            ctx->r[0] = 1;
            return;
        }
}

/* Unit::IsEquipSkill(short) const */
RECOMP_OVERRIDE(0x0052DD04) {
    if ((!cw(CW_SKILL_COUNT) && !((cw(CW_FLAGS) & F_CAPTURE) && ch_capture_skill(ctx->r[1]))) || !ctx->r[1]) {
        CALL(RECOMP_ORIGINAL(0x0052DD04));
        return;
    }
    CALL_THEN(RECOMP_ORIGINAL(0x0052DD04), ch_skill_patch, ctx->r[0], ctx->r[1]);
}

/* ---- the Support Log -------------------------------------------------------- */

/* VariableSizeFlagManager::Get(int): asked by the Support Log's list, every
   conversation reads as seen */
RECOMP_OVERRIDE(0x00220F60) {
    uint32_t from = ctx->r[14];
    if (cw(CW_FLAGS) & F_LOG) {
        cheats_words[CW_STATS + 9]++;   /* Gets seen while the cheat is on */
        cheats_words[CW_STATS + 10] = from;
    }
    if ((cw(CW_FLAGS) & F_LOG) && from >= LOG_FROM && from < LOG_TO) {
        cheats_words[CW_STATS + 7]++;
        ctx->r[0] = 1;
        RETURN_TO(ctx->r[14]);
    }
    CALL(RECOMP_ORIGINAL(0x00220F60));
}

/* PersonEnumerator_Reliance::IsExclusion(const Person*): the Support Log
   asks for each character whether any of their conversations was seen. It
   asks the seen flags (GameUserGlobalData +0x20, a VariableSizeFlagManager)
   only for an index under the manager's size (+4), so while it runs with the
   cheat on that size reads as the largest, and VariableSizeFlagManager::Get
   above answers "seen"; the size is put back right after. Nothing is
   written to the flags themselves, so the saves' seen list is unchanged. */
#define A_GLOBAL_USER 0x006DA27Cu

static void ch_log_done(Context *ctx, const uint32_t *words) {
    if (words[0])
        mem_write32(ctx, words[0] + 4, words[1]);
    if (ctx->r[0] == 0)
        cheats_words[CW_STATS + 12]++; /* characters the log lists */
}

RECOMP_OVERRIDE(0x002220B8) {
    if (!(cw(CW_FLAGS) & F_LOG)) {
        CALL(RECOMP_ORIGINAL(0x002220B8));
        return;
    }
    cheats_words[CW_STATS + 11]++;
    uint32_t global = mem_read32(ctx, A_GLOBAL_USER);
    uint32_t flags = global ? mem_read32(ctx, global + 0x20) : 0;
    uint32_t size = flags ? mem_read32(ctx, flags + 4) : 0;
    if (flags)
        mem_write32(ctx, flags + 4, 0x7FFFFFFFu);
    CALL_THEN(RECOMP_ORIGINAL(0x002220B8), ch_log_done, flags, size);
}

/* ---- seals ------------------------------------------------------------------ */

static int ch_is_seal(uint32_t id) {
    for (int i = 0; i < 8; i++) {
        uint32_t seal = cw(CW_SEALS + i);
        if (seal && seal == id)
            return 1;
    }
    return 0;
}

/* unit::Item::Expend(void): it answers 1 when the item is used up (its
   caller then takes it away), 0 when uses are left. cheats2: a seal answers
   0 with its uses untouched; round 1 left r0 as the item's address, which
   read as "used up", so the seal was still taken away. */
RECOMP_OVERRIDE(0x004199BC) {
    if ((cw(CW_FLAGS) & F_SEALS) && ch_is_seal(mem_read16(ctx, ctx->r[0]))) {
        cheats_words[CW_STATS + 5]++;
        ctx->r[0] = 0;
        RETURN_TO(ctx->r[14]);
    }
    CALL(RECOMP_ORIGINAL(0x004199BC));
}

/* Transporter::Sub(int index, int count) */
RECOMP_OVERRIDE(0x001A7460) {
    if (cw(CW_FLAGS) & F_SEALS) {
        uint32_t table = mem_read32(ctx, A_TRANSPORTER);
        if (table && ctx->r[0] < 500 && ch_is_seal(mem_read16(ctx, table + ctx->r[0] * 8))) {
            cheats_words[CW_STATS + 5]++;
            RETURN_TO(ctx->r[14]);
        }
    }
    CALL(RECOMP_ORIGINAL(0x001A7460));
}

/* ---- the turn counter ---------------------------------------------------- */

static void ch_turn_back(Context *ctx, const uint32_t *words) {
    uint32_t situation = words[0];
    uint32_t turn = mem_read16(ctx, situation + 0xC);
    if (turn > words[1]) {
        mem_write16(ctx, situation + 0xC, (uint16_t)words[1]);
        cheats_words[CW_STATS + 6]++;
    }
}

/* map::Situation::TurnEnd(void) */
RECOMP_OVERRIDE(0x003A6850) {
    if (!(cw(CW_FLAGS) & F_TURN)) {
        CALL(RECOMP_ORIGINAL(0x003A6850));
        return;
    }
    CALL_THEN(RECOMP_ORIGINAL(0x003A6850), ch_turn_back, ctx->r[0], mem_read16(ctx, ctx->r[0] + 0xC));
}

/* ---- cheats2: Dragon Veins used again ------------------------------------------ */

/* map::trick::Data::Done(ProcInst*): a Dragon Vein is marked used by its
   map's own script (TrickStringSet / TrickSet, which the vein's event runs
   when a unit uses it), so a vein that is made done while one of the
   player's units stands on it is noted, to be made ready again at the next
   player phase. The vein's own Done runs as always. */
static int ch_player_at(Context *ctx, int x, int y) {
    uint32_t forces = mem_read32(ctx, A_FORCES), guard = 0;
    for (uint32_t unit = forces ? mem_read32(ctx, forces) : 0; unit && guard < 256; unit = mem_read32(ctx, unit + 0x14), guard++)
        if ((int8_t)mem_read8(ctx, unit + 0xF4) == x && (int8_t)mem_read8(ctx, unit + 0xF5) == y)
            return 1;
    return 0;
}

RECOMP_OVERRIDE(0x0039AAA8) {
    uint32_t data = ctx->r[0];
    if ((cw(CW_FLAGS) & F_VEIN_REUSE) && data && mem_read8(ctx, data + 0xC) == TRICK_VEIN &&
        ch_player_at(ctx, (int8_t)mem_read8(ctx, data + 8), (int8_t)mem_read8(ctx, data + 9))) {
        int known = 0;
        for (uint32_t i = 0; i < veins_count; i++)
            known |= veins_used[i] == data;
        if (!known && veins_count < VEIN_MAX)
            veins_used[veins_count++] = data;
        cheats_words[CW_STATS + 19]++;
    }
    CALL(RECOMP_ORIGINAL(0x0039AAA8));
}

/* ---- cheats2: recruiting captured units -------------------------------------------- */

/* the prisoner Jail::Append made (r0) is taken back out of the Jail at once,
   to join the army at the next Proc::Exec (K_RECRUIT) */
static void ch_capture_tail(Context *ctx, const uint32_t *words) {
    uint32_t jail = words[0], entry = ctx->r[0];
    uint32_t count = mem_read32(ctx, jail + 8), array = mem_read32(ctx, jail + 0xC);
    if (!entry || !count || mem_read32(ctx, array + (count - 1) * 4) != entry)
        return;
    mem_write32(ctx, array + (count - 1) * 4, 0);
    mem_write32(ctx, jail + 8, count - 1);
    if (recruits_count < RECRUIT_MAX)
        recruits[recruits_count++] = entry;
}

/* map::SequenceBattle::ProcSequence::CaptureReflect(void): a unit was
   captured (side +0x28 bit 0x4000 on the capturer, 0x400 on the defeated).
   Without the cheat the game puts it in the Jail, when a Prison was built
   (the Jail's capacity, +0, is the Prison's level x 2). With it, the
   prisoner is made by the game's own Jail::Append (its packet, the captured
   name, CaptureNormalize) whatever the capacity, then joins the army. */
RECOMP_OVERRIDE(0x00357ED0) {
    if (!(cw(CW_FLAGS) & F_CAPTURE)) {
        CALL(RECOMP_ORIGINAL(0x00357ED0));
        return;
    }
    uint32_t seq = ctx->r[0];
    uint32_t a = mem_read32(ctx, seq + 0x24C), b = mem_read32(ctx, seq + 0x250);
    uint32_t castle = mem_read32(ctx, A_CASTLE), data = castle ? mem_read32(ctx, castle + 8) : 0;
    uint32_t jail = data ? mem_read32(ctx, data + 0x14) : 0;
    if (!a || !b || !jail || !(mem_read32(ctx, a + 0x28) & 0x4000) || !(mem_read32(ctx, b + 0x28) & 0x400) ||
        mem_read32(ctx, jail + 8) >= 10 || recruits_count >= RECRUIT_MAX) {
        CALL(RECOMP_ORIGINAL(0x00357ED0));
        return;
    }
    uint32_t unit = mem_read32(ctx, a + 0x204);
    cheats_words[CW_STATS + 18]++;
    ctx->r[0] = jail;
    ctx->r[1] = unit;
    ctx->r[15] = G_JAIL_APPEND; /* it returns straight to CaptureReflect's caller (r14) */
    ctx->thumb = 0;
    CALL_THEN(recomp_call, ch_capture_tail, jail);
}

/* the Jail's capacity put back after a Capture target search */
static void ch_capacity_back(Context *ctx, const uint32_t *words) {
    mem_write16(ctx, words[0], (uint16_t)words[1]);
}

/* map::Target::EnumerateAttack(int x, int y, unsigned short, Attack::Type)
   (the type on the stack): for Capture (type 2) a target counts only when
   the Jail has a capacity (a Prison was built) and Unit::CanCaptured. With
   the cheat, the capacity reads as the first Prison's (2) while the
   player's unit looks for targets, and is put back at once, so the save's
   Jail is unchanged. Bosses and monsters are still refused by CanCaptured. */
RECOMP_OVERRIDE(0x003A134C) {
    if (!(cw(CW_FLAGS) & F_CAPTURE) || mem_read32(ctx, ctx->r[13]) != 2 || !ch_player(ctx, mem_read32(ctx, ctx->r[0]))) {
        CALL(RECOMP_ORIGINAL(0x003A134C));
        return;
    }
    uint32_t castle = mem_read32(ctx, A_CASTLE), data = castle ? mem_read32(ctx, castle + 8) : 0;
    uint32_t jail = data ? mem_read32(ctx, data + 0x14) : 0;
    if (!jail || mem_read16(ctx, jail)) {
        CALL(RECOMP_ORIGINAL(0x003A134C));
        return;
    }
    mem_write16(ctx, jail, 2);
    cheats_words[CW_STATS2]++;
    CALL_THEN(RECOMP_ORIGINAL(0x003A134C), ch_capacity_back, jail, 0);
}
