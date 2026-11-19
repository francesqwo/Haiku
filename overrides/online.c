/* online: the game's Local Battle, played online between two Haiku.
 *
 * Wireless Battle > Local Battle is the game's own two-console battle: its
 * Host/Join choice (SelectMasterClientMenu), the host's wait for a player
 * and its confirmation, the joiner's list of networks nearby
 * (SelectNetworkMenu), then the team exchange, the host's rules and map,
 * and the battle, all over the game's Pia session on local wireless
 * (nwm::UDS). Haiku carries that local wireless between two PCs
 * (pcos/uds.rs over crate::netplay), so all of it runs unchanged. This file
 * only tells the native layer where the player is, and names the entries
 * for what they are on PC:
 *
 * - link::match::Local::Initialize (0x00406958), the Local Battle's set-up:
 *   local wireless goes online from here (op 0x710). My Castle's Parley,
 *   which uses local wireless too, still finds it off.
 * - versus::Sequence's ProcSequence destructor (0x004C5EB4): the player
 *   left Wireless Battle (op 0x711). (link::match::Local::Finalize's entry
 *   in the symbol table, 0x00408908, is inside a NEX function, so the
 *   Wireless Battle sequence's end is used.)
 * - ProcLocal::SelectNetwork (0x00406194): the joiner's list of networks
 *   opens; the frontend asks for the host's code (op 0x712).
 * - The names and help lines of the Wireless Battle menu's Local Battle
 *   (versus::TopMenu's TopMenuItem, type 1) and of the Host/Join choice
 *   (SelectMasterClientMenu's MasterMenuItem / ClientMenuItem): "Online
 *   Battle", "Host", "Join", in the game's language (English, French,
 *   Spanish, as the USA release ships). The text is written into the
 *   message system's shared buffer (Mess, 0x006DCBDC: +4 the buffer, +8 its
 *   size in UTF-16 units), 0x180 units from its end, clear of where
 *   Mess::Get composes (the start) and of offline_msg.c's text (the last
 *   ~50 units); the menus take it as they take Mess::Get's.
 */
#include "overrides.h"

typedef int (*OsNativeHook)(Context *ctx, uint32_t op);
extern OsNativeHook os_native_hook;

#define ONLINE_BATTLE_BEGIN 0x710u /* pcos::uds::op */
#define ONLINE_BATTLE_END   0x711u
#define ONLINE_JOIN_LIST    0x712u

#define ONLINE_MESS_STATE   0x006DCBDCu /* +4 buffer, +8 size in units */
#define ONLINE_LANG_INDEX   0x006BDA6Cu /* Lang::Folder's language: 1 en, 2 fr, 3 de, 4 it, 5 es */
#define ONLINE_TEXT_FROM_END 0x180u     /* units before the buffer's end */
#define ONLINE_TEXT_ROOM     0x100u     /* units the texts may take */

static void online_tell(Context *ctx, uint32_t op) {
    if (!os_native_hook)
        return;
    uint32_t saved[4] = {ctx->r[0], ctx->r[1], ctx->r[2], ctx->r[3]};
    os_native_hook(ctx, op);
    ctx->r[0] = saved[0]; ctx->r[1] = saved[1]; ctx->r[2] = saved[2]; ctx->r[3] = saved[3];
}

/* link::match::Local::Initialize(void) */
RECOMP_OVERRIDE(0x00406958) {
    online_tell(ctx, ONLINE_BATTLE_BEGIN);
    CALL(RECOMP_ORIGINAL(0x00406958));
}

/* versus::Sequence::(anonymous)::ProcSequence::~ProcSequence(void) */
RECOMP_OVERRIDE(0x004C5EB4) {
    online_tell(ctx, ONLINE_BATTLE_END);
    CALL(RECOMP_ORIGINAL(0x004C5EB4));
}

/* link::match::(anonymous)::ProcLocal::SelectNetwork(void) */
RECOMP_OVERRIDE(0x00406194) {
    online_tell(ctx, ONLINE_JOIN_LIST);
    CALL(RECOMP_ORIGINAL(0x00406194));
}

/* the texts, by slot: English, French, Spanish (German and Italian below) */
enum { T_NAME, T_HELP, T_HOST, T_HOST_HELP, T_JOIN, T_JOIN_HELP, T_COUNT };

/* the help line holds about 40 characters */
static const char *const online_en[T_COUNT] = {
    "Online Battle",
    "Battle a friend online with a code.",
    "Host",
    "Get a code to send to your opponent.",
    "Join",
    "Enter the code your opponent sent.",
};
static const char *const online_fr[T_COUNT] = {
    "Combat en ligne",
    "Combattez un ami en ligne avec un code.",
    "Organiser",
    "Obtenez un code pour votre adversaire.",
    "Rejoindre",
    "Entrez le code de votre adversaire.",
};
static const char *const online_es[T_COUNT] = {
    "Combate en línea",
    "Combate en línea con un có" "digo.",
    "Organizar",
    "Obtén un có" "digo para tu rival.",
    "Unirse",
    "Escribe el có" "digo de tu rival.",
};

/* eur2: the EUR build's German and Italian (Lang::Folder 3 and 4), in the
   game's own terms (Lokaler Kampf, Sitzung beitreten; Battaglia locale; the
   Italian name is not Battaglia online, the game's Internet battle) */
static const char *const online_de[T_COUNT] = {
    "Online-Kampf",
    "Fordere Freunde online per Code heraus.",
    "Erstellen",
    "Erhalte einen Code für deinen Gegner.",
    "Beitreten",
    "Gib den Code deines Gegners ein.",
};
static const char *const online_it[T_COUNT] = {
    "Sfida online",
    "Sfida un amico online con un codice.",
    "Crea",
    "Ottieni un codice da inviare al rivale.",
    "Unisciti",
    "Inserisci il codice del tuo rivale.",
};

/* writes a text (Latin-1) into the shared buffer and returns where, or 0
   when the buffer is too small to hold it safely */
int haiku_code_is_jpn(void);                 /* jpprep: eur_region.c */
const char *haiku_text_ja(const char *english); /* jpprep: jpn_text.c */
uint32_t haiku_utf8_next(const unsigned char **text);

static uint32_t online_text(Context *ctx, int slot) {
    const char *const *table = online_en;
    /* jpprep: the Japanese game has no Lang::Folder (jpn_text.c's texts) */
    switch (haiku_code_is_jpn() ? 0 : mem_read8(ctx, ONLINE_LANG_INDEX)) {
    case 2: table = online_fr; break;
    case 5: table = online_es; break;
    case 3: table = online_de; break; /* eur2 */
    case 4: table = online_it; break; /* eur2 */
    default: break;
    }
    const uint32_t buffer = mem_read32(ctx, ONLINE_MESS_STATE + 4);
    const uint32_t units = mem_read32(ctx, ONLINE_MESS_STATE + 8);
    if (buffer == 0 || units < ONLINE_TEXT_FROM_END + 0x200)
        return 0;
    /* each slot has its own place, so two texts shown together (a name and
       its help line) do not overwrite each other */
    const uint32_t per = ONLINE_TEXT_ROOM / T_COUNT;
    const uint32_t at = buffer + (units - ONLINE_TEXT_FROM_END + per * (uint32_t)slot) * 2;
    /* online2: the texts are UTF-8 in this file (é, í, ó), one unit each */
    const char *chosen = table[slot];
    if (haiku_code_is_jpn() && haiku_text_ja(online_en[slot]))
        chosen = haiku_text_ja(online_en[slot]); /* jpprep */
    const unsigned char *text = (const unsigned char *)chosen;
    uint32_t i = 0;
    while (*text && i + 1 < per) {
        const uint32_t c = haiku_utf8_next(&text); /* jpprep: up to three bytes */
        mem_write16(ctx, at + 2 * i++, (uint16_t)c);
    }
    mem_write16(ctx, at + 2 * i, 0);
    return at;
}

#define ONLINE_TOP_ITEM_TYPE 0x38u /* TopMenuItem: 1 Local Battle, 2 Internet, 5 Create Team, 6 Clear Blacklist */

/* versus::TopMenu::(anonymous)::TopMenuItem::GetName(void) */
RECOMP_OVERRIDE(0x004C5638) {
    if (mem_read8(ctx, ctx->r[0] + ONLINE_TOP_ITEM_TYPE) == 1) {
        const uint32_t text = online_text(ctx, T_NAME);
        if (text) {
            ctx->r[0] = text;
            RETURN_TO(ctx->r[14]);
        }
    }
    CALL(RECOMP_ORIGINAL(0x004C5638));
}

/* versus::TopMenu::(anonymous)::TopMenuItem::GetHelp(void) */
RECOMP_OVERRIDE(0x004C5578) {
    if (mem_read8(ctx, ctx->r[0] + ONLINE_TOP_ITEM_TYPE) == 1) {
        const uint32_t text = online_text(ctx, T_HELP);
        if (text) {
            ctx->r[0] = text;
            RETURN_TO(ctx->r[14]);
        }
    }
    CALL(RECOMP_ORIGINAL(0x004C5578));
}

static void online_label(Context *ctx, int slot, void (*original)(Context *)) {
    const uint32_t text = online_text(ctx, slot);
    if (text) {
        ctx->r[0] = text;
        RETURN_TO(ctx->r[14]);
    }
    CALL(original);
}

/* SelectMasterClientMenu::MasterMenuItem::GetName / GetHelp,
   ClientMenuItem::GetName / GetHelp */
RECOMP_OVERRIDE(0x00405F64) { online_label(ctx, T_HOST, RECOMP_ORIGINAL(0x00405F64)); }
RECOMP_OVERRIDE(0x00405F3C) { online_label(ctx, T_HOST_HELP, RECOMP_ORIGINAL(0x00405F3C)); }
RECOMP_OVERRIDE(0x00405F14) { online_label(ctx, T_JOIN, RECOMP_ORIGINAL(0x00405F14)); }
RECOMP_OVERRIDE(0x00405EEC) { online_label(ctx, T_JOIN_HELP, RECOMP_ORIGINAL(0x00405EEC)); }

/* ---- online2: characters modified with cheats -----------------------------
 *
 * The flag. A unit the Cheats tab changed carries bit 31 of its state word's
 * upper half (Unit +0xC, the high word of the 64-bit state at +8), which
 * Haiku sets (zakuro/src/cheats.rs, cheats.c) and the game never reads or
 * clears: the game's own masks on that word are small (0x1 to 0xC0, in
 * Unit::ClassChange, TransferImpl, ResetEndOfMap, Handover, Reinstate and
 * unit::util's versus-team and end-of-chapter flags), Unit::Clear zeroes it
 * for a new unit, and Unit::Serialize/Deserialize keep all 64 bits, so the
 * bit lives in the save as the unit does, and a save with it loads like any
 * other.
 *
 * Teams for battles are game::packet::Unit records (the versus team files,
 * the team the other console gets), made by Unit::ToPacket and read back by
 * Unit::CreateFromPacket, which keep only some state bits. The flag rides in
 * the record's flags (+0x9E, u16) as bit 0x8000, which no reader of that
 * field tests (the game uses 0x2 to 0x80): ToPacket's tail sets it from the
 * unit, CreateFromPacket's tail sets the unit's from it. A record without it
 * is the game's own, byte for byte.
 *
 * Only an online battle whose host said "Don't allow" looks at it (op 0x713
 * answers that):
 * - Select Team greys a team with such a character (TeamSelectMenuItem::
 *   BuildAttribute answers 2, as for an empty slot), and choosing it shows
 *   the game's message window with the reason instead of "Use this team?";
 * - CheckVersusData (both consoles, once the teams are swapped) ends the
 *   match through the game's own invalid-data path when either team still
 *   has one (a team that slipped past Select Team, a changed program), and
 *   tells the native layer why (op 0x714), so both players are told. */

#define ONLINE_CHEATS_FORBIDDEN 0x713u /* pcos::uds::op: r0 1 when the host doesn't allow them (asked with r0 0 from Select Team, 1 from the teams' check) */
#define ONLINE_CHEATED_TEAM     0x714u /* r0: 1 a greyed team was chosen; 2 | sides << 4 the teams were refused */

#define UNIT_STATE_HIGH      0x0Cu
#define UNIT_CHEATED         0x80000000u
#define PACKET_FLAGS         0x9Eu
#define PACKET_CHEATED       0x8000u

#define TEAM_UNIT_SIZE       0x178u  /* game::packet::ExtendedUnit */
#define TEAM_COUNT           0xEB0u  /* u8, units in the team */
#define DATA_VALID           0x00u   /* versus::Data: u8, the slot holds a team */
#define DATA_TEAM            0x08u   /* versus::Data: game::packet::Team */
#define ITEM_DATA            0x38u   /* TeamSelectMenuItem: its versus::Data */
#define ITEM_MENU            0x0Cu   /* menu item: its menu's proc */
#define MENU_EDIT_MODE       0x134u  /* TeamSelectMenu: u8, Create Team's list (not Select Team's) */
#define VERSUS_CONFIG        0x006DA318u /* versus::Config* (what Config::Get reads) */

#define GAME_MESSAGE_KEY_WAIT 0x0019A908u /* GameMessage::CreateKeyWait(ProcInst*, const wchar_t*) */
#define MATCH_INVALID_DATA    0x004049A8u /* link::match::Game::DetectInvalidData */

static uint32_t online_ask(Context *ctx, uint32_t op, uint32_t arg) {
    if (!os_native_hook)
        return 0;
    uint32_t saved[4] = {ctx->r[0], ctx->r[1], ctx->r[2], ctx->r[3]};
    ctx->r[0] = arg;
    uint32_t answer = os_native_hook(ctx, op) ? ctx->r[0] : 0;
    ctx->r[0] = saved[0]; ctx->r[1] = saved[1]; ctx->r[2] = saved[2]; ctx->r[3] = saved[3];
    return answer;
}

/* Unit::ToPacket(game::packet::Unit*, bool) const: the flag into the record */
static void online_to_packet_tail(Context *ctx, const uint32_t *words) {
    const uint32_t unit = words[0], packet = words[1];
    if (unit && packet && (mem_read32(ctx, unit + UNIT_STATE_HIGH) & UNIT_CHEATED))
        mem_write16(ctx, packet + PACKET_FLAGS, mem_read16(ctx, packet + PACKET_FLAGS) | PACKET_CHEATED);
}

RECOMP_OVERRIDE(0x00533ABC) {
    CALL_THEN(RECOMP_ORIGINAL(0x00533ABC), online_to_packet_tail, ctx->r[0], ctx->r[1]);
}

/* Unit::CreateFromPacket(const game::packet::Unit*): and back into the unit */
static void online_from_packet_tail(Context *ctx, const uint32_t *words) {
    const uint32_t unit = words[0], packet = words[1];
    if (unit && packet && (mem_read16(ctx, packet + PACKET_FLAGS) & PACKET_CHEATED))
        mem_write32(ctx, unit + UNIT_STATE_HIGH, mem_read32(ctx, unit + UNIT_STATE_HIGH) | UNIT_CHEATED);
}

RECOMP_OVERRIDE(0x003D6258) {
    CALL_THEN(RECOMP_ORIGINAL(0x003D6258), online_from_packet_tail, ctx->r[0], ctx->r[1]);
}

/* a team (game::packet::Team) with a flagged character */
static int online_team_cheated(Context *ctx, uint32_t team) {
    if (!team)
        return 0;
    uint32_t count = mem_read8(ctx, team + TEAM_COUNT);
    if (count > 10)
        count = 10;
    for (uint32_t i = 0; i < count; i++)
        if (mem_read16(ctx, team + i * TEAM_UNIT_SIZE + PACKET_FLAGS) & PACKET_CHEATED)
            return 1;
    return 0;
}

/* a Select Team (not Create Team) slot whose team the host's rule refuses */
static int online_item_refused(Context *ctx, uint32_t item) {
    const uint32_t menu = mem_read32(ctx, item + ITEM_MENU);
    if (!menu || mem_read8(ctx, menu + MENU_EDIT_MODE) || !mem_read8(ctx, item + ITEM_DATA + DATA_VALID))
        return 0;
    if (!online_team_cheated(ctx, item + ITEM_DATA + DATA_TEAM))
        return 0;
    return online_ask(ctx, ONLINE_CHEATS_FORBIDDEN, 0) == 1;
}

/* versus::TeamSelectMenu::TeamSelectMenuItem::BuildAttribute(void) */
RECOMP_OVERRIDE(0x004C10FC) {
    if (online_item_refused(ctx, ctx->r[0])) {
        ctx->r[0] = 2; /* greyed, as an empty slot */
        RETURN_TO(ctx->r[14]);
    }
    CALL(RECOMP_ORIGINAL(0x004C10FC));
}

static const uint16_t online_cheats_en[] =
    u"Characters modified with cheats are\nnot allowed in this battle.";
static const uint16_t online_cheats_fr[] =
    u"Les personnages modifiés par des codes\nde triche ne sont pas autorisés dans\nce combat.";
static const uint16_t online_cheats_es[] =
    u"Los personajes modificados con trucos\nno están permitidos en este combate.";

/* jpprep: the Japanese game's */
static const uint16_t online_cheats_ja[] =
    u"チートで変更されたキャラクターは\nこの対戦には出撃できません。";

int haiku_jpn_japanese(void); /* jpn2: jpn_text.c */

static const uint16_t *online_cheats_text(Context *ctx, uint32_t *length) {
    if (haiku_code_is_jpn()) {
        /* jpn2: English under an English translation */
        if (!haiku_jpn_japanese()) {
            *length = sizeof online_cheats_en / 2;
            return online_cheats_en;
        }
        *length = sizeof online_cheats_ja / 2;
        return online_cheats_ja;
    }
    switch (mem_read8(ctx, ONLINE_LANG_INDEX)) {
    case 2: *length = sizeof online_cheats_fr / 2; return online_cheats_fr;
    case 5: *length = sizeof online_cheats_es / 2; return online_cheats_es;
    default: *length = sizeof online_cheats_en / 2; return online_cheats_en;
    }
}

/* for offline_msg.c's GameMessage::Tick hook: this window closes with a
   click too, as its own does */
int online_message_is_ours(Context *ctx, uint32_t text) {
    const uint16_t *texts[4] = {online_cheats_en, online_cheats_fr, online_cheats_es, online_cheats_ja};
    const uint32_t lengths[4] = {sizeof online_cheats_en / 2, sizeof online_cheats_fr / 2, sizeof online_cheats_es / 2, sizeof online_cheats_ja / 2};
    for (int t = 0; t < 4; t++) { /* jpprep: and the Japanese */
        uint32_t i = 0;
        while (i < lengths[t] && mem_read16(ctx, text + 2 * i) == texts[t][i])
            i++;
        if (i == lengths[t])
            return 1;
    }
    return 0;
}

static void online_acall_tail(Context *ctx, const uint32_t *words) {
    (void)words;
    ctx->r[0] = 0x100; /* a window of the menu's is open, as after "Use this team?" */
}

/* versus::TeamSelectMenu::TeamSelectMenuItem::ACall(void): a greyed team
   says why, in the game's own message window (offline_msg.c's way: the text
   at the end of the message buffer, which GameMessage copies at once; the
   menu waits for its window as TutorialMenu's LocalShow does) */
RECOMP_OVERRIDE(0x004C111C) {
    const uint32_t item = ctx->r[0];
    if (online_item_refused(ctx, item)) {
        uint32_t length;
        const uint16_t *text = online_cheats_text(ctx, &length);
        const uint32_t buffer = mem_read32(ctx, ONLINE_MESS_STATE + 4);
        const uint32_t units = mem_read32(ctx, ONLINE_MESS_STATE + 8);
        if (buffer != 0 && units >= length + 0x200) {
            const uint32_t at = buffer + (units - length) * 2;
            for (uint32_t i = 0; i < length; i++)
                mem_write16(ctx, at + 2 * i, text[i]);
            online_ask(ctx, ONLINE_CHEATED_TEAM, 1);
            ctx->budget -= 16;
            ctx->r[0] = mem_read32(ctx, item + ITEM_MENU);
            ctx->r[1] = at;
            ctx->r[15] = GAME_MESSAGE_KEY_WAIT; /* returns straight to ACall's caller (r14) */
            ctx->thumb = 0;
            CALL_THEN(recomp_call, online_acall_tail, 0);
            return;
        }
        /* no room for the text: the slot stays greyed and silent */
        ctx->r[0] = 0x1000;
        RETURN_TO(ctx->r[14]);
    }
    CALL(RECOMP_ORIGINAL(0x004C111C));
}

/* versus::SequenceMatched::ProcSequence::CheckVersusData(void): the safety
   net. Config +8 is this console's versus::Data, +0xC the other's (what
   ReceiveVersusData read); both consoles look at both teams under the same
   rule, so both end the match together. */
RECOMP_OVERRIDE(0x004C2700) {
    const uint32_t config = mem_read32(ctx, VERSUS_CONFIG);
    if (config) {
        const uint32_t own = mem_read32(ctx, config + 8), other = mem_read32(ctx, config + 0xC);
        const uint32_t sides = (own && online_team_cheated(ctx, own + DATA_TEAM) ? 1u : 0u) | (other && online_team_cheated(ctx, other + DATA_TEAM) ? 2u : 0u);
        if (sides && online_ask(ctx, ONLINE_CHEATS_FORBIDDEN, 1) == 1) {
            online_ask(ctx, ONLINE_CHEATED_TEAM, 2 | sides << 4);
            /* the game's own invalid data (as the original on a team that
               fails Team::IsWrong), a tail call returning to the caller */
            ctx->budget -= 16;
            ctx->r[15] = MATCH_INVALID_DATA;
            ctx->thumb = 0;
            CALL(recomp_call);
            return;
        }
    }
    CALL(RECOMP_ORIGINAL(0x004C2700));
}
