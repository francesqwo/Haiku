/* haiku_mod.h: the Haiku mod API, for native plugin mods (C, C++, Rust...).
 *
 * A plugin is a DLL in a mod's folder (plugin.dll, or the name mod.toml gives
 * as `plugin = "x.dll"`) that exports haiku_mod_init. Haiku calls it once,
 * when the game starts with the mod on, and the plugin registers what it
 * wants there:
 *
 *   - hooks before and after the game's functions, by address (the
 *     game's own; odd for a Thumb function). A hook
 *     sees the guest's registers: the arguments in r[0]..r[3] before, the
 *     result in r[0] after. A before hook that returns HAIKU_SKIP skips the
 *     function: it returns to its caller at once, with r[0] as the hook set
 *     it. After hooks run once the function has returned, even when it left
 *     for the game's threads on the way.
 *   - a callback once a frame, before the game's code runs.
 *   - game memory, read and written from inside hooks and frame callbacks.
 *   - options: declared in mod.toml (`[[options]] id, label, default, min,
 *     max`), set on the Mods screen, read here with option().
 *
 * Plugins run natively with full trust, like any DLL. They are built for a
 * version of the game (Fates USA Special Edition, v2): function addresses
 * belong to it. Hooks can't go on functions Haiku has its own version of
 * (its overrides); those are refused with a note on the Mods screen.
 *
 * Build with any C compiler for Windows x64, e.g.
 *   gcc -shared -O2 -s -o plugin.dll my_mod.c
 *   zig cc -shared -O2 -target x86_64-windows-gnu -o plugin.dll my_mod.c
 */

#ifndef HAIKU_MOD_H
#define HAIKU_MOD_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define HAIKU_MOD_API_VERSION 1

#ifdef _WIN32
#define HAIKU_MOD_EXPORT __declspec(dllexport)
#else
#define HAIKU_MOD_EXPORT __attribute__((visibility("default")))
#endif

/* what a hook answers: go on, or (before hooks only) skip the function. */
#define HAIKU_CONTINUE 0
#define HAIKU_SKIP 1

/* the guest's registers: r[13] is sp, r[14] lr, r[15] pc. */
typedef struct HaikuRegs {
    uint32_t r[16];
} HaikuRegs;

/* the plugin, as Haiku knows it; pass it back where asked. */
typedef struct HaikuMod HaikuMod;

typedef int (*HaikuHook)(HaikuRegs *regs, void *user);
typedef void (*HaikuFrame)(void *user);

typedef struct HaikuModApi {
    uint32_t version; /* HAIKU_MOD_API_VERSION of the Haiku running it */
    uint32_t size;    /* sizeof(HaikuModApi) there */
    /* a line in Haiku's log, "mod <name>: <message>" */
    void (*log)(const HaikuMod *mod, const char *message);
    /* hooks, from haiku_mod_init only: how many hooks the address has now,
       -1 when it can't take one */
    int (*hook_before)(const HaikuMod *mod, uint32_t address, HaikuHook hook, void *user);
    int (*hook_after)(const HaikuMod *mod, uint32_t address, HaikuHook hook, void *user);
    /* a callback once a frame, from haiku_mod_init only */
    int (*on_frame)(const HaikuMod *mod, HaikuFrame frame, void *user);
    /* game memory, from hooks and frame callbacks: 1 done, 0 not now */
    int (*read)(uint32_t address, void *out, uint32_t size);
    int (*write)(uint32_t address, const void *data, uint32_t size);
    /* the option with id as the player set it, else its mod.toml default,
       else fallback */
    int64_t (*option)(const HaikuMod *mod, const char *id, int64_t fallback);
    /* the frame the game is on */
    uint64_t (*frame)(void);
} HaikuModApi;

/* the entry point a plugin exports: 0 when it started, anything else to be
   left out (its hooks with it). */
typedef int (*HaikuModInit)(const HaikuModApi *api, const HaikuMod *mod);

/* plugins export the version they were built for, so that an older Haiku
   turns down a plugin it can't run:
       HAIKU_MOD_DECLARE;
       HAIKU_MOD_EXPORT int haiku_mod_init(const HaikuModApi *api, const HaikuMod *mod) { ... } */
#define HAIKU_MOD_DECLARE HAIKU_MOD_EXPORT const uint32_t haiku_mod_api_version = HAIKU_MOD_API_VERSION

/* small helpers over read and write */
static inline uint32_t haiku_read32(const HaikuModApi *api, uint32_t address) {
    uint32_t value = 0;
    api->read(address, &value, 4);
    return value;
}
static inline uint16_t haiku_read16(const HaikuModApi *api, uint32_t address) {
    uint16_t value = 0;
    api->read(address, &value, 2);
    return value;
}
static inline uint8_t haiku_read8(const HaikuModApi *api, uint32_t address) {
    uint8_t value = 0;
    api->read(address, &value, 1);
    return value;
}
static inline void haiku_write32(const HaikuModApi *api, uint32_t address, uint32_t value) { api->write(address, &value, 4); }
static inline void haiku_write16(const HaikuModApi *api, uint32_t address, uint16_t value) { api->write(address, &value, 2); }
static inline void haiku_write8(const HaikuModApi *api, uint32_t address, uint8_t value) { api->write(address, &value, 1); }

#ifdef __cplusplus
}
#endif

#endif
