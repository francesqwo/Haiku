/* amiibo: the four amiibo Hero Battles in the Dragon's Gate's Play DLC list.
 *
 * On a 3DS, scanning a Marth, Ike, Lucina or Robin amiibo in My Castle's
 * amiibo Gazebo calls that Hero to the castle. Scripts/amiibo/AMB_<hero>.cmb
 * holds the talks: the first scan gives one of two accessories, the second
 * is a question, the third runs ev::ChapterAppear("CID_M00n"), which sets
 * the chapter's Spot state to 1 so the Hero Battle shows on the world map
 * (castle::SpotSelectSeq lists type-7 chapters with action 1). Choosing it
 * there (MI_NextChapterYes::ACall) sets GameUserData's next chapter (+0xC)
 * and castle::MainSeq::SetResult(5, -1), and the castle hands over to the
 * chapter. Scripts/M00n.cmb ends with ev::JoinGuest("PID_<hero>"), so the
 * Hero joins the army when the map is won (event::JoinUniqueGuest), and
 * Spot::UpdateSpot marks the spot cleared (state 2, the turn count at +0xA).
 * A cleared spot has no action (spot::Data::GetAction), so the battle is
 * gone from the world map for good: later scans only talk.
 *
 * Here the same battles are listed after the DLC maps, built as the game
 * builds a DLC entry (link::ec::play Menu::MenuItem), with the game's own
 * chapter name (Chapter::GetTitleName: "Hero-King Marth", ...) and a
 * DLC-style icon from the Hero's own portrait (amiibo_icons.c, made by
 * dev/amiibo/make_icons.py), turned into a texture by the game's own
 * link::ec::EcUtil::EcIconToITexture. Choosing one asks "Load this
 * content?" as every entry does; Yes starts the chapter exactly as the world
 * map does (next chapter, MainSeq result 5), so the map, its talks, its
 * rewards and the Hero joining are the game's own. A cleared Hero Battle
 * stays in the list greyed out, and choosing it only gives the menu's miss
 * sound: the game allows no replay.
 *
 * - BasicMenu::Create (0x004FA880), overridden in mods.c, calls
 *   amiibo_menu_create first. For the list (called from
 *   link::ec::play::ContentsMenu::CreateBind, returning to 0x003F3BB0) it
 *   appends the entries before the game's own Create runs. The guest calls
 *   it needs (operator new, ScrollText's constructor, GetTitleName,
 *   EcIconToITexture) return to 0x004FA880 and carry on from a per-thread
 *   step record, as mods.c does.
 * - The icon textures go in the ContentsReader's own icon slots (+0x220,
 *   256 of them; the DLC uses the first ~62), at 0xFC to 0xFF, so
 *   ContentsReader::Free destroys them with the DLC's.
 * - BasicMenuItem::BuildAttribute (gate.c) asks amiibo_item_attribute:
 *   2 (the game's greyed-out attribute) for a cleared Hero Battle, and
 *   Menu::MenuItem::ACall (0x003F4608) answers 0x1000 (BasicMenu::SEMiss)
 *   for it instead of asking "Load this content?".
 * - ProcSequence::ContentsMenuResult (0x003F3CF8): for a Hero Battle entry,
 *   the chapter start; any other entry runs the game's code (CID_E000 and
 *   the DLC content index).
 */
#include "overrides.h"

/* amiibo_icons.c, made by dev/amiibo/make_icons.py: one IconData per Hero */
#define AMIIBO_ICON_WORDS 2304u
extern const uint16_t amiibo_icons[4][2304];

#define LIST_CREATE_RETURN 0x003F3BB0u /* after bl BasicMenu::Create in ContentsMenu::CreateBind */
#define BASIC_MENU_CREATE  0x004FA880u
#define OPERATOR_NEW       0x002FDC68u /* operator new(unsigned int) */
#define SCROLLTEXT_CTOR    0x004E6B40u /* GameFont::ScrollText::ScrollText(void) */
#define CHAPTER_TITLE      0x005441B4u /* Chapter::GetTitleName(void) const */
#define ICON_TO_TEXTURE    0x003F48D0u /* link::ec::EcUtil::EcIconToITexture */

/* link::ec::play Menu::MenuItem, as CreateBind fills it */
#define ITEM_VTABLE   0x0062AABCu
#define ITEM_SIZE     0x4FCu
#define ITEM_NAME     0x38u  /* wchar_t[0x40] */
#define ITEM_NAME_MAX 0x40u
#define ITEM_ID       0xB8u  /* content id (u16) */
#define ITEM_ICON     0xBCu  /* ITexture* */
#define ITEM_TURNS    0xC0u  /* record, 0 = none */
#define ITEM_CLEARED  0xC4u  /* u8, then three zero bytes */
#define ITEM_SCROLL   0xC8u  /* GameFont::ScrollText */
/* jpn: the Japanese build's MenuItem is the same up to +0xC4 and ends there
   (operator new(0xC8) in its ContentsMenu::CreateBind, 0x003F4934; it has
   no ScrollText: GameFont::ScrollText isn't in that build) */
#define ITEM_SIZE_JPN 0xC8u
#define LIST_ROOM     256u   /* GameMenuItemListTemplate<256> */

/* ContentsReader */
#define READER_COUNT  0x0Cu
#define READER_ICONS  0x220u
#define ICON_SLOT     0xFCu  /* slots 0xFC..0xFF are the Heroes' */

/* globals */
#define SPOT_PTR      0x006D92F8u /* Spot*: +8 data array, +0xC count, 0x44 each */
#define USERDATA_PTR  0x006DA248u /* GameUserData*: +0xC next chapter */
#define MAINSEQ_PTR   0x006D1B70u /* castle::MainSeq*: +0x98 result, +0x9C value */
#define PLAY_SELECT   0x006D595Cu /* link::ec::play: +0 selected, +2 content id */
#define PLAY_DECIDED  0x006D5990u /* link::ec::play::Sequence::IsDecide */
#define CASTLE_PTR    0x006D1998u /* Castle::Get: +4 castle state, +0x10 castle::Amiibo::Data* by AmiiboType */
#define FORCES_PTR    0x006D9F44u /* Force::Get's forces, 12 bytes each, units linked at +0x14 */
#define AMIIBO_TALK   0x0045BAE8u /* castle::AmiiboTalkSequence::CreateBind(ProcInst*, const Unit*) */
#define AMIIBO_PIDS   0x006121E0u /* castle::Amiibo::GetPid's table: char* by AmiiboType (Shift-JIS) */

/* content ids for the Heroes: far above any DLC content (62 of them);
   a cleared battle's entry has HERO_ID_CLEARED added */
#define HERO_ID_BASE    0xA001u
#define HERO_ID_CLEARED 0x100u
#define HEROES          4u

static const char *const hero_cids[HEROES] = {"CID_M001", "CID_M002", "CID_M003", "CID_M004"};

static int is_hero_id(uint32_t id) {
    id &= ~HERO_ID_CLEARED;
    return id >= HERO_ID_BASE && id < HERO_ID_BASE + HEROES;
}

static uint32_t hero_of(uint32_t id) {
    return (id & ~HERO_ID_CLEARED) - HERO_ID_BASE;
}

static int guest_streq(Context *ctx, uint32_t at, const char *s) {
    if (!at)
        return 0;
    for (uint32_t i = 0;; i++) {
        uint8_t c = mem_read8(ctx, at + i);
        if (c != (uint8_t)s[i])
            return 0;
        if (!c)
            return 1;
    }
}

/* the Spot entry of hero k's chapter on this route, or 0 */
static uint32_t hero_spot(Context *ctx, uint32_t k) {
    uint32_t spot = mem_read32(ctx, SPOT_PTR);
    if (!spot)
        return 0;
    uint32_t data = mem_read32(ctx, spot + 8), count = mem_read32(ctx, spot + 0xC);
    if (!data || count > 0x100)
        return 0;
    for (uint32_t i = 0; i < count; i++) {
        uint32_t entry = data + i * 0x44;
        uint32_t chapter = mem_read32(ctx, entry + 4);
        if (chapter && guest_streq(ctx, mem_read32(ctx, chapter), hero_cids[k]))
            return entry;
    }
    return 0;
}

static int hero_cleared(Context *ctx, uint32_t spot) {
    return spot && mem_read8(ctx, spot + 8) == 2;
}

/* ---- building the entries --------------------------------------------- */

enum { STEP_NEXT, STEP_NEW, STEP_SCROLL, STEP_NAME, STEP_ICON };

typedef struct {
    uint32_t tls, lr, menu, list, reader, hero, step, item, sp;
} HeroAdd;
static HeroAdd hero_adds[4];

static HeroAdd *hero_add_find(Context *ctx) {
    for (unsigned i = 0; i < sizeof hero_adds / sizeof *hero_adds; i++)
        if (hero_adds[i].lr && hero_adds[i].tls == ctx->tls)
            return &hero_adds[i];
    return 0;
}

/* a guest call that returns to BasicMenu::Create's address, so to us */
static void hero_call(Context *ctx, uint32_t target) {
    ctx->r[14] = BASIC_MENU_CREATE;
    ctx->r[15] = target;
    ctx->thumb = 0;
    CALL(recomp_call);
}

/* the game's own BasicMenu::Create */
static void hero_create(Context *ctx) {
    CALL(RECOMP_ORIGINAL(0x004FA880));
}

static void hero_append(Context *ctx, HeroAdd *add) {
    uint32_t count = 0;
    while (count + 1 < LIST_ROOM && mem_read32(ctx, add->list + 4 * count))
        count++;
    if (count + 1 < LIST_ROOM) {
        mem_write32(ctx, add->list + 4 * count, add->item);
        mem_write32(ctx, add->list + 4 * (count + 1), 0);
    }
    /* else the item is lost: there is no room (never with 62 contents) */
}

/* the list holds the Heroes already: the game's Create was entered again at
   its start after ours ran (its first block out of budget comes back to
   0x004FA880 through the host, so to this override) */
static int hero_listed(Context *ctx, uint32_t list) {
    for (uint32_t i = 0; i < LIST_ROOM; i++) {
        uint32_t item = mem_read32(ctx, list + 4 * i);
        if (!item)
            return 0;
        if (mem_read32(ctx, item) == ITEM_VTABLE && is_hero_id(mem_read16(ctx, item + ITEM_ID)))
            return 1;
    }
    return 0;
}

/* called first by the BasicMenu::Create override; 1 when it took the call */
int haiku_code_is_jpn(void); /* jpn: eur_region.c */

int amiibo_menu_create(Context *ctx) {
    HeroAdd *add = hero_add_find(ctx);
    if (!add) {
        if (ctx->r[14] != LIST_CREATE_RETURN || hero_listed(ctx, ctx->r[1]))
            return 0;
        for (unsigned i = 0; i < sizeof hero_adds / sizeof *hero_adds && !add; i++)
            if (!hero_adds[i].lr)
                add = &hero_adds[i];
        if (!add)
            return 0;
        add->tls = ctx->tls;
        add->lr = ctx->r[14];
        add->menu = ctx->r[0];
        add->list = ctx->r[1];
        add->reader = ctx->r[6]; /* CreateBind keeps the reader in r6 */
        add->hero = (uint32_t)-1;
        add->step = STEP_NEXT;
        add->item = 0;
        if (mem_read32(ctx, add->reader + READER_COUNT) >= ICON_SLOT)
            add->hero = HEROES; /* no free icon slots: leave the list as it is */
    }
    for (;;) {
        ctx->budget -= 8;
        switch (add->step) {
        case STEP_NEXT: {
            add->hero++;
            while (add->hero < HEROES && !hero_spot(ctx, add->hero))
                add->hero++;
            if (add->hero >= HEROES) {
                /* the game's own Create with the longer list */
                uint32_t menu = add->menu, list = add->list, lr = add->lr;
                add->lr = 0;
                ctx->r[0] = menu;
                ctx->r[1] = list;
                ctx->r[14] = lr;
                ctx->r[15] = BASIC_MENU_CREATE;
                ctx->thumb = 0;
                hero_create(ctx);
                return 1;
            }
            add->step = STEP_NEW;
            ctx->r[0] = haiku_code_is_jpn() ? ITEM_SIZE_JPN : ITEM_SIZE;
            hero_call(ctx, OPERATOR_NEW);
            break;
        }
        case STEP_NEW: {
            uint32_t item = ctx->r[0];
            if (!item) {
                add->step = STEP_NEXT;
                continue;
            }
            const uint32_t size = haiku_code_is_jpn() ? ITEM_SIZE_JPN : ITEM_SIZE;
            for (uint32_t at = 0; at < size; at += 4)
                mem_write32(ctx, item + at, 0);
            add->item = item;
            add->step = STEP_SCROLL;
            if (haiku_code_is_jpn())
                continue; /* jpn: no ScrollText in this build's item */
            ctx->r[0] = item + ITEM_SCROLL;
            hero_call(ctx, SCROLLTEXT_CTOR);
            break;
        }
        case STEP_SCROLL: {
            uint32_t item = add->item, spot = hero_spot(ctx, add->hero);
            mem_write32(ctx, item, ITEM_VTABLE);
            /* the game keeps no turn record for a Hero Battle (Spot::UpdateSpot
               stores GameUserData +0x30 at spot +0xA, which read 5 after a
               one-turn win: not this map's turns), so the Record column
               shows the game's "no record" dash, and a cleared battle is
               told by its greyed-out look */
            int cleared = hero_cleared(ctx, spot);
            mem_write16(ctx, item + ITEM_ID, (uint16_t)(HERO_ID_BASE + add->hero + (cleared ? HERO_ID_CLEARED : 0)));
            mem_write32(ctx, item + ITEM_TURNS, 0);
            mem_write8(ctx, item + ITEM_CLEARED, 0);
            add->step = STEP_NAME;
            ctx->r[0] = mem_read32(ctx, spot + 4); /* the Chapter */
            hero_call(ctx, CHAPTER_TITLE);
            break;
        }
        case STEP_NAME: {
            uint32_t name = ctx->r[0], item = add->item, n = 0;
            for (; name && n + 1 < ITEM_NAME_MAX; n++) {
                uint16_t c = mem_read16(ctx, name + 2 * n);
                if (!c)
                    break;
                mem_write16(ctx, item + ITEM_NAME + 2 * n, c);
            }
            mem_write16(ctx, item + ITEM_NAME + 2 * n, 0);
            uint32_t slot = add->reader + READER_ICONS + 4 * (ICON_SLOT + add->hero);
            uint32_t texture = mem_read32(ctx, slot);
            if (texture) {
                /* the list was built before with this reader: its texture */
                mem_write32(ctx, item + ITEM_ICON, texture);
                hero_append(ctx, add);
                add->step = STEP_NEXT;
                continue;
            }
            /* the IconData on the stack, as ContentsReader::Load has it */
            add->step = STEP_ICON;
            add->sp = ctx->r[13];
            uint32_t buffer = (ctx->r[13] - AMIIBO_ICON_WORDS * 2) & ~7u;
            for (uint32_t i = 0; i < AMIIBO_ICON_WORDS; i++)
                mem_write16(ctx, buffer + 2 * i, amiibo_icons[add->hero][i]);
            ctx->r[13] = buffer - 8;
            ctx->r[0] = buffer;
            hero_call(ctx, ICON_TO_TEXTURE);
            break;
        }
        case STEP_ICON: {
            uint32_t texture = ctx->r[0];
            ctx->r[13] = add->sp;
            mem_write32(ctx, add->reader + READER_ICONS + 4 * (ICON_SLOT + add->hero), texture);
            mem_write32(ctx, add->item + ITEM_ICON, texture);
            hero_append(ctx, add);
            add->step = STEP_NEXT;
            continue;
        }
        }
        /* the guest call came straight back to 0x004FA880: go on here.
           Otherwise it left for the host (we come back at 0x004FA880 and
           carry on from the record), or its return was dispatched, which
           already ran the next steps in a nested call of this function and
           left r15 where the code goes on: either way this frame is done. */
        if (ctx->exit || ctx->r[15] != BASIC_MENU_CREATE || ctx->thumb)
            return 1;
    }
}

/* for gate.c's BasicMenuItem::BuildAttribute: 2 (greyed out) for a cleared
   Hero Battle, 0 for any other item */
uint32_t amiibo_item_attribute(Context *ctx, uint32_t item) {
    if (mem_read32(ctx, item) != ITEM_VTABLE)
        return 0;
    uint32_t id = mem_read16(ctx, item + ITEM_ID);
    if (!is_hero_id(id))
        return 0;
    return (id & HERO_ID_CLEARED) ? 2u : 0u;
}

/* link::ec::play::anonymous_namespace::Menu::MenuItem::ACall(void): a
   cleared Hero Battle only gives the menu's "miss" sound (0x1000,
   BasicMenu::SEMiss) instead of "Load this content?" */
RECOMP_OVERRIDE(0x003F4608) {
    uint32_t item = ctx->r[0];
    uint32_t id = mem_read16(ctx, item + ITEM_ID);
    if (mem_read32(ctx, item) != ITEM_VTABLE || !is_hero_id(id) || !(id & HERO_ID_CLEARED)) {
        CALL(RECOMP_ORIGINAL(0x003F4608));
        return;
    }
    ctx->r[0] = 0x1000;
    ctx->budget -= 8;
    RETURN_TO(ctx->r[14]);
}

/* ---- starting the battle ------------------------------------------------ */

/* link::ec::play::anonymous_namespace::ProcSequence::ContentsMenuResult */
RECOMP_OVERRIDE(0x003F3CF8) {
    uint32_t id = mem_read16(ctx, PLAY_SELECT + 2);
    if (!mem_read8(ctx, PLAY_SELECT) || !is_hero_id(id)) {
        CALL(RECOMP_ORIGINAL(0x003F3CF8));
        return;
    }
    uint32_t spot = hero_spot(ctx, hero_of(id));
    /* a cleared (greyed-out) battle is never started: nothing is decided,
       and the sequence closes the list as if it were cancelled */
    if (spot && !hero_cleared(ctx, spot)) {
        /* what MI_NextChapterYes::ACall does for a world-map spot */
        uint32_t userdata = mem_read32(ctx, USERDATA_PTR);
        mem_write32(ctx, userdata + 0xC, mem_read32(ctx, spot + 4));
        uint32_t mainseq = mem_read32(ctx, MAINSEQ_PTR);
        if (mainseq) {
            mem_write8(ctx, mainseq + 0x98, 5);
            mem_write32(ctx, mainseq + 0x9C, 0xFFFFFFFFu);
        }
        mem_write8(ctx, PLAY_DECIDED, 1);
    }
    ctx->budget -= 24;
    RETURN_TO(ctx->r[14]);
}

/* ---- the first scan's gift ---------------------------------------------- */

/* The first scan of a Hero's amiibo runs amiibo::Talk_1 of AMB_<hero>.cmb in
   My Castle: the Hero's greeting, "Which gift do you want?" with two
   accessories (ev::Dialog2), ev::AccessoryGain with its "obtained" message,
   and the Hero's farewell. castle::AmiiboTalkSequence::CreateBind
   (0x0045BAE8) runs the talk for the Hero's castle::Amiibo::Data stage
   (amiibo::Talk_<stage + 1>), and AmiiboSeq::PostTalk then raises the stage,
   which is the game's own record that the gift was given.

   So when the castle is entered after a Hero Battle was won (its Spot is
   cleared) and that Hero's stage is still 0 (no amiibo was ever scanned for
   him: a player who scanned on a 3DS has stage 1 or more and already has
   the gift), the castle runs Talk_1 for him, once. The hook is the castle's
   entry step for birthday events, castle::CastleSeq::InitEnterEventBirthday
   (0x0049C284): a step that starts a bound sequence and is called again when
   it ends, at which point the stage is 1 and the game's birthday check runs
   as before. The talk needs the Hero's Unit (AmiiboTalkSequence and
   Amiibo::GetType read its person), so a Hero who was declined at "Recruit
   X?" gets the talk only once he is in the army. */

#define CASTLE_SEQ_BIRTHDAY_DONE 0xA4u /* CastleSeq byte: a birthday sequence ran this entry */

static uint32_t hero_gift_tried; /* heroes whose talk was started in this run */

/* two guest strings are equal */
static int guest_same(Context *ctx, uint32_t a, uint32_t b) {
    if (!a || !b)
        return 0;
    for (uint32_t i = 0; i < 64; i++) {
        uint8_t c = mem_read8(ctx, a + i);
        if (c != mem_read8(ctx, b + i))
            return 0;
        if (!c)
            return 1;
    }
    return 0;
}

/* the Hero's unit in the army (forces 0 to 3), or 0 */
static uint32_t hero_unit(Context *ctx, uint32_t k) {
    uint32_t forces = mem_read32(ctx, FORCES_PTR);
    uint32_t pid = mem_read32(ctx, AMIIBO_PIDS + 4 * (k + 1)); /* "PID_<hero>" */
    if (!forces || !pid)
        return 0;
    for (uint32_t force = 0; force < 4; force++) {
        uint32_t unit = mem_read32(ctx, forces + force * 12);
        for (int seen = 0; unit && seen < 256; seen++) {
            uint32_t person = mem_read32(ctx, unit + 0x9C);
            if (person && guest_same(ctx, mem_read32(ctx, person + 8), pid))
                return unit;
            unit = mem_read32(ctx, unit + 0x14);
        }
    }
    return 0;
}

/* castle::CastleSeq::InitEnterEventBirthday(void) */
RECOMP_OVERRIDE(0x0049C284) {
    uint32_t seq = ctx->r[0];
    uint32_t castle = mem_read32(ctx, CASTLE_PTR);
    /* not in another player's castle (castle::cut::InOthersCastle) */
    uint32_t state = castle ? mem_read32(ctx, castle + 4) : 0;
    if (castle && state && mem_read8(ctx, state) != 1 && !mem_read8(ctx, seq + CASTLE_SEQ_BIRTHDAY_DONE)) {
        uint32_t data = mem_read32(ctx, castle + 0x10);
        for (uint32_t k = 0; data && k < HEROES; k++) {
            if (hero_gift_tried & 1u << k)
                continue;
            uint32_t amiibo = mem_read32(ctx, data + 4 * (k + 1)); /* AmiiboType k + 1 */
            if (!amiibo || mem_read8(ctx, amiibo + 4) != 0 || !hero_cleared(ctx, hero_spot(ctx, k)))
                continue;
            uint32_t unit = hero_unit(ctx, k);
            if (!unit)
                continue;
            hero_gift_tried |= 1u << k;
            /* AmiiboTalkSequence::CreateBind(this, unit), returning to our
               caller: this step is called again when the talk ends */
            ctx->r[1] = unit;
            ctx->r[15] = AMIIBO_TALK;
            ctx->thumb = 0;
            ctx->budget -= 40;
            CALL(recomp_call);
            return;
        }
    }
    CALL(RECOMP_ORIGINAL(0x0049C284));
}

/* pulsefix: what of this file is the console's state, kept with Divine
   Pulse's moments and saved states (overrides/keep.c) */
void amiibo_keep(void (*add)(const char *, void *, uint32_t)) {
    add("amiibo.hero_adds", &hero_adds, sizeof hero_adds);
    add("amiibo.hero_gift_tried", &hero_gift_tried, sizeof hero_gift_tried);
}
