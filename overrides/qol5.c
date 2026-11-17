/* qol5: the map's menu fits on the screen.
 *
 * map::CreateSystemMenu builds the map's menu (Units, Guide, Options,
 * Bookmark or Save, Surrender, Delegate, End Turn; each mode shows some), a
 * game::menu::SplitLargeMenu of large buttons. qol4 added Return to Title
 * (qol5b: only that; Exit to Desktop left the map's menu at the user's
 * word) before End Turn.
 *
 * The game lays the menu out from its rows: each is the large button's
 * height (SplitWindow::GetLargeWindowItemH, 32 lines), BasicMenu::PoseH
 * sums the rows shown into the menu's height (+0x6C), PoseXY centres it on
 * the 240-line screen and SystemMenu::AfterBuild lifts it 16 lines, above
 * the help bar: its middle is line 104, the help bar starts at line 208. So
 * 6 rows (192 lines, lines 8 to 200) fit; 7 or more run off the top and
 * under the help bar. The button art is drawn whole at that height
 * (SplitWindow::DrawLargeWindow tiles 32-line pieces) and the 2D is drawn
 * natively from the game's prim states, so the rows can't simply be drawn
 * slimmer or scaled (both were tried: slimmer rows overlap the art; a
 * scaled world matrix on the menu's prim states is not taken up).
 *
 * So a longer menu is done the way the game does any menu longer than its
 * room: the BasicMenu's limit on the rows shown (+0x90, read by
 * BasicMenu::Build: the rows shown are cut to it and the rest scroll, with
 * the game's own scroll bar) is ROWS_SHOWN. Build applies it only when more
 * rows are shown than that, so a menu of 6 rows or fewer (the game's own,
 * and My Castle's with Return to Title) is laid out exactly as the game
 * lays it out; a longer one (a battle map's with Delegate, Surrender...)
 * shows 6 and scrolls, the cursor, its wrap, the "Left N" box and the help
 * line all the game's own.
 *
 * FATES_QOL5_MENU=0 leaves the menu as it is; FATES_QOL5_MENU_LOG=1 prints
 * the limit when it is set. */
#include <stdio.h>
#include <stdlib.h>
#include "overrides.h"

#define SYSTEM_MENU_VTABLE 0x0063156Cu /* map::(anon)::SystemMenu */
#define MENU_ITEM_COUNT    0x40u       /* the items given (BasicMenu::Create) */
#define MENU_SHOWN_LIMIT   0x90u       /* the most rows shown at once, <= 0: all */
#define ROWS_SHOWN         6

extern uint32_t qol_map_items; /* qol4 (overrides/qol.c): Return to Title and Exit to Desktop are in the menu */

static int qol5_menu_mode = -1;

/* BasicMenu::Build(void) */
RECOMP_OVERRIDE(0x004F9A90) {
    uint32_t menu = ctx->r[0];
    if (menu && mem_read32(ctx, menu) == SYSTEM_MENU_VTABLE && *(volatile uint32_t *)&qol_map_items) {
        if (qol5_menu_mode < 0) {
            const char *v = getenv("FATES_QOL5_MENU");
            const char *l = getenv("FATES_QOL5_MENU_LOG");
            qol5_menu_mode = v && v[0] == '0' ? 0 : (l && l[0] == '1' ? 2 : 1);
        }
        /* qol5b: any map menu with Haiku's row in it; the limit only acts
           when more than ROWS_SHOWN rows are shown (the items given include
           ones the mode hides) */
        int items = (int)mem_read32(ctx, menu + MENU_ITEM_COUNT);
        int limit = (int)mem_read32(ctx, menu + MENU_SHOWN_LIMIT);
        if (qol5_menu_mode && limit <= 0) {
            mem_write32(ctx, menu + MENU_SHOWN_LIMIT, ROWS_SHOWN);
            if (qol5_menu_mode == 2) fprintf(stderr, "[qol5] map menu: %d items given, at most %d rows shown at once\n", items, ROWS_SHOWN);
        }
    }
    CALL(RECOMP_ORIGINAL(0x004F9A90));
}
