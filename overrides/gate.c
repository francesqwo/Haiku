/* gate: the Dragon's Gate without the Nintendo eShop.
 *
 * All the DLC is installed on PC and the eShop cannot be reached, so the
 * Gate keeps only what plays what is installed:
 *
 * - Anna's menu (anonymous_namespace::ProcContentsMenu::Menu, 0x003C7B74)
 *   builds Play DLC, Purchase DLC and Exit. Purchase DLC opens the eShop
 *   catalogue (link::ec::purchase) and is hidden.
 * - The DLC list (link::ec::play::ContentsMenu::CreateBind, 0x003F39D0)
 *   lists every content the add-on title holds. The DLC title ships empty
 *   placeholder contents for maps that were never sold, all named
 *   "Reserved" in its ContentInfoArchive (every language); a 3DS never
 *   lists them because no ticket owns them. They are hidden too.
 * - Title screen > Extras > DLC (title::DownLoadItem, MainSequence's
 *   ContentsManagement, link::ec::management) only manages add-on content
 *   on the SD card through the eShop: Delete, Download Separately and
 *   Redownload All. Nothing there applies to the installed DLC on PC (and
 *   Delete would only take it away), so the Extras entry is hidden; Extras
 *   keeps Wireless Battle, SpotPass, Presents and Wipe Data.
 *
 * All three menus build their items with BasicMenuItem::BuildAttribute, the
 * shared default that answers 1 (enabled). BasicMenu::Build (0x004F9A90)
 * leaves out any item whose attribute has bit 4 (the game's own "hidden"
 * attribute) from the visible list, cursor and scroll bar; the item object
 * stays in the menu and is freed with it. So the override answers 4 for
 * those two kinds of item and 1 for everything else, exactly as before.
 * No guest calls, no drawing: the menus look as the game draws them.
 */
#include "overrides.h"

/* vtables, from the item constructors inlined in the two menu builders */
#define GATE_PURCHASE_ITEM_VTABLE 0x00629984u /* Menu::PurchaseMenuItem */
#define DLC_PLAY_ITEM_VTABLE      0x0062AABCu /* link::ec::play Menu::MenuItem */
#define DLC_PLAY_ITEM_NAME        0x38u       /* wchar_t[0x40], GetName() */
#define EXTRAS_DLC_ITEM_VTABLE    0x0069E654u /* title::DownLoadItem (ProcTitle::CreateExtraMenu) */

#define ATTRIBUTE_ENABLED 1u
#define ATTRIBUTE_HIDDEN  4u

static int gate_is_placeholder(Context *ctx, uint32_t item) {
    static const char reserved[] = "Reserved";
    uint32_t name = item + DLC_PLAY_ITEM_NAME;
    for (uint32_t i = 0; i < sizeof reserved; i++) {
        uint16_t c = mem_read16(ctx, name + 2 * i);
        if (c != (uint16_t)(unsigned char)reserved[i])
            return 0;
    }
    return 1;
}

/* amiibo: 2 (greyed out) for a cleared Hero Battle, else 0 (overrides/amiibo.c) */
uint32_t amiibo_item_attribute(Context *ctx, uint32_t item);

/* BasicMenuItem::BuildAttribute(void) */
RECOMP_OVERRIDE(0x001C68C0) {
    uint32_t item = ctx->r[0];
    uint32_t vtable = mem_read32(ctx, item);
    uint32_t attribute = ATTRIBUTE_ENABLED;
    if (vtable == GATE_PURCHASE_ITEM_VTABLE || vtable == EXTRAS_DLC_ITEM_VTABLE)
        attribute = ATTRIBUTE_HIDDEN;
    else if (vtable == DLC_PLAY_ITEM_VTABLE && gate_is_placeholder(ctx, item))
        attribute = ATTRIBUTE_HIDDEN;
    else if (vtable == DLC_PLAY_ITEM_VTABLE && amiibo_item_attribute(ctx, item)) /* amiibo: */
        attribute = amiibo_item_attribute(ctx, item);
    ctx->r[0] = attribute;
    /* what the original costs (mov r0, #1; bx lr), so timing is unchanged */
    ctx->budget -= 2;
    RETURN_TO(ctx->r[14]);
}
