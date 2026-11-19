/* jpprep: Haiku's texts in the game's own menus, in Japanese, for the
 * Japanese build (Fire Emblem if, zakuro_core::region JPN).
 *
 * The Japanese game has no Lang::Folder: it is Japanese only. The overrides
 * that put Haiku's entries in the game's menus (qol.c, mods.c, online.c,
 * offline_msg.c) ask haiku_code_is_jpn() first (eur_region.c's title, which
 * dev/region/translate.py writes for each build) and take the Japanese text
 * here by the English one; on the USA and EUR builds nothing here is used.
 *
 * The words follow the Japanese game's own terms (its map menu's ターン終了
 * pattern, 設定 for settings, its Yes/No questions); Divine Pulse is
 * 天刻の拍動, as Fire Emblem: Three Houses names it in Japanese. UTF-8 here;
 * haiku_utf8_next reads one code point (up to three bytes: kana and kanji). */
#include "overrides.h"

static const char *const ja_texts[][2] = {
    /* qol.c: My Castle's and the map's system menus */
    {"Return to Title", "タイトルに戻る"},
    {"Return to the title screen.", "タイトル画面に戻ります。"},
    {"Return to the title screen?\nAnything you haven't saved will be lost.",
     "タイトル画面に戻りますか？\nセーブしていない進行状況は失われます。"},
    {"Exit to Desktop", "ゲームを終了"},
    {"Close the game and return to the desktop.", "ゲームを終了してデスクトップに戻ります。"},
    {"Exit to the desktop?\nAnything you haven't saved will be lost.",
     "ゲームを終了しますか？\nセーブしていない進行状況は失われます。"},
    {"Divine Pulse", "天刻の拍動"},
    {"Turn back time to undo actions on this map.", "時を巻き戻して、このマップでの行動をやり直します。"},
    /* mods.c: the title screen's entries */
    {"Mods", "MOD"},
    {"Turn mods on or off and choose their order.", "MODの使用と読み込む順番を設定します。"},
    {"Options", "PC設定"},
    {"Change the display, sound and controls.", "画面、サウンド、操作の設定を変更します。"},
    /* online.c */
    {"Online Battle", "オンライン対戦"},
    {"Battle a friend online with a code.", "コードを使ってフレンドとオンライン対戦します。"},
    {"Host", "部屋を作る"},
    {"Get a code to send to your opponent.", "対戦相手に伝えるコードを発行します。"},
    {"Join", "部屋に入る"},
    {"Enter the code your opponent sent.", "対戦相手のコードを入力します。"},
};

/* jpn2: 1 while an English translation mod is on (Team If, the Uncensored
   Patch): the host sets it when it lays the mods' text
   (zakuro::jpn_english); Haiku's entries are English then, as the game's */
RECOMP_EXPORT volatile uint32_t haiku_jpn_english = 0;

int haiku_code_is_jpn(void); /* eur_region.c */

/* jpn2: whether Haiku's entries are Japanese: the Japanese game's script */
int haiku_jpn_japanese(void) {
    return haiku_code_is_jpn() && !haiku_jpn_english;
}

/* the Japanese text for one of Haiku's English menu texts, or 0 (also 0
   under an English translation: jpn2) */
const char *haiku_text_ja(const char *english) {
    if (haiku_jpn_english)
        return 0;
    for (unsigned k = 0; k < sizeof ja_texts / sizeof ja_texts[0]; k++) {
        const char *a = ja_texts[k][0], *b = english;
        while (*a && *a == *b) {
            a++;
            b++;
        }
        if (*a == 0 && *b == 0)
            return ja_texts[k][1];
    }
    return 0;
}

/* one UTF-8 code point (one, two or three bytes), the pointer moved past it */
uint32_t haiku_utf8_next(const unsigned char **text) {
    const unsigned char *p = *text;
    uint32_t c = *p++;
    if ((c & 0xE0) == 0xC0 && (p[0] & 0xC0) == 0x80) {
        c = (c & 0x1F) << 6 | (p[0] & 0x3F);
        p += 1;
    } else if ((c & 0xF0) == 0xE0 && (p[0] & 0xC0) == 0x80 && (p[1] & 0xC0) == 0x80) {
        c = (c & 0x0F) << 12 | (uint32_t)(p[0] & 0x3F) << 6 | (p[1] & 0x3F);
        p += 2;
    }
    *text = p;
    return c;
}
