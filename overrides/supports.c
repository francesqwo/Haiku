/* supports: Corrin can S-support anyone, and Kana's gender can be chosen.
 *
 * The work is the host's (zakuro-core/src/supports.rs); these overrides only
 * tell it when something happens and let it answer. With both features off
 * (or no hook) every override runs the original alone.
 *
 * Supports are "Reliance" in the game's code. Each character has a list of
 * 12-byte entries in GameData (Person::Initialize loads them): +0 the
 * partner's person id, +4..+7 the points C, B, A and S need (0xFF: no such
 * rank), +9/+10 the pair's conversation index (C, then B, A, S). Marriage
 * (Unit::GetMarrigePid) is any pair at S, whatever the genders. So:
 *
 * - Person::Initialize (0x0044A04C), after: the host fills in the S rank of
 *   Corrin's pairs that only the other Corrin has, with that Corrin's
 *   threshold, in both lists of the pair.
 * - ProcSequence::Talk (0x003CD230): a support conversation starts. A new
 *   same-gender S would mark the pair's conversation index + 3 as seen,
 *   which is another pair's C. The host remembers the pair, and
 *   VariableSizeFlagManager::Set (0x00220F8C) marks the other Corrin's S
 *   with the same partner instead: the same conversation, which the support
 *   log then shows.
 * - ProcMuseum::CreateReliance / ExitReliance (0x00189730 / 0x00188F14): the
 *   support log (Extras) lists the game's own pairs, so the S ranks are put
 *   back as the game has them while it is open.
 * - IdentHash::GetIdent (0x00127330) on the message table: the host may
 *   answer a label with one of Haiku's own variants (Kana's paralogue with
 *   Kana's chosen gender, Live2D lines with Corrin's gender).
 * - Person::IsFemale (0x0053ED90), called from Spot::UpdateSpot for a
 *   child's paralogue (returning to 0x003D2F88): the game opens it only when
 *   the fixed parent and the spouse are of different genders (r8 holds the
 *   parent's). With S for anyone on, the spouse answers as the other gender,
 *   so every marriage opens the child's paralogue.
 * - ev::UnitIsPID (0x003B0FE4): in Kana's paralogue (X002), "is the player
 *   PID_プレイヤー女/男?" is answered as if Corrin were the parent the chosen
 *   Kana has in the game (male Kana: a female Corrin), which picks Kana's
 *   dispos group, chapter flag and dragon class.
 * - cmvm::CmContext::CallCFunc (0x003DB668): the script calls a native.
 *   In X002, when Kana's gender is to be asked, Kana's first line
 *   (ev::Talk("MID_X002_OP1_1"), in the opening cutscene once its scenery
 *   has faded in) is held back: the
 *   script's pc (CmContext +0x10) goes back to its call instruction (4
 *   bytes: opcode, name, argc), its arguments stay on the script's stack,
 *   and the game's own three-choice event dialog (ev::Dialog3, 0x003AF92C)
 *   runs in its place. Dialog3 makes the script wait (+0x21) until a choice
 *   is made, which goes in the script value S_イベント選択肢; the script
 *   then runs the call again, and the scene goes on with the chosen Kana.
 *   (A cutscene in another chapter, ev::CutScene, lets the host forget the
 *   choice for the next run of the paralogue.)
 *   Dialog3's fifth argument is on the stack: it returns into
 *   debug::Menu::MoveToNext (0x00421114, used by the debug menus only,
 *   never reached in the retail game), whose override takes it off and
 *   returns to the script engine.
 */
#include "overrides.h"

typedef uint32_t (*HaikuSupportsHook)(Context *ctx, uint32_t op, uint32_t a, uint32_t b);
/* set by Haiku's main before the game's code runs */
RECOMP_EXPORT HaikuSupportsHook haiku_supports_hook = 0;
/* bit 0: message lookups may be answered with Haiku's variants; bit 1:
   Corrin's S with anyone is on */
RECOMP_EXPORT volatile uint32_t haiku_supports_text = 0;
#define TEXT_ON 1u
#define S_ANY_ON 2u

/* the same numbers as zakuro_core::supports::op */
enum {
    SP_PERSON_INIT = 1,
    SP_TALK = 2,
    SP_FLAG_SET = 3,
    SP_MUSEUM_ENTER = 4,
    SP_MUSEUM_EXIT = 5,
    SP_IDENT = 6,
    SP_IS_PID = 7,
    SP_CALL_C = 8,
    SP_ASK_TEXT = 9,
};

#define MESS_HASH 0x006DCB44u  /* Mess's IdentHash* */
#define DIALOG3 0x003AF92Cu    /* (anonymous)::Dialog3(CmContext*, title, a, b, c) */
#define TRAMP_BACK 0x00421114u
#define CUT_SCENE 0x002097B8u   /* ev::CutScene's function: BeginScene(CmContext*, name) */
#define EV_TALK 0x003AF36Cu     /* ev::Talk(CmContext*, mid) */
#define VM_PC 0x10u            /* CmContext: the script's pc */
#define VM_SP 0x18u            /* CmContext: the script's stack (grows up) */
#define VM_CALL_SIZE 4u        /* the call instruction: 0x47, name (2), argc */

static uint32_t supports_call(Context *ctx, uint32_t op, uint32_t a, uint32_t b) {
    uint32_t saved[4] = {ctx->r[0], ctx->r[1], ctx->r[2], ctx->r[3]};
    uint32_t v = haiku_supports_hook(ctx, op, a, b);
    ctx->r[0] = saved[0]; ctx->r[1] = saved[1]; ctx->r[2] = saved[2]; ctx->r[3] = saved[3];
    return v;
}

static void supports_person_init(Context *ctx, const uint32_t *words) {
    (void)words;
    if (haiku_supports_hook)
        supports_call(ctx, SP_PERSON_INIT, 0, 0);
}

/* Person::Initialize(const void*) */
RECOMP_OVERRIDE(0x0044A04C) {
    if (!haiku_supports_hook) {
        CALL(RECOMP_ORIGINAL(0x0044A04C));
        return;
    }
    CALL_THEN(RECOMP_ORIGINAL(0x0044A04C), supports_person_init, ctx->r[0]);
}

/* 1 between a support conversation that may be a same-gender S and its
   seen flag */
static uint32_t supports_flag_pending = 0;

/* (anonymous)::ProcSequence::Talk(void) */
RECOMP_OVERRIDE(0x003CD230) {
    if (haiku_supports_hook)
        supports_flag_pending = supports_call(ctx, SP_TALK, ctx->r[0], 0);
    CALL(RECOMP_ORIGINAL(0x003CD230));
}

/* VariableSizeFlagManager::Set(int) */
RECOMP_OVERRIDE(0x00220F8C) {
    if (supports_flag_pending && haiku_supports_hook) {
        supports_flag_pending = 0;
        uint32_t index = supports_call(ctx, SP_FLAG_SET, ctx->r[1], 0);
        if (index != ctx->r[1]) {
            /* the capacity Resize grows to (SetCapacity's field) */
            uint32_t manager = ctx->r[0];
            if ((int32_t)mem_read32(ctx, manager + 4) <= (int32_t)index)
                mem_write32(ctx, manager + 4, index + 1);
            ctx->r[1] = index;
        }
    }
    CALL(RECOMP_ORIGINAL(0x00220F8C));
}

/* ProcMuseum::CreateReliance(void): the support log opens */
RECOMP_OVERRIDE(0x00189730) {
    if (haiku_supports_hook)
        supports_call(ctx, SP_MUSEUM_ENTER, 0, 0);
    CALL(RECOMP_ORIGINAL(0x00189730));
}

/* ProcMuseum::ExitReliance(void): the support log closes */
RECOMP_OVERRIDE(0x00188F14) {
    if (haiku_supports_hook)
        supports_call(ctx, SP_MUSEUM_EXIT, 0, 0);
    CALL(RECOMP_ORIGINAL(0x00188F14));
}

/* IdentHash::GetIdent(const char*) const */
RECOMP_OVERRIDE(0x00127330) {
    if ((haiku_supports_text & TEXT_ON) && haiku_supports_hook && ctx->r[1] != 0 && ctx->r[0] == mem_read32(ctx, MESS_HASH)) {
        uint32_t label = ctx->r[1];
        uint8_t c0 = mem_read8(ctx, label), c1 = mem_read8(ctx, label + 1);
        if (c0 == 'M' && (c1 == 'I' || c1 == 'K')) {
            uint32_t node = supports_call(ctx, SP_IDENT, ctx->r[0], label);
            if (node != 0) {
                ctx->r[0] = node;
                RETURN_TO(ctx->r[14]);
                return;
            }
        }
    }
    CALL(RECOMP_ORIGINAL(0x00127330));
}

/* (anonymous)::UnitIsPID(CmContext*, int unit, const char* pid) */
RECOMP_OVERRIDE(0x003B0FE4) {
    if (haiku_supports_hook) {
        uint32_t v = supports_call(ctx, SP_IS_PID, ctx->r[1], ctx->r[2]);
        if (v < 2) {
            ctx->r[0] = v;
            RETURN_TO(ctx->r[14]);
            return;
        }
    }
    CALL(RECOMP_ORIGINAL(0x003B0FE4));
}

/* the ask's state: where the dialog returns to */
static uint32_t ask_back = 0, ask_return = 0;

/* cmvm::CmContext::CallCFunc(CmFunction*, int argc) */
RECOMP_OVERRIDE(0x003DB668) {
    uint32_t vm = ctx->r[0], fn = ctx->r[1], argc = ctx->r[2];
    uint32_t native = argc >= 1 ? mem_read32(ctx, fn + 4) : 0;
    if (haiku_supports_hook && !ask_back && (native == CUT_SCENE || native == EV_TALK)) {
        uint32_t first = mem_read32(ctx, mem_read32(ctx, vm + VM_SP) - argc * 4);
        if (supports_call(ctx, SP_CALL_C, native, first) == 1) {
            /* the call runs again once the dialog is closed */
            mem_write32(ctx, vm + VM_PC, mem_read32(ctx, vm + VM_PC) - VM_CALL_SIZE);
            ask_back = 1;
            ask_return = ctx->r[14];
            ctx->r[0] = vm;
            ctx->r[1] = supports_call(ctx, SP_ASK_TEXT, 0, 0);
            ctx->r[2] = supports_call(ctx, SP_ASK_TEXT, 1, 0);
            ctx->r[3] = supports_call(ctx, SP_ASK_TEXT, 2, 0);
            uint32_t c = supports_call(ctx, SP_ASK_TEXT, 3, 0);
            ctx->r[13] -= 8;
            mem_write32(ctx, ctx->r[13], c);
            mem_write32(ctx, ctx->r[13] + 4, 0);
            ctx->r[14] = TRAMP_BACK;
            RETURN_TO(DIALOG3);
            return;
        }
    }
    CALL(RECOMP_ORIGINAL(0x003DB668));
}

/* debug::Menu::MoveToNext(int): the same; where Dialog3 returns to */
RECOMP_OVERRIDE(0x00421114) {
    if (!ask_back) {
        CALL(RECOMP_ORIGINAL(0x00421114));
        return;
    }
    ask_back = 0;
    ctx->r[13] += 8;
    RETURN_TO(ask_return);
}

/* Person::IsFemale(void) const */
RECOMP_OVERRIDE(0x0053ED90) {
    if (ctx->r[14] == 0x003D2F88u && (haiku_supports_text & S_ANY_ON)) {
        /* Spot::UpdateSpot, the spouse: the other gender than the parent's */
        ctx->r[0] = ctx->r[8] ? 0 : 1;
        RETURN_TO(ctx->r[14]);
        return;
    }
    CALL(RECOMP_ORIGINAL(0x0053ED90));
}
