/* mods: a "Mods" entry in the title screen's Main Menu, which opens Haiku's
 * Mods screen (the options menu's Mods page, drawn by fates_ui).
 *
 * The Main Menu (title::ProcTitle::CreateTopMenu, 0x00440F7C) is built from
 * title::TopMenuItem objects: Continue, Copy, New Game, Delete, Branch of
 * Fate and Extras with a save; New Game and Extras without one. On a game
 * that does not own all three paths the game adds a seventh (third) item,
 * "Explore Fates" (title::TopMenuPurchaseItem, vtable 0x0069F11C), and
 * title::TopMenu::PoseItem (0x0043F390) has its own layout for 7 and for 3
 * items. The Special Edition owns every path, so that item and its layout
 * are never used. The Mods entry takes its place: it is a
 * TopMenuPurchaseItem whose name is "Mods", laid out, drawn, selected and
 * cancelled by the game's own code exactly as that item would be, in the
 * game's font.
 *
 * - BasicMenu::Create (0x004FA880), called by CreateTopMenu with the
 *   null-terminated item list (GameMenuItemListTemplate<256>, room for 256),
 *   is where the item is added: operator new (0x002FDC68), the item filled
 *   in as TopMenuItem's constructor fills it, then appended to the list.
 *   operator new is a guest call that can leave for the host (the budget,
 *   a blocked thread); its return comes back to 0x004FA880 and the override
 *   carries on from a per-thread step record, as snd_native.c's sequences do.
 * - TopMenuPurchaseItem::ACall (0x0043E068): for the Mods item, tells the
 *   host (haiku_mods_hook) and answers 0x100, the decide sound with the menu
 *   kept, as TopMenuStartItem::ACall does when it opens a dialog.
 * - TopMenuPurchaseItem::OnSelect (0x0043E0CC): for the Mods item, the help
 *   line at the bottom, through FrameManager::Layout::SetItem (0x001B2B24)
 *   as every top-menu item does; the text lives in the item.
 *
 * Any other TopMenuPurchaseItem runs the game's own code. The entry is only
 * added when the host set haiku_mods_hook (Haiku.exe does).
 *
 * qol3: an "Exit to Desktop" entry after it, made the same way (a second
 * TopMenuPurchaseItem, marked "EXIT"), when the host set qol_title_exit.
 * Its A tells the host (qol_title_exit_request), which asks the player
 * with the options' own Exit to Desktop question. With both entries the
 * menu has 8 items (4 without a save), a count PoseItem has no layout for
 * (its fallback poses three), so PoseItem (0x0043F390) is overridden for
 * those two counts with the layout of the game's own even counts (6 and 2):
 * two columns at x 0x6C and 0xF6, rows 0x22 apart, the items' sizes 1 and
 * 0 by column; 8 has rows from 0x1D to 0x83 (the 7-item layout's span), 4
 * rows 0x3F and 0x61 (centred on the 2-item row, 0x50). An even count is
 * moved through by BasicMenu's own two-column grid (TopMenu::MoveUp and
 * MoveDown pass even counts to it), as the 6-item menu is.
 *
 * qol4: an "Options" entry between Mods and Exit to Desktop (a third
 * TopMenuPurchaseItem, marked "OPTS"), when the host set qol_title_options.
 * Its A tells the host (qol_title_options_request), which opens Haiku's
 * options menu over the Main Menu. With all three the menu has 9 items (5
 * without a save). The game's odd layouts (7 and 3) stagger every item down
 * one half row, which would reshuffle the whole menu; so the 9 and 5 keep
 * the game's two columns at x 0x6C and 0xF6 with Mods | Options as the last
 * pair, and Exit to Desktop alone on a last row, centred between the
 * columns (x 0xB1). 9 has rows 0x1D apart from 0x16 to 0x8A (the 8-item
 * span grown by 7 above and below, inside the panel), 5 the 6-item rows
 * 0x2E, 0x50, 0x72. The odd counts' own moves (TopMenu::MoveUp, MoveDown,
 * KeyLeft, KeyRight) are written for the staggered layout, so for these
 * two counts the moves are done here, through the game's
 * BasicMenu::SetSelectFromIndex: up and down by rows, wrapping top to
 * bottom; left and right within a pair; the Exit row remembers the column
 * it was entered from (in TopMenu's own +0x13C, the odd layouts' "came
 * from" word, unused otherwise here).
 */
#include "overrides.h"

typedef void (*HaikuModsHook)(Context *ctx, uint32_t what);
/* set by Haiku's main before the game's code runs */
HaikuModsHook haiku_mods_hook = 0;

enum { MODS_OPEN_SCREEN = 1, MODS_ENTRY_ADDED = 2 };

#define CREATE_TOP_MENU_RETURN 0x004414E8u /* after bl BasicMenu::Create in CreateTopMenu */
#define OPERATOR_NEW           0x002FDC68u /* operator new(unsigned int) */
#define PURCHASE_ITEM_VTABLE   0x0069F11Cu /* title::TopMenuPurchaseItem */
#define ONSELECT_LAYOUT        0x0043E110u /* OnSelect's literal: &FrameManager's layout pointer */
#define LAYOUT_ITEM            0x38u       /* the layout's help line (Layout::SetItem's this) */

/* the item: TopMenuItem's 0x58 bytes (name at +0x38, wchar_t[16]), then the
   mark and the help line */
#define ITEM_NAME   0x38u
#define ITEM_MARK   0x58u
#define ITEM_HELP   0x5Cu
#define ITEM_SIZE   0xC0u
#define MODS_MARK   0x53444F4Du /* "MODS" */
#define LIST_ROOM   256u

static const char mods_name[] = "Mods";
static const char mods_help[] = "Turn mods on or off and choose their order.";

/* qol3: the Exit to Desktop entry (overrides/qol.c's words) */
#define EXIT_MARK   0x54495845u /* "EXIT" */
extern uint32_t qol_title_exit;
extern uint32_t qol_title_exit_request;
static const char exit_name[] = "Exit to Desktop"; /* 15 and the end: the name's room is 16 */
static const char exit_help[] = "Close the game and return to the desktop.";
enum { ADD_MODS = 1, ADD_EXIT = 2, ADD_OPTIONS = 4 }; /* qol4: ADD_OPTIONS */

/* qol4: the Options entry (overrides/qol.c's words) */
#define OPTIONS_MARK 0x5354504Fu /* "OPTS" */
extern uint32_t qol_title_options;
extern uint32_t qol_title_options_request;
static const char options_name[] = "Options";
static const char options_help[] = "Change the display, sound and controls.";

static int is_mods_item(Context *ctx, uint32_t item) {
    return item && mem_read32(ctx, item) == PURCHASE_ITEM_VTABLE && mem_read32(ctx, item + ITEM_MARK) == MODS_MARK;
}

static int is_exit_item(Context *ctx, uint32_t item) {
    return item && mem_read32(ctx, item) == PURCHASE_ITEM_VTABLE && mem_read32(ctx, item + ITEM_MARK) == EXIT_MARK;
}

static int is_options_item(Context *ctx, uint32_t item) { /* qol4 */
    return item && mem_read32(ctx, item) == PURCHASE_ITEM_VTABLE && mem_read32(ctx, item + ITEM_MARK) == OPTIONS_MARK;
}

/* eur2: the entries' texts in the EUR game's other languages (Lang::
   Folder's index: 2 French, 3 German, 4 Italian, 5 Spanish; the USA game
   is always 1, English), by the English text; Options as the game's own
   menus name it. UTF-8 here; a help line holds 49 characters. */
#define MODS_LANG_INDEX 0x006BDA6Cu
static const char *const mods_texts[][5] = {
    /* English, French, German, Italian, Spanish */
    {"Turn mods on or off and choose their order.", "Activer, désactiver et ordonner les mods.",
     "Mods ein- oder ausschalten, Reihenfolge wählen.", "Attiva o disattiva le mod e scegline l'ordine.", "Activa o desactiva mods y elige su orden."},
    {"Exit to Desktop", "Quitter le jeu", "Spiel beenden", "Esci dal gioco", "Salir del juego"},
    {"Close the game and return to the desktop.", "Fermer le jeu et revenir au bureau.", "Das Spiel schließen und zum Desktop.",
     "Chiudi il gioco e torna al desktop.", "Cerrar el juego y volver al escritorio."},
    {"Options", "Options", "Optionen", "Opzioni", "Opciones"},
    {"Change the display, sound and controls.", "Régler l'affichage, le son et les commandes.", "Bild, Ton und Steuerung einstellen.",
     "Regola immagine, audio e comandi.", "Ajusta la imagen, el sonido y los controles."},
};

int haiku_code_is_jpn(void);                 /* jpprep: eur_region.c */
const char *haiku_text_ja(const char *english); /* jpprep: jpn_text.c */
uint32_t haiku_utf8_next(const unsigned char **text);

static void put_text(Context *ctx, uint32_t at, const char *text, uint32_t room) {
    /* jpprep: the Japanese game's, which has no Lang::Folder */
    const int jpn = haiku_code_is_jpn();
    if (jpn && haiku_text_ja(text))
        text = haiku_text_ja(text);
    /* eur2: the language's own, when there is one */
    const uint32_t lang = jpn ? 0 : mem_read8(ctx, MODS_LANG_INDEX);
    if (lang >= 2 && lang <= 5) {
        for (uint32_t k = 0; k < sizeof mods_texts / sizeof mods_texts[0]; k++) {
            const char *a = mods_texts[k][0], *b = text;
            while (*a && *a == *b) {
                a++;
                b++;
            }
            if (*a == 0 && *b == 0) {
                text = mods_texts[k][lang - 1];
                break;
            }
        }
    }
    const unsigned char *p = (const unsigned char *)text;
    uint32_t i = 0;
    while (*p && i + 1 < room) {
        /* eur2: two-byte UTF-8 (é, ü...); jpprep: three (kana, kanji) */
        const uint32_t c = haiku_utf8_next(&p);
        mem_write16(ctx, at + 2 * i++, (uint16_t)c);
    }
    mem_write16(ctx, at + 2 * i, 0);
}

/* the step a thread is at in adding the item: which list, and where
   BasicMenu::Create returns to */
typedef struct {
    uint32_t tls, lr, menu, list;
    uint32_t kinds, kind; /* qol3: the entries still to add, the one being made */
} ModsAdd;
static ModsAdd mods_adds[4];

static ModsAdd *mods_add_find(Context *ctx) {
    for (unsigned i = 0; i < sizeof mods_adds / sizeof *mods_adds; i++)
        if (mods_adds[i].lr && mods_adds[i].tls == ctx->tls)
            return &mods_adds[i];
    return 0;
}

static ModsAdd *mods_add_new(Context *ctx) {
    for (unsigned i = 0; i < sizeof mods_adds / sizeof *mods_adds; i++)
        if (!mods_adds[i].lr) {
            mods_adds[i].tls = ctx->tls;
            return &mods_adds[i];
        }
    return 0;
}

/* amiibo: the Hero Battles in the Dragon's Gate list (overrides/amiibo.c) */
int amiibo_menu_create(Context *ctx);
int qol_menu_create(Context *ctx); /* qol */

/* operator new(ITEM_SIZE) for the next entry to add, returning into
   BasicMenu::Create (qol3: one entry at a time, Mods then Exit) */
static void mods_add_alloc(Context *ctx, ModsAdd *add) {
    add->kind = (add->kinds & ADD_MODS) ? ADD_MODS : (add->kinds & ADD_OPTIONS) ? ADD_OPTIONS : ADD_EXIT; /* qol4: Options between */
    add->kinds &= ~add->kind;
    ctx->r[0] = ITEM_SIZE;
    ctx->r[14] = 0x004FA880u;
    ctx->r[15] = OPERATOR_NEW;
    ctx->thumb = 0;
    CALL(recomp_call);
}

/* BasicMenu::Create(BasicMenuItem**) */
RECOMP_OVERRIDE(0x004FA880) {
    if (amiibo_menu_create(ctx)) /* amiibo: */
        return;
    if (qol_menu_create(ctx)) /* qol: My Castle's Return to Title and Exit to Desktop */
        return;
    ModsAdd *add = mods_add_find(ctx);
    if (!add) {
        uint32_t kinds = 0;
        if (ctx->r[14] == CREATE_TOP_MENU_RETURN) {
            if (haiku_mods_hook)
                kinds |= ADD_MODS;
            if (*(volatile uint32_t *)&qol_title_exit) /* qol3 */
                kinds |= ADD_EXIT;
            if (*(volatile uint32_t *)&qol_title_options) /* qol4 */
                kinds |= ADD_OPTIONS;
        }
        if (!kinds || !(add = mods_add_new(ctx))) {
            CALL(RECOMP_ORIGINAL(0x004FA880));
            return;
        }
        add->lr = ctx->r[14];
        add->menu = ctx->r[0];
        add->list = ctx->r[1];
        add->kinds = kinds;
        mods_add_alloc(ctx, add);
        return;
    }
    /* back from operator new, the item in r0 */
    uint32_t item = ctx->r[0];
    uint32_t menu = add->menu, list = add->list, lr = add->lr;
    if (item) {
        int exit_item = add->kind == ADD_EXIT;
        int options_item = add->kind == ADD_OPTIONS; /* qol4 */
        for (uint32_t at = 0; at < ITEM_SIZE; at += 4)
            mem_write32(ctx, item + at, 0);
        mem_write32(ctx, item, PURCHASE_ITEM_VTABLE);
        put_text(ctx, item + ITEM_NAME, options_item ? options_name : exit_item ? exit_name : mods_name, 16);
        mem_write32(ctx, item + ITEM_MARK, options_item ? OPTIONS_MARK : exit_item ? EXIT_MARK : MODS_MARK);
        put_text(ctx, item + ITEM_HELP, options_item ? options_help : exit_item ? exit_help : mods_help, (ITEM_SIZE - ITEM_HELP) / 2);
        uint32_t count = 0;
        while (count + 1 < LIST_ROOM && mem_read32(ctx, list + 4 * count))
            count++;
        if (count + 1 < LIST_ROOM) {
            mem_write32(ctx, list + 4 * count, item);
            mem_write32(ctx, list + 4 * (count + 1), 0);
            if (!exit_item && !options_item)
                haiku_mods_hook(ctx, MODS_ENTRY_ADDED);
        }
    }
    if (add->kinds) { /* qol3: the next entry */
        mods_add_alloc(ctx, add);
        return;
    }
    add->lr = 0;
    ctx->r[0] = menu;
    ctx->r[1] = list;
    ctx->r[14] = lr;
    ctx->r[15] = 0x004FA880u;
    ctx->thumb = 0;
    CALL(RECOMP_ORIGINAL(0x004FA880));
}

/* title::TopMenuPurchaseItem::ACall(void) */
RECOMP_OVERRIDE(0x0043E068) {
    /* qol3: Exit to Desktop: the host asks, the menu stays */
    if (is_exit_item(ctx, ctx->r[0])) {
        qol_title_exit_request = 1;
        ctx->r[0] = 0x100;
        ctx->budget -= 4;
        RETURN_TO(ctx->r[14]);
    }
    /* qol4: Options: the host opens its options menu, the menu stays */
    if (is_options_item(ctx, ctx->r[0])) {
        qol_title_options_request = 1;
        ctx->r[0] = 0x100;
        ctx->budget -= 4;
        RETURN_TO(ctx->r[14]);
    }
    if (!is_mods_item(ctx, ctx->r[0]) || !haiku_mods_hook) {
        CALL(RECOMP_ORIGINAL(0x0043E068));
        return;
    }
    haiku_mods_hook(ctx, MODS_OPEN_SCREEN);
    /* the decide sound, and the menu stays as it is */
    ctx->r[0] = 0x100;
    ctx->budget -= 4;
    RETURN_TO(ctx->r[14]);
}

/* title::TopMenuPurchaseItem::OnSelect(void) */
RECOMP_OVERRIDE(0x0043E0CC) {
    uint32_t item = ctx->r[0];
    if (!is_mods_item(ctx, item) && !is_exit_item(ctx, item) && !is_options_item(ctx, item)) { /* qol3: and Exit to Desktop's; qol4: and Options' */
        CALL(RECOMP_ORIGINAL(0x0043E0CC));
        return;
    }
    /* FrameManager::Layout::SetItem(layout + 0x38, help), a tail call */
    uint32_t layout = mem_read32(ctx, mem_read32(ctx, ONSELECT_LAYOUT));
    ctx->r[0] = layout + LAYOUT_ITEM;
    ctx->r[1] = item + ITEM_HELP;
    ctx->r[15] = 0x001B2B24u;
    ctx->thumb = 0;
    ctx->budget -= 4;
    /* SetItem is an entry the generated code has inside a larger function,
       so it is run through the host's table */
    CALL(recomp_call);
}

/* qol3: title::TopMenu::PoseItem(void) for 8 and 4 items (see the top).
   Every top-menu item's width is BaseMenuItem::GetWidth (0x0053D3B0: size
   1 is 0x40, else 0x48) and its height BaseMenuItem::BuildH (0x00432E6C:
   0x18); a menu with an item of another kind is left to the game. Item
   fields as PoseItem writes them: +0x0C the menu, +0x10/+0x14 the place on
   screen (the menu's +0x58/+0x5C plus +0x18/+0x1C), +0x20 the width, +0x24
   the height. */
#define POSE_GET_WIDTH 0x0053D3B0u
#define POSE_BUILD_H   0x00432E6Cu
RECOMP_OVERRIDE(0x0043F390) {
    uint32_t menu = ctx->r[0];
    uint32_t count = mem_read32(ctx, menu + 0x38);
    uint32_t items = mem_read32(ctx, menu + 0x34);
    int ours = (count == 8 || count == 4 || count == 9 || count == 5) && items; /* qol4: 9 and 5 */
    for (uint32_t i = 0; ours && i < count; i++) {
        uint32_t item = mem_read32(ctx, items + 4 * i);
        uint32_t vtable = item ? mem_read32(ctx, item) : 0;
        if (!vtable || (mem_read32(ctx, vtable + 0x60) & ~1u) != POSE_GET_WIDTH || (mem_read32(ctx, vtable + 0x14) & ~1u) != POSE_BUILD_H)
            ours = 0;
    }
    if (!ours) {
        CALL(RECOMP_ORIGINAL(0x0043F390));
        return;
    }
    /* qol4: 9 and 5: the pairs 0x1D apart from 0x16 (9) or the 6-item rows
       (5), and the last (Exit to Desktop) alone, centred between the columns */
    if ((count == 9 || count == 5) && !is_exit_item(ctx, mem_read32(ctx, items + 4 * (count - 1)))) {
        CALL(RECOMP_ORIGINAL(0x0043F390));
        return;
    }
    uint32_t top = count == 8 ? 0x1Du : count == 9 ? 0x16u : count == 5 ? 0x2Eu : 0x3Fu;
    uint32_t step = count == 9 ? 0x1Du : 0x22u;
    for (uint32_t i = 0; i < count; i++) {
        uint32_t item = mem_read32(ctx, items + 4 * i);
        int alone = (count & 1) && i == count - 1;
        uint32_t x = alone ? 0xB1u : (i & 1) ? 0xF6u : 0x6Cu;
        uint32_t y = top + step * (i / 2);
        uint32_t owner = mem_read32(ctx, item + 0x0C);
        mem_write32(ctx, item + 0x20, ((i & 1) || alone) ? 0x48u : 0x40u);
        mem_write32(ctx, item + 0x18, x);
        mem_write32(ctx, item + 0x1C, y);
        mem_write32(ctx, item + 0x24, 0x18u);
        mem_write32(ctx, item + 0x10, mem_read32(ctx, owner + 0x58) + x);
        mem_write32(ctx, item + 0x14, mem_read32(ctx, owner + 0x5C) + y);
    }
    ctx->budget -= 16 + 24 * count;
    RETURN_TO(ctx->r[14]);
}

/* ---- qol4: the moves in the 9- and 5-item Main Menu -------------------- */

/* this menu is ours: 9 or 5 items, the last our Exit to Desktop */
static int qol4_grid(Context *ctx, uint32_t menu) {
    uint32_t count = mem_read32(ctx, menu + 0x38);
    uint32_t items = mem_read32(ctx, menu + 0x34);
    return (count == 9 || count == 5) && items && is_exit_item(ctx, mem_read32(ctx, items + 4 * (count - 1)));
}

#define TOP_CAME_FROM 0x13Cu /* TopMenu's own word: the column the Exit row was entered from */
#define TOP_SELECTED  0x9Cu  /* BasicMenu's selected index */

/* BasicMenu::SetSelectFromIndex(menu, index), as a tail call: it returns to
   the move's caller, which sees the new index as after any move */
static void qol4_select(Context *ctx, uint32_t menu, uint32_t index) {
    ctx->r[0] = menu;
    ctx->r[1] = index;
    ctx->r[15] = 0x004F932Cu;
    ctx->thumb = 0;
    ctx->budget -= 12;
    CALL(recomp_call);
}

static uint32_t qol4_came_from(Context *ctx, uint32_t menu) {
    return mem_read32(ctx, menu + TOP_CAME_FROM) == 1 ? 1u : 0u;
}

/* one move: dir 0 up, 1 down, 2 left, 3 right; wrap as the game's moves
   take it (a held key's repeat does not wrap) */
static void qol4_move(Context *ctx, uint32_t menu, int dir, int wrap) {
    uint32_t count = mem_read32(ctx, menu + 0x38);
    uint32_t last = count - 1;
    uint32_t at = mem_read32(ctx, menu + TOP_SELECTED);
    uint32_t to = at;
    if (at > last) {
        to = 0;
    } else if (dir == 0) { /* up */
        if (at == last) {
            to = last - 2 + qol4_came_from(ctx, menu);
        } else if (at < 2) {
            if (wrap) {
                mem_write32(ctx, menu + TOP_CAME_FROM, at & 1);
                to = last;
            }
        } else {
            to = at - 2;
        }
    } else if (dir == 1) { /* down */
        if (at == last) {
            if (wrap)
                to = qol4_came_from(ctx, menu);
        } else if (at + 2 >= last) {
            mem_write32(ctx, menu + TOP_CAME_FROM, at & 1);
            to = last;
        } else {
            to = at + 2;
        }
    } else if (at != last) { /* left and right, within a pair */
        int edge = dir == 2 ? !(at & 1) : (int)(at & 1);
        if (!edge || wrap)
            to = at ^ 1;
    }
    if (to != at) {
        qol4_select(ctx, menu, to);
        return;
    }
    ctx->budget -= 8;
    RETURN_TO(ctx->r[14]);
}

/* title::TopMenu::MoveUp(bool) */
RECOMP_OVERRIDE(0x004FA93C) {
    if (!qol4_grid(ctx, ctx->r[0])) {
        CALL(RECOMP_ORIGINAL(0x004FA93C));
        return;
    }
    qol4_move(ctx, ctx->r[0], 0, ctx->r[1] & 0xFF);
}

/* title::TopMenu::MoveDown(bool) */
RECOMP_OVERRIDE(0x004FB228) {
    if (!qol4_grid(ctx, ctx->r[0])) {
        CALL(RECOMP_ORIGINAL(0x004FB228));
        return;
    }
    qol4_move(ctx, ctx->r[0], 1, ctx->r[1] & 0xFF);
}

/* title::TopMenu::KeyLeft(bool) */
RECOMP_OVERRIDE(0x004FADD0) {
    if (!qol4_grid(ctx, ctx->r[0])) {
        CALL(RECOMP_ORIGINAL(0x004FADD0));
        return;
    }
    qol4_move(ctx, ctx->r[0], 2, ctx->r[1] & 0xFF);
}

/* title::TopMenu::KeyRight(bool) */
RECOMP_OVERRIDE(0x004FB048) {
    if (!qol4_grid(ctx, ctx->r[0])) {
        CALL(RECOMP_ORIGINAL(0x004FB048));
        return;
    }
    qol4_move(ctx, ctx->r[0], 3, ctx->r[1] & 0xFF);
}

/* pulsefix: what of this file is the console's state, kept with Divine
   Pulse's moments and saved states (overrides/keep.c) */
void mods_keep(void (*add)(const char *, void *, uint32_t)) {
    add("mods.mods_adds", &mods_adds, sizeof mods_adds);
}
