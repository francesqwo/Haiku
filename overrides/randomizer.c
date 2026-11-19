/* randomizer: the New Game randomizer (characters and classes swapped
 * around, story scenes included), kept per save.
 *
 * The work is the host's (zakuro-core/src/randomizer): these overrides
 * only tell it when something happens and hand it the game's own buffers.
 * With the randomizer off they cost nothing: the talk, dispos and chapter
 * hooks check haiku_rando_active first and run the original alone, and the
 * save hooks only read memory (they never write unless a randomized run is
 * on), so an ordinary run is the same to the byte.
 *
 * - title::MainMenu::Create (0x0043FE08): the title screen, where a run
 *   ends.
 * - title::ProcTitle::JumpMyUnitEdit (0x00441768): a new game starts (the
 *   difficulty and mode are chosen; the avatar is next).
 * - GameUserData::Deserialize (0x001B3788) / Serialize (0x005096F0): a save
 *   loaded (is it randomized?) and written (keep the seed in it).
 * - Person::Load (0x0044A808): a chapter's person file was loaded and
 *   appended to Person's list; its characters take their new look.
 * - TalkUtil::GetFidForWindow (0x004F29A8): the FID a talk line's window
 *   ($Wm / $Ws) asks for; the portrait and the name plate follow it.
 * - TcdiSound::DisposeVoice (0x00504354): the clip a talk line plays,
 *   which it leaves in ProcTalkManager + 0x129E.
 * - Unit::CreateFromDispos (0x003D60D8): a unit a chapter places; its class
 *   and weapons are fitted before the game reads them.
 * - TalkExpander::ExpandMessage(const wchar_t*) (0x001C009C): a talk line,
 *   before the game expands it into the expander's own buffer (0x2000
 *   characters); the names of characters said in it become the names of
 *   whoever is shown, and gendered words about a swapped character fit
 *   whoever is shown (rando3: done on the line before expansion, where
 *   Corrin's $Nu and $G codes are still codes).
 * - BattleUnitResName::Construct (0x001F996C): every character model; a
 *   cutscene cast named after a swapped character gets a stand-in person.
 * - BattleUnitResHolder::GetSimilarAnim (0x0050D700) and
 *   BattleUnit::PlayAnimForceDirectEndPose (0x00170388): a cutscene clip
 *   the shown model has no file for becomes its nearest one, or standing.
 *
 * The New Game screens (see zakuro-core/src/randomizer/menu.rs). After the
 * difficulty and the mode (title::ProcTitle::CreateModeSelectMenu, label 4
 * of the title proc; Phoenix, Casual and Classic panels), one more step
 * asks how to play: the same mode screen built again by the game, with two
 * panels (phase 2): "Normal" and "Randomizer". Normal goes on as the
 * mode's ACall would have (label 8). Randomizer opens the randomizer
 * screen, the mode screen once more with its three panels (phase 1); A
 * there goes on with that randomizer, B goes back to the Normal /
 * Randomizer step, and B there goes back to the mode screen (phase 0).
 * Everything is the game's own menu: its panels, cursor, sounds, touch and
 * B; only the texts, ACall and BCall are answered here.
 * - FrameManager::Layout::SetItem (0x001B2B24) called from
 *   CreateModeSelectMenu (returning to 0x004423C8): the footer's text, in
 *   phases 1 and 2.
 * - CreateModeSelectMenu (0x0044208C): built with the difficulty byte
 *   (ProcTitle + 0x73) at 0 in phase 1 (all three panels exist) and at 1
 *   in phase 2 (Casual's and Classic's panels only, as on Hard), and the
 *   byte put back after.
 * - the panels' GetGameModeName / GetGameModeHName (phases 1 and 2), their
 *   ACall (all phases: in phase 0 it opens the step, unless the host says
 *   not to, during a replay that does not answer it) and
 *   SelectModeBaseItem::BCall (0x0043DC28; phases 1 and 2).
 */
#include "overrides.h"

typedef uint32_t (*HaikuRandoHook)(Context *ctx, uint32_t op, uint32_t a, uint32_t b);
/* set by Haiku's main before the game's code runs */
RECOMP_EXPORT HaikuRandoHook haiku_rando_hook = 0;
/* 1 while a randomized run is on, written by the host */
RECOMP_EXPORT volatile uint32_t haiku_rando_active = 0;

/* the same numbers as zakuro_core::randomizer::op */
enum {
    RZ_TITLE = 1,
    RZ_NEW_GAME = 2,
    RZ_SAVE_LOADED = 3,
    RZ_SAVE_WRITE = 4,
    RZ_PERSON_LOADED = 5,
    RZ_FID = 6,
    RZ_VOICE = 7,
    RZ_DISPOS_UNIT = 8,
    RZ_TEXT = 9,
    RZ_CAST = 10,
    RZ_CLIP = 11,
    RZ_SPRITE = 12, /* rando3 */
    RZ_BADGE = 13,  /* rando3 */
    RZ_TEXT_LINE = 14, /* rando3 */
    RZ_UNIT_JOB = 15,  /* rando3 */
    RZ_DISPOS_ITEMS = 16, /* rando3 */
    RZ_UNIT_LOOK = 17,    /* corrinswap */
    RZ_ICON_LOOK = 18,
    RZ_FACE_EDIT = 19,
    RZ_MESS_NAME = 20,
    RZ_PERSON_NAME = 21,
    RZ_ALIAS = 22,
    RZ_CREATOR = 23,
    RZ_HAIR = 24,       /* rando4 */
    RZ_HAIR_MODEL = 25,
};

/* corrinswap: the bit of haiku_rando_active that says Corrin swaps places
   with someone (zakuro-core/src/randomizer/corrin.rs) */
#define RZ_CORRIN_ON 0x100u
#define RANDO_CORRIN() (haiku_rando_hook && (haiku_rando_active & RZ_CORRIN_ON))

/* the New Game screens' state: 0 the game's mode screen, 1 the
   randomizer screen, 2 the Normal / Randomizer step, 3 (corrinswap) the
   Corrin step after the randomizer screen; the mode and difficulty chosen
   before them */
static uint32_t rando_phase = 0;
/* corrinswap: the randomizer screen's panel chosen before the Corrin step */
static uint32_t rando_rpanel = 0;
static uint32_t rando_mode = 0;
static uint32_t rando_difficulty = 0;
static uint32_t rando_cursor = 0; /* the mode screen's cursor, put back after */
/* the panels the mode screen (any phase) shows while it is up, else 0, for
   the mouse (see rando_pointer) */
static uint32_t rando_menu_up = 0;
/* the phase the mode screen showing was built in: its texts keep to it
   while it fades out after a choice, whatever comes next */
static uint32_t rando_text_phase = 0;
static void rando_menu_shown(Context *ctx, uint32_t panels);

/* title::MainMenu::Create(ProcInst*) */
RECOMP_OVERRIDE(0x0043FE08) {
    rando_phase = 0;
    rando_text_phase = 0;
    rando_menu_shown(ctx, 0);
    if (haiku_rando_hook)
        haiku_rando_hook(ctx, RZ_TITLE, 0, 0);
    CALL(RECOMP_ORIGINAL(0x0043FE08));
}

/* title::ProcTitle::JumpMyUnitEdit(void) */
RECOMP_OVERRIDE(0x00441768) {
    rando_menu_shown(ctx, 0);
    if (haiku_rando_hook)
        haiku_rando_hook(ctx, RZ_NEW_GAME, ctx->r[0], 0);
    CALL(RECOMP_ORIGINAL(0x00441768));
}

static void rando_save_loaded(Context *ctx, const uint32_t *words) {
    if (haiku_rando_hook)
        haiku_rando_hook(ctx, RZ_SAVE_LOADED, words[0], 0);
}

/* GameUserData::Deserialize(Stream*) */
RECOMP_OVERRIDE(0x001B3788) {
    if (!haiku_rando_hook) {
        CALL(RECOMP_ORIGINAL(0x001B3788));
        return;
    }
    CALL_THEN(RECOMP_ORIGINAL(0x001B3788), rando_save_loaded, ctx->r[0]);
}

/* GameUserData::Serialize(Stream*) const */
RECOMP_OVERRIDE(0x005096F0) {
    if (haiku_rando_hook && haiku_rando_active)
        haiku_rando_hook(ctx, RZ_SAVE_WRITE, ctx->r[0], 0);
    CALL(RECOMP_ORIGINAL(0x005096F0));
}

static void rando_person_loaded(Context *ctx, const uint32_t *words) {
    (void)words;
    if (haiku_rando_hook)
        haiku_rando_hook(ctx, RZ_PERSON_LOADED, 0, 0);
}

/* Person::Load(const char*) */
RECOMP_OVERRIDE(0x0044A808) {
    if (!haiku_rando_hook) {
        CALL(RECOMP_ORIGINAL(0x0044A808));
        return;
    }
    CALL_THEN(RECOMP_ORIGINAL(0x0044A808), rando_person_loaded, 0);
}

static void rando_fid(Context *ctx, const uint32_t *words) {
    if (haiku_rando_hook)
        haiku_rando_hook(ctx, RZ_FID, words[0], words[1]);
}

/* TalkUtil::GetFidForWindow(const wchar_t*, char* out, int size, LocationType*) */
RECOMP_OVERRIDE(0x004F29A8) {
    if (!haiku_rando_hook || !haiku_rando_active) {
        CALL(RECOMP_ORIGINAL(0x004F29A8));
        return;
    }
    CALL_THEN(RECOMP_ORIGINAL(0x004F29A8), rando_fid, ctx->r[1], ctx->r[2]);
}

static void rando_voice(Context *ctx, const uint32_t *words) {
    if (haiku_rando_hook)
        haiku_rando_hook(ctx, RZ_VOICE, words[0], 0);
}

/* TcdiSound::DisposeVoice(const wchar_t*, ProcTalkManager*) */
RECOMP_OVERRIDE(0x00504354) {
    if (!haiku_rando_hook || !haiku_rando_active) {
        CALL(RECOMP_ORIGINAL(0x00504354));
        return;
    }
    CALL_THEN(RECOMP_ORIGINAL(0x00504354), rando_voice, ctx->r[2]);
}

/* Unit::CreateFromDispos(const void*, int) */
RECOMP_OVERRIDE(0x003D60D8) {
    if (haiku_rando_hook && haiku_rando_active)
        haiku_rando_hook(ctx, RZ_DISPOS_UNIT, ctx->r[1], 0);
    CALL(RECOMP_ORIGINAL(0x003D60D8));
}


/* TalkExpander::ExpandMessage(const wchar_t*) (r0 expander, r1 the line).
   rando3: the line is rewritten before the game expands it, not after:
   expanding resolves $Nu (Corrin's name) and $Ggendered,words| (Corrin's
   gender) into plain text, and the names and gendered words of swapped
   characters are told apart from Corrin's by those codes. The host
   answers a copy of the line with names and words changed (in its own
   page), or 0, and the game expands that copy as it would the line. */
RECOMP_OVERRIDE(0x001C009C) {
    if (!haiku_rando_hook || !haiku_rando_active || !ctx->r[1]) {
        CALL(RECOMP_ORIGINAL(0x001C009C));
        return;
    }
    {
        uint32_t line = haiku_rando_hook(ctx, RZ_TEXT_LINE, ctx->r[0], ctx->r[1]);
        if (line)
            ctx->r[1] = line;
    }
    CALL(RECOMP_ORIGINAL(0x001C009C));
}

/* BattleUnitResName::Construct(const Unit*, const Person*, const char* name,
   const Job*, const char* item, unsigned int): every character model. A
   cast named only by a swapped character (a cutscene's Garon) is given a
   stand-in person for whoever is shown, before the game reads the
   arguments (r2, r3 and the stacked job and item) */
RECOMP_OVERRIDE(0x001F996C) {
    /* corrinswap: with Corrin swapped, models with a unit too (the place
       shown as Corrin, Corrin's own) */
    if (haiku_rando_hook && haiku_rando_active && (ctx->r[1] == 0 || (haiku_rando_active & RZ_CORRIN_ON)))
        haiku_rando_hook(ctx, RZ_CAST, 0, 0);
    CALL(RECOMP_ORIGINAL(0x001F996C));
}

/* Cutscene motions. A model's clips come from its own animation sets
   (bs/aset: the character's event set over the gender's Order0 set), and a
   script asks for a clip by name (AnimClipNameTable, 256 clips). A clip the
   model has no file for goes through BattleUnitResHolder::GetSimilarAnim
   (0x0050D700), which for an event clip (0x20 and up) answers Cheer (0x18):
   a victory cheer in the middle of a scene, or nothing at all. A swapped
   character meets that whenever the script asks for a clip only the
   original had (Garon's bow, Azura's songs, a story death), so while the
   randomizer is on an event clip without a file becomes its nearest
   ordinary clip below (a chain, the first one the model has), and failing
   those standing (0x20). BattleUnit::PlayAnimForceDirectEndPose
   (0x00170388), a clip's end pose, does nothing without a file; it gets
   the same answer. dev/reports/randomizer.md has the table in words. */
#define HOLDER_CLIP_FILE 0x3D8u /* BattleUnitResHolder: clip * 8 + 0x3D8, the clip's file */
#define CLIP_STAND       0x20u  /* standing (IdleNormal) */

static const uint8_t rando_clip_fallback[][4] = {
    /* clip, then up to three to try in order */
    {53, 96, 0, 0},     /* step back -> stagger */
    {63, 62, 61, 0},    /* tired 3 -> tired 2, tired 1 */
    {67, 66, 65, 0},    /* peer 3 -> peer 2, peer 1 */
    {71, 73, 0, 0},     /* wake up from sleep -> sit up */
    {75, 126, 0, 0},    /* touch a face -> hold hands */
    {85, 44, 0, 0},     /* deep breath -> look down */
    {87, 99, 0, 0},     /* cast a spell -> raise an arm */
    {89, 126, 0, 0},    /* pat a back -> hold hands */
    {92, 97, 0, 0},     /* move on horseback -> ready */
    {93, 60, 0, 0},     /* talk seated 2 -> talk seated 1 */
    {104, 78, 0, 0},    /* cower -> fluster */
    {105, 146, 0, 0},   /* stroke a face -> hug */
    {108, 38, 0, 0},    /* sing 1 -> talk 1 */
    {109, 38, 0, 0},    /* sing 3 -> talk 1 */
    {111, 38, 0, 0},    /* sing 2 -> talk 1 */
    {112, 44, 0, 0},    /* cry -> look down */
    {115, 57, 0, 0},    /* suffer in a bath 1 -> bath 1 */
    {116, 57, 0, 0},    /* suffer in a bath 2 -> bath 1 */
    {117, 40, 0, 0},    /* rejoice 2 -> nod */
    {125, 40, 0, 0},    /* rejoice -> nod */
    {129, 120, 50, 0},  /* lament, ready -> gesture holding a weapon, big gesture */
    {131, 130, 0, 0},   /* coronation 2 -> coronation 1 */
    {139, 97, 0, 0},    /* look down, ready -> ready */
    {145, 59, 0, 0},    /* lie down -> sleep */
    {147, 146, 0, 0},   /* hug tightly -> hug */
    {150, 146, 0, 0},   /* stroke a face 2 -> hug */
    {153, 59, 0, 0},    /* sit up and fall back -> sleep */
    {158, 59, 0, 0},    /* lie dead -> sleep */
    {159, 59, 0, 0},    /* lie dead, waiting -> sleep */
    {160, 59, 0, 0},    /* lie dead, waiting 2 -> sleep */
    {161, 97, 0, 0},    /* threatened, waiting -> ready */
    {163, 154, 0, 0},   /* suicide 1 -> kneel, waiting */
    {164, 154, 0, 0},   /* suicide 2 -> kneel, waiting */
    {165, 154, 0, 0},   /* suicide 3 -> kneel, waiting */
    {166, 136, 0, 0},   /* suicide 4 -> die 1 */
    {167, 154, 0, 0},   /* kneel and sink -> kneel, waiting */
    {168, 47, 0, 0},    /* listen 1 -> look around */
    {169, 47, 0, 0},    /* listen 2 -> look around */
    {178, 126, 0, 0},   /* hold hands 2 -> hold hands */
    {179, 126, 0, 0},   /* hold hands 3 -> hold hands */
    {184, 7, 80, 0},    /* attack 1 -> battle Attack1, thrust a weapon */
    {185, 8, 7, 80},    /* attack 2 -> battle Attack2, battle Attack1, thrust a weapon */
    {194, 154, 0, 0},   /* crouch -> kneel, waiting */
    {196, 126, 0, 0},   /* hold hands 6 -> hold hands */
    {197, 40, 0, 0},    /* bow -> nod */
    {209, 97, 0, 0},    /* capture 1 -> ready */
    {210, 97, 0, 0},    /* capture 2 -> ready */
    {211, 96, 0, 0},    /* captured -> stagger */
    {221, 40, 0, 0},    /* nod (ending) -> nod */
    {222, 38, 0, 0},    /* talk (ending) -> talk 1 */
    {225, 136, 0, 0},   /* seppuku -> die 1 */
    {226, 64, 0, 0},    /* blown away (event) -> blown away */
    {227, 7, 80, 0},    /* attack (event) -> battle Attack1, thrust a weapon */
    {228, 97, 0, 0},    /* threaten -> ready */
    {229, 207, 97, 0},  /* inspect a sword -> draw a weapon 1, ready */
    {234, 198, 0, 0},   /* Lilith dives 1 -> dive */
    {235, 198, 0, 0},   /* Lilith dives 2 -> dive */
    {239, 237, 0, 0},   /* magic while falling -> damaged fall 1 */
    {240, 60, 0, 0},    /* talk seated 3 -> talk seated 1 */
    {241, 97, 0, 0},    /* idle holding a weapon -> ready */
    {242, 121, 38, 0},  /* talk holding a weapon 1 -> talk holding a weapon, talk 1 */
    {243, 121, 38, 0},  /* talk holding a weapon 2 -> talk holding a weapon, talk 1 */
};

static int rando_clip_has(Context *ctx, uint32_t holder, uint32_t clip) {
    return mem_read32(ctx, holder + clip * 8 + HOLDER_CLIP_FILE) != 0;
}

/* the clip to play for an event clip the model has no file for, or -1 */
static int rando_clip_stand_in(Context *ctx, uint32_t holder, uint32_t clip) {
    if (clip < 0x20 || clip > 0xFF || !holder || rando_clip_has(ctx, holder, clip))
        return -1;
    for (unsigned i = 0; i < sizeof rando_clip_fallback / sizeof rando_clip_fallback[0]; i++) {
        if (rando_clip_fallback[i][0] != clip)
            continue;
        for (unsigned j = 1; j < 4 && rando_clip_fallback[i][j]; j++)
            if (rando_clip_has(ctx, holder, rando_clip_fallback[i][j]))
                return rando_clip_fallback[i][j];
        break;
    }
    return rando_clip_has(ctx, holder, CLIP_STAND) ? (int)CLIP_STAND : -1;
}

/* BattleUnitResHolder::GetSimilarAnim(AnimClip::Type) const: r0 holder,
   r1 clip; answers the clip to play */
RECOMP_OVERRIDE(0x0050D700) {
    if (haiku_rando_active) {
        int clip = rando_clip_stand_in(ctx, ctx->r[0], ctx->r[1]);
        if (clip >= 0) {
            if (haiku_rando_hook)
                haiku_rando_hook(ctx, RZ_CLIP, ctx->r[1], (uint32_t)clip);
            ctx->r[0] = (uint32_t)clip;
            ctx->budget -= 24;
            RETURN_TO(ctx->r[14]);
        }
    }
    CALL(RECOMP_ORIGINAL(0x0050D700));
}

#define UNIT_RES_HOLDER 0x7Cu /* BattleUnit: its BattleUnitResHolder* */

/* BattleUnit::PlayAnimForceDirectEndPose(AnimClip::Type): r0 unit, r1 clip */
RECOMP_OVERRIDE(0x00170388) {
    if (haiku_rando_active && ctx->r[0]) {
        int clip = rando_clip_stand_in(ctx, mem_read32(ctx, ctx->r[0] + UNIT_RES_HOLDER), ctx->r[1]);
        if (clip >= 0) {
            if (haiku_rando_hook)
                haiku_rando_hook(ctx, RZ_CLIP, ctx->r[1], (uint32_t)clip);
            ctx->r[1] = (uint32_t)clip;
        }
    }
    CALL(RECOMP_ORIGINAL(0x00170388));
}

/* rando3: UnitIcon::SetIcon(const char* job, const char* chara, UnitColor):
   every map sprite, named by its class (JID without JID_) and character
   (AID without AID_). A character whose head sheet has no head for the
   class's body (Kotaro or Hans as a Paladin, Garon in anything) is drawn
   with the class's own generic head instead of none; the host decides
   (randomizer::sprite, from the game's own sheets). Only while a run is
   randomized. */
RECOMP_OVERRIDE(0x004F53B4) {
    if (haiku_rando_hook && haiku_rando_active && ctx->r[1] && ctx->r[2]) {
        uint32_t chara = haiku_rando_hook(ctx, RZ_SPRITE, ctx->r[1], ctx->r[2]);
        if (chara)
            ctx->r[2] = chara;
    }
    CALL(RECOMP_ORIGINAL(0x004F53B4));
}

/* rando3: the save list's mark for a randomized save.
   ChapterMenuItem::OnDraw (0x001D95E4) draws a save slot each frame, after
   its header was read (ReadHeader, then SetupParam, fill the slot's texts
   and badges: item + 0x44 is set once a slot has a save). Before it draws,
   the host is asked about the slot; when its save file is randomized, the
   host answers the mode badge's new text (the mode, a dot, "Randomizer",
   once per slot: it answers 0 when the badge already says so, and keeps
   what it read of each save file), which goes where the game keeps it
   (item + 0x1F8, 0x20 characters). Ordinary saves are left as they are. */
#define ITEM_MODE_TEXT 0x1F8u
#define ITEM_HAS_DATA  0x44u

/* ChapterMenuItem::OnDraw(void) */
RECOMP_OVERRIDE(0x001D95E4) {
    uint32_t item = ctx->r[0];
    if (haiku_rando_hook && item && mem_read8(ctx, item + ITEM_HAS_DATA)) {
        uint32_t text = haiku_rando_hook(ctx, RZ_BADGE, item, 0);
        if (text) {
            uint32_t i = 0;
            for (; i < 0x1F; i++) {
                uint16_t u = mem_read16(ctx, text + i * 2);
                if (!u)
                    break;
                mem_write16(ctx, item + ITEM_MODE_TEXT + i * 2, u);
            }
            mem_write16(ctx, item + ITEM_MODE_TEXT + i * 2, 0);
        }
    }
    CALL(RECOMP_ORIGINAL(0x001D95E4));
}

/* rando3: Unit::Create(const Person*, const Job*, int, Random*): a unit is
   made (r1 the person, r2 the class). The avatar creator makes Corrin's
   unit with Corrin's own class (Nohr Prince or Princess) named outright, not
   the person's; when Corrin's class is randomized the host answers the
   class to use instead (0: as asked). Only while a run is randomized. */
RECOMP_OVERRIDE(0x003D7AB0) {
    if (haiku_rando_hook && haiku_rando_active && ctx->r[1] && ctx->r[2]) {
        uint32_t job = haiku_rando_hook(ctx, RZ_UNIT_JOB, ctx->r[1], ctx->r[2]);
        if (job)
            ctx->r[2] = job;
    }
    CALL(RECOMP_ORIGINAL(0x003D7AB0));
}

/* rando3: Unit::SetDisposItem(const void* dispos): a chapter gives a unit
   that already exists its items (Corrin's, whose unit the avatar creator
   made). With Corrin's class randomized, a weapon the class can't use
   becomes one it can, as for placed units. Only while a run is
   randomized. */
RECOMP_OVERRIDE(0x003D8C38) {
    if (haiku_rando_hook && haiku_rando_active && ctx->r[1])
        haiku_rando_hook(ctx, RZ_DISPOS_ITEMS, ctx->r[1], 0);
    CALL(RECOMP_ORIGINAL(0x003D8C38));
}

/* -- corrinswap: Corrin as a character -------------------------------------

   With the CORRIN option the player's Corrin appears in another place S,
   and the one S would have shown (Y) appears in Corrin's. The game draws
   the avatar from its unit's edit (Unit + 0xB0) and its person
   PID_プレイヤー男/女, everyone else from the person's FID and AID. Each
   function below that draws a character from a unit is handed a copy of
   the unit made by the host (S's unit with the avatar's person, edit and
   hair colour; the avatar's with Y's person, no edit and Y's hair colour),
   so the units themselves never change. Only while a randomized run swaps
   Corrin (RZ_CORRIN_ON); otherwise each runs the original alone. */

/* the unit argument in r<reg> is replaced by the host's copy, if any */
static void rando_unit_arg(Context *ctx, int reg, uint32_t why) {
    if (!RANDO_CORRIN() || !ctx->r[reg])
        return;
    uint32_t copy = haiku_rando_hook(ctx, RZ_UNIT_LOOK, ctx->r[reg], why);
    if (copy)
        ctx->r[reg] = copy;
}

/* Unit::GetName(void) const: the name in menus, map windows, battles */
RECOMP_OVERRIDE(0x005331C8) {
    rando_unit_arg(ctx, 0, 0);
    CALL(RECOMP_ORIGINAL(0x005331C8));
}

/* FaceInstance::LoadByUnit(const Unit*, FaceData::Type, bool): a portrait */
RECOMP_OVERRIDE(0x001AE730) {
    rando_unit_arg(ctx, 1, 1);
    CALL(RECOMP_ORIGINAL(0x001AE730));
}

/* UnitIcon::SetIcon(const Unit*): a map sprite */
RECOMP_OVERRIDE(0x004F5264) {
    rando_unit_arg(ctx, 1, 1);
    CALL(RECOMP_ORIGINAL(0x004F5264));
}

/* UnitIcon::SetIcon(const Unit*, Force::Type) */
RECOMP_OVERRIDE(0x004F52E0) {
    rando_unit_arg(ctx, 1, 1);
    CALL(RECOMP_ORIGINAL(0x004F52E0));
}

/* UnitIcon::SetIcon(const Job*, const Person*, Force::Type, const unit::Edit*,
   bool): a map sprite from a person and an edit (menus, support lists,
   castle). From the two SetIcon(const Unit*) above (returning to 0x004F52D0
   and 0x004F534C) the arguments are the copy's already. */
RECOMP_OVERRIDE(0x004F5104) {
    if (RANDO_CORRIN() && ctx->r[14] != 0x004F52D0u && ctx->r[14] != 0x004F534Cu)
        haiku_rando_hook(ctx, RZ_ICON_LOOK, 0, 0);
    CALL(RECOMP_ORIGINAL(0x004F5104));
}

/* FaceCutInTexFile::Create(const Unit*): the critical cut-in's portrait */
RECOMP_OVERRIDE(0x001EA1E4) {
    rando_unit_arg(ctx, 0, 1);
    CALL(RECOMP_ORIGINAL(0x001EA1E4));
}

/* FaceCutIn::HasCutIn(const Unit*) */
RECOMP_OVERRIDE(0x004FD6CC) {
    rando_unit_arg(ctx, 0, 1);
    CALL(RECOMP_ORIGINAL(0x004FD6CC));
}

#define LOAD_BY_FID 0x001B18E8u /* FaceInstance::LoadByFid(const char*, FaceData::Type, bool, bool, bool) */

/* FaceInstance::LoadByUnitEdit(const unit::Edit*, FaceData::Type, bool, bool,
   bool): a portrait from the avatar's edit (castle cards, support lists);
   Corrin's shown as Y loads Y's FID (LoadByFid takes the same arguments) */
RECOMP_OVERRIDE(0x001AF6F4) {
    if (RANDO_CORRIN() && ctx->r[1]) {
        uint32_t fid = haiku_rando_hook(ctx, RZ_FACE_EDIT, ctx->r[1], 0);
        if (fid) {
            ctx->r[1] = fid;
            ctx->r[15] = LOAD_BY_FID;
            ctx->thumb = 0;
            ctx->budget -= 20;
            CALL(recomp_call);
            return;
        }
    }
    CALL(RECOMP_ORIGINAL(0x001AF6F4));
}

/* FaceData::GetMessName(void) const: MPID_username's name is the avatar's
   (Unit::GetName of the player, which would be Y's above) */
RECOMP_OVERRIDE(0x00545BD8) {
    if (RANDO_CORRIN() && ctx->r[0]) {
        uint32_t name = haiku_rando_hook(ctx, RZ_MESS_NAME, ctx->r[0], 0);
        if (name) {
            ctx->r[0] = name;
            ctx->budget -= 20;
            RETURN_TO(ctx->r[14]);
        }
    }
    CALL(RECOMP_ORIGINAL(0x00545BD8));
}

/* Person::GetName(void) const: S's person says the player's name (the
   host answers it in r0), Corrin's person asks Y's stand-in (r0) */
RECOMP_OVERRIDE(0x0053ED88) {
    if (RANDO_CORRIN() && ctx->r[0]) {
        uint32_t how = haiku_rando_hook(ctx, RZ_PERSON_NAME, 0, 0);
        if (how == 1) {
            ctx->budget -= 20;
            RETURN_TO(ctx->r[14]);
        }
    }
    CALL(RECOMP_ORIGINAL(0x0053ED88));
}

/* A talk window whose FID is FID_username (TalkWindow::FaceLoadByUnit,
   0x00507A54) loads the face and name of TalkUtil::GetPlayerUnit()'s unit.
   With Corrin swapped, Corrin's own windows were given Y's FID already
   (GetFidForWindow above), so FID_username here is the window of the place
   shown as Corrin: it gets the avatar's unit as it is (the host's alias,
   a copy the swaps leave alone). The flag is set at each FaceLoadByUnit
   and read only by calls from inside it. */
static uint32_t rando_talk_alias = 0;
#define FACE_LOAD_BY_UNIT     0x00507A54u
#define FACE_LOAD_BY_UNIT_END 0x00507C34u

/* TalkWindow::FaceLoadByUnit(FaceInstance*, const char* fid, TalkNamePlate*,
   const char*) const */
RECOMP_OVERRIDE(0x00507A54) {
    rando_talk_alias = 0;
    if (RANDO_CORRIN() && ctx->r[2]) {
        static const char user[] = "FID_username";
        uint32_t i = 0;
        while (i < sizeof user - 1 && mem_read8(ctx, ctx->r[2] + i) == (uint8_t)user[i])
            i++;
        rando_talk_alias = i == sizeof user - 1;
    }
    CALL(RECOMP_ORIGINAL(0x00507A54));
}

/* TalkUtil::GetPlayerUnit(void) */
RECOMP_OVERRIDE(0x004F2330) {
    if (rando_talk_alias && RANDO_CORRIN() && ctx->r[14] >= FACE_LOAD_BY_UNIT && ctx->r[14] < FACE_LOAD_BY_UNIT_END) {
        uint32_t unit = haiku_rando_hook(ctx, RZ_ALIAS, 0, 0);
        if (unit) {
            ctx->r[0] = unit;
            ctx->budget -= 20;
            RETURN_TO(ctx->r[14]);
        }
    }
    CALL(RECOMP_ORIGINAL(0x004F2330));
}

/* title::ProcMyUnitEdit::Create(ProcInst*, CALL_MODE) and
   ~ProcMyUnitEdit(void): while the avatar's creator is open its preview
   is the avatar as edited, never swapped */
RECOMP_OVERRIDE(0x0043AA70) {
    if (haiku_rando_hook)
        haiku_rando_hook(ctx, RZ_CREATOR, 1, 0);
    CALL(RECOMP_ORIGINAL(0x0043AA70));
}

RECOMP_OVERRIDE(0x0043C068) {
    if (haiku_rando_hook)
        haiku_rando_hook(ctx, RZ_CREATOR, 0, 0);
    CALL(RECOMP_ORIGINAL(0x0043C068));
}

/* -- the New Game screens ------------------------------------------------- */

enum {
    RZ_MENU_FOOTER = 0x40,
    RZ_MENU_OPEN = 0x41,
    RZ_MENU_TEXT = 0x42,
    RZ_MENU_CHOOSE = 0x43,
    RZ_MENU_CANCEL = 0x44,
    RZ_MENU_STEP = 0x45,
    RZ_MENU_SHOWN = 0x46,
    RZ_MENU_POINTER = 0x47,
    RZ_MENU_CORRIN_STEP = 0x48, /* corrinswap */
    RZ_MENU_CORRIN = 0x49,
};

/* the mode screen is up with this many panels (0: it closed); the host
   forgets any click it held */
static void rando_menu_shown(Context *ctx, uint32_t panels) {
    if (rando_menu_up == panels)
        return;
    rando_menu_up = panels;
    if (haiku_rando_hook)
        haiku_rando_hook(ctx, RZ_MENU_SHOWN, panels, 0);
}

#define TITLE_PROC       0x006FEC18u /* title::ProcTitle* */
#define MODE_CURSOR      0x006D93ECu /* the mode screen's remembered cursor */
#define FOOTER_RETURN    0x004423C8u /* after bl Layout::SetItem in CreateModeSelectMenu */
#define PHOENIX_VTABLE   0x0069F3FCu
#define CASUAL_VTABLE    0x0069F1FCu
#define CLASSIC_VTABLE   0x0069F370u
#define ITEM_MENU        0x0Cu       /* the item's menu */
#define ITEM_INDEX       0x2Cu       /* the item's index */
#define MENU_FLAGS       0x50u
#define PROC_DIFFICULTY  0x73u
#define PROC_MODE        0x74u
#define LABEL_MODE_MENU  4u
#define LABEL_START      8u
#define DECIDE_AND_CLOSE 0x101u      /* what a mode's ACall answers */
#define CANCEL_AND_CLOSE 0x401u      /* what SelectModeBaseItem::BCall answers */

/* ProcInst::Jump(proc, label), done here: the proc goes on at the label */
static void rando_proc_jump(Context *ctx, uint32_t proc, uint32_t label) {
    uint32_t cmd = mem_read32(ctx, proc + 4);
    for (int i = 0; i < 1024; i++, cmd += 0x14) {
        uint8_t type = mem_read8(ctx, cmd);
        if (type == 0)
            return;
        if (type == 4 && mem_read32(ctx, cmd + 4) == label) {
            mem_write32(ctx, proc + 8, cmd);
            return;
        }
    }
}

static int rando_panel_of(Context *ctx, uint32_t item) {
    uint32_t vtable = item ? mem_read32(ctx, item) : 0;
    if (vtable == PHOENIX_VTABLE)
        return 0;
    if (vtable == CASUAL_VTABLE)
        return 1;
    if (vtable == CLASSIC_VTABLE)
        return 2;
    return -1;
}

/* FrameManager::Layout::SetItem(const wchar_t*), called by
   CreateModeSelectMenu (returning to 0x004423C8) with the footer it got
   from Mess::Get: the footer becomes the randomizer's line before the
   game copies it into its text. */
RECOMP_OVERRIDE(0x001B2B24) {
    if (haiku_rando_hook && rando_text_phase != 0 && ctx->r[14] == FOOTER_RETURN) {
        uint32_t text = haiku_rando_hook(ctx, RZ_MENU_FOOTER, rando_text_phase, 0);
        if (text)
            ctx->r[1] = text;
    }
    CALL(RECOMP_ORIGINAL(0x001B2B24));
}

static void rando_menu_built(Context *ctx, const uint32_t *words) {
    /* the difficulty back, after a phase-1 or phase-2 build */
    uint32_t proc = mem_read32(ctx, TITLE_PROC);
    if (proc)
        mem_write8(ctx, proc + PROC_DIFFICULTY, (uint8_t)words[0]);
}

/* title::ProcTitle::CreateModeSelectMenu(void) */
RECOMP_OVERRIDE(0x0044208C) {
    rando_text_phase = rando_phase;
    if (rando_phase == 0) {
        /* Phoenix only on Normal difficulty */
        rando_menu_shown(ctx, mem_read8(ctx, ctx->r[0] + PROC_DIFFICULTY) == 0 ? 3 : 2);
        CALL(RECOMP_ORIGINAL(0x0044208C));
        return;
    }
    rando_menu_shown(ctx, rando_phase >= 2 ? 2 : 3);
    /* phase 1: Normal difficulty's three panels; phases 2 and 3: Hard's two */
    mem_write8(ctx, ctx->r[0] + PROC_DIFFICULTY, rando_phase >= 2 ? 1 : 0);
    CALL_THEN(RECOMP_ORIGINAL(0x0044208C), rando_menu_built, rando_difficulty);
}

/* a panel's name (field 0) or line (field 1) on the randomizer screen
   (phase 1) or the Normal / Randomizer step (phase 2) */
static int rando_text(Context *ctx, uint32_t panel, uint32_t field) {
    uint32_t text = 0;
    if (rando_text_phase != 0 && haiku_rando_hook)
        text = haiku_rando_hook(ctx, RZ_MENU_TEXT, panel | field << 8 | rando_text_phase << 16, 0);
    if (!text)
        return 0;
    ctx->r[0] = text;
    ctx->budget -= 40;
    return 1;
}

/* the recompiler finds overrides by their RECOMP_OVERRIDE marks in the
   text, so each is written out */
/* title::SelectModePhoenixItem::GetGameModeName(void) */
RECOMP_OVERRIDE(0x0043E5D0) {
    if (rando_text(ctx, 0, 0)) {
        RETURN_TO(ctx->r[14]);
    }
    CALL(RECOMP_ORIGINAL(0x0043E5D0));
}

/* title::SelectModePhoenixItem::GetGameModeHName(void) */
RECOMP_OVERRIDE(0x0043E640) {
    if (rando_text(ctx, 0, 1)) {
        RETURN_TO(ctx->r[14]);
    }
    CALL(RECOMP_ORIGINAL(0x0043E640));
}

/* title::SelectModeCasualItem::GetGameModeName(void) */
RECOMP_OVERRIDE(0x0043E168) {
    if (rando_text(ctx, 1, 0)) {
        RETURN_TO(ctx->r[14]);
    }
    CALL(RECOMP_ORIGINAL(0x0043E168));
}

/* title::SelectModeCasualItem::GetGameModeHName(void) */
RECOMP_OVERRIDE(0x0043E1D8) {
    if (rando_text(ctx, 1, 1)) {
        RETURN_TO(ctx->r[14]);
    }
    CALL(RECOMP_ORIGINAL(0x0043E1D8));
}

/* title::SelectModeClassicItem::GetGameModeName(void) */
RECOMP_OVERRIDE(0x0043E4BC) {
    if (rando_text(ctx, 2, 0)) {
        RETURN_TO(ctx->r[14]);
    }
    CALL(RECOMP_ORIGINAL(0x0043E4BC));
}

/* title::SelectModeClassicItem::GetGameModeHName(void) */
RECOMP_OVERRIDE(0x0043E52C) {
    if (rando_text(ctx, 2, 1)) {
        RETURN_TO(ctx->r[14]);
    }
    CALL(RECOMP_ORIGINAL(0x0043E52C));
}

/* the mode screen built again (label 4) for another phase, its cursor on
   `cursor`; the menu that was open closes as after a mode's ACall */
static void rando_reopen(Context *ctx, uint32_t proc, uint32_t item, uint32_t phase, uint32_t cursor) {
    rando_phase = phase;
    mem_write32(ctx, MODE_CURSOR, cursor);
    rando_proc_jump(ctx, proc, LABEL_MODE_MENU);
    uint32_t menu = mem_read32(ctx, item + ITEM_MENU);
    mem_write32(ctx, menu + MENU_FLAGS, mem_read32(ctx, menu + MENU_FLAGS) & ~4u);
}

/* the new game goes on (label 8) in the mode chosen in phase 0, as that
   mode's ACall would have gone on */
static void rando_start(Context *ctx, uint32_t proc, uint32_t item) {
    rando_phase = 0;
    mem_write8(ctx, proc + PROC_DIFFICULTY, (uint8_t)rando_difficulty);
    mem_write8(ctx, proc + PROC_MODE, (uint8_t)rando_mode);
    mem_write32(ctx, MODE_CURSOR, (uint32_t)rando_cursor);
    rando_proc_jump(ctx, proc, LABEL_START);
    uint32_t menu = mem_read32(ctx, item + ITEM_MENU);
    mem_write32(ctx, menu + MENU_FLAGS, mem_read32(ctx, menu + MENU_FLAGS) & ~4u);
}

/* A on a panel:
   - phase 0 (a mode): the mode is kept and the Normal / Randomizer step
     opens, its cursor on Normal;
   - phase 2: Normal (Casual's panel) goes on with the mode, no
     randomizer; Randomizer (Classic's panel) opens the randomizer screen,
     its cursor on its first panel;
   - phase 1: the run goes on with that randomizer. */
static int rando_choose(Context *ctx) {
    uint32_t item = ctx->r[0];
    rando_menu_shown(ctx, 0);
    int panel = rando_panel_of(ctx, item);
    uint32_t proc = mem_read32(ctx, TITLE_PROC);
    if (panel < 0 || !proc || !haiku_rando_hook)
        return 0;
    if (rando_phase == 0) {
        if (!haiku_rando_hook(ctx, RZ_MENU_STEP, 0, 0))
            return 0;
        rando_mode = (uint32_t)panel; /* Phoenix 0, Casual 1, Classic 2, as ProcTitle + 0x74 */
        rando_cursor = mem_read32(ctx, item + ITEM_INDEX);
        rando_difficulty = mem_read8(ctx, proc + PROC_DIFFICULTY);
        rando_reopen(ctx, proc, item, 2, 0);
    } else if (rando_phase == 2 && panel == 1) {
        haiku_rando_hook(ctx, RZ_MENU_CANCEL, 0, 0);
        rando_start(ctx, proc, item);
    } else if (rando_phase == 2) {
        haiku_rando_hook(ctx, RZ_MENU_OPEN, 0, 0);
        rando_reopen(ctx, proc, item, 1, 0);
    } else if (rando_phase == 1) {
        haiku_rando_hook(ctx, RZ_MENU_CHOOSE, (uint32_t)panel, 0);
        /* corrinswap: the Corrin step, its cursor on Keep Corrin */
        if (haiku_rando_hook(ctx, RZ_MENU_CORRIN_STEP, 0, 0)) {
            rando_rpanel = (uint32_t)panel;
            rando_reopen(ctx, proc, item, 3, 0);
        } else {
            rando_start(ctx, proc, item);
        }
    } else {
        /* corrinswap: Keep Corrin (Casual's panel) or Swap Corrin (Classic's) */
        haiku_rando_hook(ctx, RZ_MENU_CORRIN, panel == 2, 0);
        rando_start(ctx, proc, item);
    }
    ctx->r[0] = DECIDE_AND_CLOSE;
    ctx->budget -= 12;
    return 1;
}

/* title::SelectModeClassicItem::ACall(void) */
RECOMP_OVERRIDE(0x0043E55C) {
    if (rando_choose(ctx)) {
        RETURN_TO(ctx->r[14]);
    }
    CALL(RECOMP_ORIGINAL(0x0043E55C));
}

/* title::SelectModeCasualItem::ACall(void) */
RECOMP_OVERRIDE(0x0043E208) {
    if (rando_choose(ctx)) {
        RETURN_TO(ctx->r[14]);
    }
    CALL(RECOMP_ORIGINAL(0x0043E208));
}

/* title::SelectModePhoenixItem::ACall(void) */
RECOMP_OVERRIDE(0x0043E674) {
    if (rando_choose(ctx)) {
        RETURN_TO(ctx->r[14]);
    }
    CALL(RECOMP_ORIGINAL(0x0043E674));
}

/* title::SelectModeBaseItem::BCall(void): B on the randomizer screen goes
   back to the Normal / Randomizer step (cursor on Randomizer), and B there
   back to the mode screen (cursor on the mode chosen); on the mode screen
   B is the game's own (back to the difficulty) */
RECOMP_OVERRIDE(0x0043DC28) {
    uint32_t proc = mem_read32(ctx, TITLE_PROC);
    rando_menu_shown(ctx, 0);
    if (rando_phase == 0 || !proc) {
        CALL(RECOMP_ORIGINAL(0x0043DC28));
        return;
    }
    if (haiku_rando_hook)
        haiku_rando_hook(ctx, RZ_MENU_CANCEL, 0, 0);
    mem_write8(ctx, proc + PROC_DIFFICULTY, (uint8_t)rando_difficulty);
    if (rando_phase == 3) {
        /* corrinswap: back to the randomizer screen, on the panel chosen */
        rando_phase = 1;
        mem_write32(ctx, MODE_CURSOR, rando_rpanel);
    } else if (rando_phase == 1) {
        rando_phase = 2;
        mem_write32(ctx, MODE_CURSOR, 1);
    } else {
        rando_phase = 0;
        mem_write32(ctx, MODE_CURSOR, (uint32_t)rando_cursor);
    }
    rando_proc_jump(ctx, proc, LABEL_MODE_MENU);
    ctx->r[0] = CANCEL_AND_CLOSE;
    ctx->budget -= 8;
    RETURN_TO(ctx->r[14]);
}

/* -- the mouse on the mode screens ----------------------------------------

   The game's difficulty and mode screens are on the upper screen, which a
   3DS cannot touch, so the game reads only buttons there. On a PC the
   mouse (and a touchscreen's taps, which come as the mouse) clicks a panel
   of the mode screen, the Normal / Randomizer step or the randomizer
   screen, and the right button goes back. The host finds the panel under
   the pointer (zakuro's lib.rs, randomizer::menu_click); each frame the
   menu reads its input (BasicMenu::Input, 0x004F9FB4) while one waits, the
   button that brings the cursor a step closer to it is pressed for the
   menu, then A on it (B to go back), as if the player pressed them, so
   the menu moves, sounds and decides as its own. The click is forgotten
   when the menu closes (ACall, BCall) or another is built. */
#define PAD_PTR      0x006BD96Cu /* the pad the menus read: +0x14 pressed now, +0x1C pressed or repeating */
#define PAD_TRIGGER  0x14u
#define PAD_REPEAT   0x1Cu
#define MENU_ITEMS   0x34u       /* BasicMenu: its items */
#define MENU_COUNT   0x38u       /* and how many */
#define MENU_SELECT  0x9Cu       /* the item under the cursor */
#define PICK_BACK    0x80u

static void rando_pointer(Context *ctx) {
    uint32_t menu = ctx->r[0];
    uint32_t count = mem_read32(ctx, menu + MENU_COUNT);
    uint32_t items = mem_read32(ctx, menu + MENU_ITEMS);
    if (count == 0 || count > 3 || !items || rando_panel_of(ctx, mem_read32(ctx, items)) < 0)
        return;
    uint32_t pick = haiku_rando_hook(ctx, RZ_MENU_POINTER, count, 0);
    uint32_t pad = pick ? mem_read32(ctx, PAD_PTR) : 0;
    if (!pad)
        return;
    uint32_t press;
    if (pick == PICK_BACK) {
        press = 0x2; /* B */
    } else {
        uint32_t want = pick - 1, now = mem_read32(ctx, menu + MENU_SELECT);
        if (want >= count)
            return;
        if (now < want)
            press = 0x80 | 0x8000000u; /* down, as the D-pad's bit and the menu's own */
        else if (now > want)
            press = 0x40 | 0x4000000u; /* up */
        else
            press = 0x1; /* A */
    }
    mem_write32(ctx, pad + PAD_TRIGGER, mem_read32(ctx, pad + PAD_TRIGGER) | press);
    mem_write32(ctx, pad + PAD_REPEAT, mem_read32(ctx, pad + PAD_REPEAT) | press);
}

/* BasicMenu::Input(void): every menu, each frame; only the mode screen's
   while it is up and a click waits */
RECOMP_OVERRIDE(0x004F9FB4) {
    if (rando_menu_up && haiku_rando_hook)
        rando_pointer(ctx);
    CALL(RECOMP_ORIGINAL(0x004F9FB4));
}

/* -- rando4: children's hair colours ---------------------------------------

   A child's hair takes the colour of the parent the fixed parent married.
   FaceDataManager::GetHairColorFromPerson(Color8* out, const Person*)
   (0x001E0180) works it out (the fixed parent's unit, its spouse's unit and
   the spouse's hair colour, Unit + 0x13C; else the child's own FaceData
   colour). Unit::Create and the castle's units keep the answer in the
   unit's +0x13C, which the save keeps and every portrait, map sprite,
   cut-in and model reads. With the HAIR option (zakuro-core/src/randomizer/
   hair.rs) the host answers a child's own colour, drawn from the seed out
   of the colours the game's parents give children. Only while such a run
   is on (RZ_HAIR_ON); otherwise each runs the original alone. */
#define RZ_HAIR_ON 0x200u
#define RANDO_HAIR() (haiku_rando_hook && (haiku_rando_active & RZ_HAIR_ON))

/* FaceDataManager::GetHairColorFromPerson(Color8* out, const Person*) */
RECOMP_OVERRIDE(0x001E0180) {
    if (RANDO_HAIR() && ctx->r[0] && ctx->r[1]) {
        uint32_t colour = haiku_rando_hook(ctx, RZ_HAIR, ctx->r[1], 0);
        if (colour) {
            mem_write32(ctx, ctx->r[0], colour);
            ctx->budget -= 20;
            RETURN_TO(ctx->r[14]);
        }
    }
    CALL(RECOMP_ORIGINAL(0x001E0180));
}

/* the model's hair colour (BattleUnitResName + 0x724, 0xRRGGBB), which
   PostProcess sets from the unit only for a child's person or the avatar:
   a child shown in an adult's place, or cast with no unit, gets theirs */
static void rando_hair_model(Context *ctx, const uint32_t *words) {
    if (!RANDO_HAIR() || !words[0])
        return;
    {
        uint32_t colour = haiku_rando_hook(ctx, RZ_HAIR_MODEL, words[1], words[2]);
        if (colour)
            mem_write32(ctx, words[0] + 0x724u, colour & 0xFFFFFFu);
    }
}

/* BattleUnitResName::PostProcess(const Unit*, const Person*) */
RECOMP_OVERRIDE(0x001F8A84) {
    if (!RANDO_HAIR()) {
        CALL(RECOMP_ORIGINAL(0x001F8A84));
        return;
    }
    CALL_THEN(RECOMP_ORIGINAL(0x001F8A84), rando_hair_model, ctx->r[0], ctx->r[1], ctx->r[2]);
}

/* pulsefix: what of this file is the console's state, kept with Divine
   Pulse's moments and saved states (overrides/keep.c) */
void randomizer_keep(void (*add)(const char *, void *, uint32_t)) {
    add("randomizer.rando_phase", &rando_phase, sizeof rando_phase);
    add("randomizer.rando_rpanel", &rando_rpanel, sizeof rando_rpanel);
    add("randomizer.rando_mode", &rando_mode, sizeof rando_mode);
    add("randomizer.rando_difficulty", &rando_difficulty, sizeof rando_difficulty);
    add("randomizer.rando_cursor", &rando_cursor, sizeof rando_cursor);
    add("randomizer.rando_menu_up", &rando_menu_up, sizeof rando_menu_up);
    add("randomizer.rando_text_phase", &rando_text_phase, sizeof rando_text_phase);
    add("randomizer.rando_talk_alias", &rando_talk_alias, sizeof rando_talk_alias);
}
