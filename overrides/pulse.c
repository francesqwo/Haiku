/* pulse: Divine Pulse, the modern games' rewind, as an option (USA Special
 * Edition v2). The rewind itself is the host's (zakuro/src/pulse.rs, with
 * zakuro-core's state::rewind): snapshots of the whole console between two
 * frames. This file only tells the host what the game is doing, so it knows
 * when a moment is worth keeping, and adds the map menu's entry:
 *
 * - The player phase's free cursor (FreeCursorTick, 0x00351904, overridden
 *   in info_ui.c, which calls pulse_free_cursor first): the frame of its
 *   last tick. The free cursor coming back after an action (a Wait, a
 *   fight, a Dragon Vein, a trade, a capture, Pair Up...) is a moment before
 *   the next action, as Three Houses keeps one before each of yours.
 * - The enemy and other phases: AI::Processing (0x002238CC, overridden in
 *   info_ui.c, which calls pulse_ai_decided after the real one returns)
 *   decides for one unit at a time and leaves the decision in map::Mind;
 *   each real decision (not info_ui.c's predictions, which run on a copy) is
 *   a moment before that unit acts, as Engage's list has the enemy's
 *   actions too.
 * - map::Situation::SetGameOver (0x003A6094): the battle is lost (Corrin or
 *   another unit that must survive fell, or a lose rule). The host offers
 *   the rewind there, as Three Houses and Engage do before a Game Over;
 *   declining lets the game's own Game Over go on.
 * - The map's System Menu (map::CreateSystemMenu; qol.c's qol_menu_create
 *   adds its items) gets "Divine Pulse" directly above Return to Title (pulse2; above End
 *   when Return to Title is off) while the host sets PW_MENU_ITEM. (Engage puts the Time Crystal first; here the menu's own
 *   cursor, which starts on Unit and goes Up to End, stays as Fates has it,
 *   so a player's habits and recorded inputs still land where they did.) Choosing it closes the
 *   menu the way B does (SystemMenuItem::BCall, 0x003787E8, which hides the
 *   menu's layout), with the decide sound's answer (0x101, as End answers
 *   when it ends the turn without asking), and asks the host to open the
 *   rewind (PW_MENU_REQUEST).
 *
 * With the option off (PW_ON 0) nothing here writes anything but these host
 * words, and every override runs the game's own code alone: the game's
 * memory and the budget are as without this file.
 */
#include "overrides.h"

#define PW_COUNT 64
uint32_t pulse_words[PW_COUNT];

/* the words, by index (zakuro/src/pulse.rs has the same list) */
#define PW_ON 0            /* host: the option is on (and this is not an online battle) */
#define PW_FREE_FRAME 1    /* the frame counter at the last free-cursor tick */
#define PW_AI_CALLS 2      /* real AI decisions, counted */
#define PW_GAME_OVER 3     /* SetGameOver calls, counted */
#define PW_MENU_ITEM 4     /* host: the System Menu gets Divine Pulse */
#define PW_MENU_REQUEST 5  /* Divine Pulse chosen in the System Menu, counted */
#define PW_GAME_OVER_TYPE 6 /* the last SetGameOver's WinLoseResult::Type */
#define PW_AI_UNIT 7       /* the unit (pool index) of the last real decision */

#define FRAME_COUNTER 0x006DCAA0u
#define MIND_PTR 0x006D7568u

/* info_ui.c's FreeCursorTick override, before anything else */
void pulse_free_cursor(Context *ctx) {
    if (pulse_words[PW_ON])
        pulse_words[PW_FREE_FRAME] = mem_read32(ctx, FRAME_COUNTER);
}

/* info_ui.c's AI::Processing override, once the real one returned */
void pulse_ai_decided(Context *ctx) {
    if (!pulse_words[PW_ON])
        return;
    uint32_t mind = mem_read32(ctx, MIND_PTR);
    pulse_words[PW_AI_UNIT] = mind ? mem_read8(ctx, mind) : 0;
    pulse_words[PW_AI_CALLS]++;
}

/* map::Situation::SetGameOver(map::Situation::WinLoseResult::Type) */
RECOMP_OVERRIDE(0x003A6094) {
    if (pulse_words[PW_ON]) {
        pulse_words[PW_GAME_OVER_TYPE] = ctx->r[1];
        pulse_words[PW_GAME_OVER]++;
    }
    CALL(RECOMP_ORIGINAL(0x003A6094));
}

/* qol.c's map menu: whether it adds Divine Pulse */
int pulse_menu_wanted(void) {
    return pulse_words[PW_ON] && pulse_words[PW_MENU_ITEM];
}

/* map::(anonymous)::SystemMenuItem::BCall(void), the generated code */
void f_003787E8(Context *ctx);

static void pulse_menu_closed(Context *ctx, const uint32_t *words) {
    (void)words;
    ctx->r[0] = 0x101; /* the decide sound, the menu closed */
}

/* qol.c's TurnEnd::ACall override, for the Divine Pulse item: the menu
   closes as B closes it, and the host opens the rewind */
void pulse_menu_choose(Context *ctx) {
    pulse_words[PW_MENU_REQUEST]++;
    ctx->r[15] = 0x003787E8u;
    ctx->thumb = 0;
    ctx->budget -= 4;
    CALL_THEN(f_003787E8, pulse_menu_closed, 0);
}
