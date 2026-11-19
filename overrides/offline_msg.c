/* offline-msg: "Online features aren't available in this version."
 *
 * Every Internet failure in the game (Update Data, Visit Castles, Clear
 * Blacklist, Internet Battle, the eShop sessions) ends in
 * HomeButton::StartErrEulaApplet(ProcInst* parent, int code), 0x0017CC4C.
 * It creates a child proc, ProcErrorCode, under the proc that failed; that
 * proc's Exec starts the console's error display (erreula) with the code
 * and waits for it to close. The callers are link::ProcShowError::Start
 * (case 1), link::ProcErrorReport::ShowCode and four eShop session procs.
 * PC has no error display (pcos/applet.rs closes it at once), so the player
 * used to be sent back to the menu without a word.
 *
 * The override asks the native layer how to answer (pcos/online.rs, op
 * 0x700). While no network backend is connected, the answer is "not
 * available", and the override shows the game's own message window in
 * place of the error display: GameMessage::CreateKeyWait(parent, text),
 * 0x0019A908. That is the window, child proc and A-to-close wait the same
 * callers use for the game's other link messages (ProcShowError cases 2 to
 * 6, ProcErrorReport::ShowMessage/ShowLabel, e.g. "Please wait and try the
 * update again later."), so the parent waits for it exactly as it waited
 * for ProcErrorCode and carries on as before once it closes.
 *
 * The text goes where the game's own message text comes from: the message
 * system's shared buffer (Mess, state at 0x006DCBDC: +4 the buffer, +8 its
 * size in UTF-16 units), which Mess::Get returns for composed messages and
 * callers hand straight to CreateKeyWait. It is written at the end of the
 * buffer, away from the start where Mess::Get composes, and GameMessage's
 * constructor copies it before anything else runs. The language follows
 * the game's (Lang::Folder's index at 0x006BDA6C: 1 English, 2 French,
 * 5 Spanish; the others fall back to English).
 *
 * Otherwise (a backend connected, or the native layer off) the original
 * runs and starts the error display as before.
 */
#include "overrides.h"

typedef int (*OsNativeHook)(Context *ctx, uint32_t op);
extern OsNativeHook os_native_hook;

#define OFFLINE_ERROR_DISPLAY       0x700u /* pcos::online::op::ERROR_DISPLAY */
#define OFFLINE_NOT_AVAILABLE       1u     /* pcos::online::Answer::NotAvailable */

#define GAME_MESSAGE_CREATE_KEY_WAIT 0x0019A908u /* GameMessage::CreateKeyWait(ProcInst*, const wchar_t*) */
#define MESS_STATE                   0x006DCBDCu /* +4 buffer, +8 size in units */
#define HOME_BUTTON_STATE            0x006BD8E8u /* +8: set by StartErrEulaApplet */
#define LANG_INDEX                   0x006BDA6Cu /* Lang::Folder's language */

static const uint16_t offline_text_en[] =
    u"Online features aren't available\nin this version.";
static const uint16_t offline_text_fr[] =
    u"Les fonctions en ligne ne sont pas\ndisponibles dans cette version.";
static const uint16_t offline_text_es[] =
    u"Las funciones en línea no están\ndisponibles en esta versión.";
/* eur: the EUR build's other two languages (Lang::Folder 3 and 4) */
static const uint16_t offline_text_de[] =
    u"Online-Funktionen sind in dieser\nVersion nicht verfügbar.";
static const uint16_t offline_text_it[] =
    u"Le funzioni online non sono\ndisponibili in questa versione.";

/* jpprep: the Japanese game's (it has no Lang::Folder) */
static const uint16_t offline_text_ja[] =
    u"このバージョンでは\nオンライン機能を利用できません。";
int haiku_code_is_jpn(void); /* eur_region.c */
int haiku_jpn_japanese(void); /* jpn2: jpn_text.c */

/* HomeButton::StartErrEulaApplet(ProcInst* parent, int code) */
RECOMP_OVERRIDE(0x0017CC4C) {
    const uint32_t parent = ctx->r[0], code = ctx->r[1];
    if (os_native_hook && os_native_hook(ctx, OFFLINE_ERROR_DISPLAY) && ctx->r[0] == OFFLINE_NOT_AVAILABLE) {
        const uint16_t *text = offline_text_en;
        uint32_t length = sizeof offline_text_en / 2; /* with the terminating zero */
        switch (haiku_code_is_jpn() ? 0 : mem_read8(ctx, LANG_INDEX)) {
        case 0: if (haiku_jpn_japanese()) { text = offline_text_ja; length = sizeof offline_text_ja / 2; } break; /* jpprep; jpn2: English under a translation */
        case 2: text = offline_text_fr; length = sizeof offline_text_fr / 2; break;
        case 5: text = offline_text_es; length = sizeof offline_text_es / 2; break;
        case 3: text = offline_text_de; length = sizeof offline_text_de / 2; break; /* eur */
        case 4: text = offline_text_it; length = sizeof offline_text_it / 2; break; /* eur */
        default: break;
        }
        const uint32_t buffer = mem_read32(ctx, MESS_STATE + 4);
        const uint32_t units = mem_read32(ctx, MESS_STATE + 8);
        if (buffer != 0 && units >= length + 0x40) {
            const uint32_t at = buffer + (units - length) * 2;
            for (uint32_t i = 0; i < length; i++)
                mem_write16(ctx, at + 2 * i, text[i]);
            /* what the original does besides creating its proc */
            mem_write8(ctx, HOME_BUTTON_STATE + 8, 1);
            ctx->budget -= 16;
            /* a tail call, as the original's to ProcInst::Create: r14 is the
               caller's, CreateKeyWait returns straight to it */
            ctx->r[0] = parent;
            ctx->r[1] = at;
            RETURN_TO(GAME_MESSAGE_CREATE_KEY_WAIT);
        }
    }
    ctx->r[0] = parent;
    ctx->r[1] = code;
    CALL(RECOMP_ORIGINAL(0x0017CC4C));
}

/* The message closes as the game's key-wait messages do: A (Enter, Space or
 * Z on the keyboard, A on a controller), or B/Start, after its first few
 * frames (GameMessage::Tick, 0x0019AA20). For the mouse, two more ways, set
 * up on this message only while it ticks:
 *
 * - a touch of the lower screen closes it, as the game's own
 *   CreateKeyTouchWaitLower messages do (flag 4, touch began and released
 *   while it waits);
 * - a click anywhere else on the game is the confirm button: the native
 *   layer is told the message takes the confirm button now (op 0x701) and
 *   the frontend presses A for the click (zakuro/src/offline_msg.rs).
 *
 * Other messages tick exactly as before. */
#define OFFLINE_MESSAGE_WAITING      0x701u /* pcos::online::op::MESSAGE_WAITING */
#define GAME_MESSAGE_TEXT            0x30u  /* wchar_t*, the window's own copy */
#define GAME_MESSAGE_FLAGS           0x50u
#define GAME_MESSAGE_TICKS           0x54u  /* frames waited; A counts past 4 */
#define GAME_MESSAGE_TOUCH_CLOSES    4u

int online_message_is_ours(Context *ctx, uint32_t text); /* online.c */

static int offline_text_is(Context *ctx, uint32_t text, const uint16_t *ours, uint32_t length) {
    for (uint32_t i = 0; i < length; i++)
        if (mem_read16(ctx, text + 2 * i) != ours[i])
            return 0;
    return 1;
}

static int offline_is_ours(Context *ctx, uint32_t message) {
    const uint32_t text = mem_read32(ctx, message + GAME_MESSAGE_TEXT);
    if (text == 0 || (text & 1))
        return 0;
    return offline_text_is(ctx, text, offline_text_en, sizeof offline_text_en / 2)
        || offline_text_is(ctx, text, offline_text_fr, sizeof offline_text_fr / 2)
        || offline_text_is(ctx, text, offline_text_es, sizeof offline_text_es / 2)
        || online_message_is_ours(ctx, text); /* online2: "Characters modified with cheats..." */
}

/* GameMessage::Tick(void) */
RECOMP_OVERRIDE(0x0019AA20) {
    const uint32_t message = ctx->r[0];
    if (offline_is_ours(ctx, message)) {
        const uint32_t flags = mem_read32(ctx, message + GAME_MESSAGE_FLAGS);
        mem_write32(ctx, message + GAME_MESSAGE_FLAGS, flags | GAME_MESSAGE_TOUCH_CLOSES);
        if ((int32_t)mem_read32(ctx, message + GAME_MESSAGE_TICKS) >= 4 && os_native_hook) {
            uint32_t saved[4] = {ctx->r[0], ctx->r[1], ctx->r[2], ctx->r[3]};
            os_native_hook(ctx, OFFLINE_MESSAGE_WAITING);
            ctx->r[0] = saved[0]; ctx->r[1] = saved[1]; ctx->r[2] = saved[2]; ctx->r[3] = saved[3];
        }
    }
    CALL(RECOMP_ORIGINAL(0x0019AA20));
}
