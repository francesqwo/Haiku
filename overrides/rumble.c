/* input: controller rumble where the 3DS has none.
 *
 * The battle camera's quake (big hits, criticals, heavy landings) also
 * rumbles the connected controllers through the port's input API,
 * fates_pc_rumble() in vendor/zakuro/crates/zakuro/src/input_api.rs, which
 * honours the pc.rumble setting. The game's own code still runs unchanged.
 */
#include <stdint.h>
#include "overrides.h"

extern void fates_pc_rumble(uint16_t low, uint16_t high, uint32_t ms);

/* BattleCamera::Quake(float, int) */
RECOMP_OVERRIDE(0x001A98FC) {
    fates_pc_rumble(42000, 26000, 220);
    CALL(RECOMP_ORIGINAL(0x001A98FC));
}
