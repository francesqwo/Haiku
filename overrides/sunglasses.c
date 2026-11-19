/* sunglasses: the avatar creator's Sunglasses facial detail in saves.
 *
 * Corrin's looks are a unit::Edit; its facial detail is the byte at +0x29,
 * an index into face/FaceData_Acc1.bin (FaceAccessoryDataManager::
 * GetAccessory1Data(0, n) indexes the table with no bounds check). Haiku's
 * own RomFS layer (assets/builtin) adds entry 13, Sunglasses. The game on a
 * 3DS has entries 0 to 12 only, and would read past its table for 13 (a
 * garbage file name, likely a crash) if a save held it.
 *
 * So in the stream a save is made of, a unit::Edit with detail 13 is
 * written as detail 0 (Nothing) with bit 7 set in its version byte, and
 * read back as 13:
 *
 *   unit::Edit::Serialize (0x0053AC88) writes: version 4, name (0x1A),
 *   +0x1A, +0x1B, short +0x1E, +0x20..+0x23, +0x24..+0x27, +0x28, +0x29
 *   (stream offset 0x28), +0x2A..+0x2C, through Stream::WriteByte etc.
 *   (0x0044C3DC: buffer at stream +4, position at +0x10).
 *   unit::Edit::Deserialize (0x00419474) reads the version byte and only
 *   asks "0?" (old layout) and "< 2 / 3 / 4?" (fields added later), all
 *   unsigned: 0x84 reads exactly like 4.
 *
 * The 3DS game reading such a save sees version 0x84 = a current edit, and
 * Corrin with no facial detail. Every other detail, and a save without
 * sunglasses, is written byte for byte as before.
 *
 * Both overrides call the original and fix the bytes in a tail (CALL_THEN):
 * the tails only touch memory.
 */
#include <stdint.h>
#include "overrides.h"

#define EDIT_DETAIL 0x29
#define DETAIL_SUNGLASSES 13
#define STREAM_DETAIL 0x28        /* the detail's offset in the stream */
#define VERSION_SUNGLASSES 0x80   /* bit 7 of the version byte */
/* tones: Corrin's speaking style (tones.c), the edit's byte +0x2D (0 to 2,
   padding on the US build), in bits 4 and 5 of the version byte: 0x14,
   0x24 (and 0x94, 0xA4 with sunglasses) read on a 3DS exactly like 4, and
   style 0 (the US game's only one) writes the same bytes as before */
#define EDIT_TONE 0x2D
#define VERSION_TONE_SHIFT 4
#define VERSION_TONE_MASK 0x30

int haiku_code_is_jpn(void); /* jpn: eur_region.c; no Haiku style there (tones.c) */

/* words: the stream's buffer address at the edit's first byte, the edit */
static void serialize_end(Context *ctx, const uint32_t *w) {
    uint32_t at = w[0];
    if (mem_read8(ctx, at + STREAM_DETAIL) == DETAIL_SUNGLASSES) {
        mem_write8(ctx, at + STREAM_DETAIL, 0);
        mem_write8(ctx, at, mem_read8(ctx, at) | VERSION_SUNGLASSES);
    }
    uint8_t tone = mem_read8(ctx, w[1] + EDIT_TONE); /* tones */
    if ((tone == 1 || tone == 2) && !haiku_code_is_jpn())
        mem_write8(ctx, at, mem_read8(ctx, at) | (uint8_t)(tone << VERSION_TONE_SHIFT));
}

/* unit::Edit::Serialize(Stream*) const */
RECOMP_OVERRIDE(0x0053AC88) {
    uint32_t edit = ctx->r[0], stream = ctx->r[1];
    uint32_t at = mem_read32(ctx, stream + 4) + mem_read32(ctx, stream + 0x10);
    CALL_THEN(RECOMP_ORIGINAL(0x0053AC88), serialize_end, at, edit);
}

/* words: the edit, the version byte read */
static void deserialize_end(Context *ctx, const uint32_t *w) {
    if ((w[1] & VERSION_SUNGLASSES) && mem_read8(ctx, w[0] + EDIT_DETAIL) == 0)
        mem_write8(ctx, w[0] + EDIT_DETAIL, DETAIL_SUNGLASSES);
    /* tones: the style back (0 when the save has none) */
    uint8_t tone = (uint8_t)((w[1] & VERSION_TONE_MASK) >> VERSION_TONE_SHIFT);
    if (tone > 2 || haiku_code_is_jpn())
        tone = 0;
    if (mem_read8(ctx, w[0] + EDIT_TONE) != tone)
        mem_write8(ctx, w[0] + EDIT_TONE, tone);
}

/* unit::Edit::Deserialize(Stream*) */
RECOMP_OVERRIDE(0x00419474) {
    uint32_t edit = ctx->r[0], stream = ctx->r[1];
    uint32_t version = mem_read8(ctx, mem_read32(ctx, stream + 4) + mem_read32(ctx, stream + 0x10));
    CALL_THEN(RECOMP_ORIGINAL(0x00419474), deserialize_end, edit, version);
}
