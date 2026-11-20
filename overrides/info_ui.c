/* info-ui: enemy targeting lines and the unit growth-rate view.
 *
 * 1. Enemy Targeting Lines (pc.targeting_lines).
 *
 *    Which unit each enemy will attack next enemy phase is asked of the
 *    game's own AI, run on a copy of the game's state:
 *
 *    - The enemy phase runs AI::Processing (0x002238CC) on its AI thread
 *      (map::anonymous_namespace::AIThread::Function). Each call decides for
 *      one unit and leaves the decision in map::Mind (*0x006D7568): the unit
 *      (+0), where it moves (+4, +5), what it does (+6), the item (+8) and the
 *      unit it acts on (+9). The main thread then carries the decision out
 *      and asks for the next one. map::SequenceAI::Create makes the AI
 *      object (AI::AI, 0x00223D44) at the start of the phase, seeding the
 *      AI's own Random from the game's (0x00752B7C).
 *
 *    - info_predict, called from the player phase's free cursor
 *      (FreeCursorTick, 0x00351904) whenever the units changed, does the
 *      same on a copy-on-write view of the guest's memory: the phase byte
 *      (map::Situation +8) is set to the enemy force, an AI object is made
 *      with the game's operator new and AI::AI, and AI::Processing is called
 *      until it ends the phase (Mind +6 = 0x24). After each decision the
 *      unit is moved to the tile it chose, in the copy, as the game would
 *      move it before the next unit thinks (its battle is not fought).
 *
 *    - The copy: while info_predict runs, the context's page tables point
 *      at our own, whose write side is empty, so every store comes to our
 *      host functions, which copy the page first (4 KiB, at most
 *      COW_PAGES) and write the copy; the read side follows the copies.
 *      Nothing the AI writes reaches the game's memory: its allocations, its
 *      map images, its own Random, the units it trades items between
 *      (AI::Completion does that), the stack. The registers, the VFP
 *      registers and the budget are put back afterwards. An svc or an exit
 *      from the code during the prediction abandons it (counted).
 *      predict: so does the first access to nothing (a read of an unmapped
 *      page, a write the copy refuses), at once and without a host warning
 *      (cow_bad); and a guest call carries on from r15, as the host does,
 *      until the guest has really returned (gcall).
 *
 *    - The lines are drawn on the map, in its 3D space, with the game's own
 *      prim calls, not depth tested against the map (so walls and roofs
 *      never hide them), from map::Panel::Route::Draw (0x0038FC5C), so they
 *      sit with the movement and danger panels: under the units and every
 *      window. lines2: each is a thin arc of light from the enemy to its
 *      target (a parabola lifted in the middle), drawn procedurally as a
 *      ribbon facing the camera with the same width on screen at every
 *      zoom: a pale core, the attack panels' red (the colour table at
 *      0x00717648 +0x8C) tinted pink at its anti-aliased edge, a soft glow,
 *      no arrowhead and no texture. It fades in from the enemy, and a soft
 *      bright pulse runs along it to the target.
 *      They show only while the free cursor runs (FreeCursorTick this frame
 *      or the one before): not in the enemy phase, battles or menus.
 *    - Mode 2 shows only the lines of the enemy under the cursor, or those
 *      aimed at the unit under it.
 *    - Round 3: while a unit's destination is chosen (PickCursorTick,
 *      0x00352800) the phase is predicted with that unit standing on the
 *      hovered tile, cached per tile and one run a frame, so the lines
 *      follow it as it is moved (mode 2: those aimed at it).
 *
 * 2. Growth rates on the lower screen's unit status (pc.growth_button).
 *
 *    ViewerUnitStatus::Draw (0x001F580C) draws the unit card on the lower
 *    screen. Before it, the growth rates of its unit (this +8) are asked of
 *    the game's own Unit::GetGrow (0x005330B4, what Unit::LevelUp rolls
 *    against: personal + class + boon/bane, a child's parent, Aptitude) and
 *    left for the host, which draws the button and the growth window.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include "overrides.h"

/* pulse: overrides/pulse.c's hooks */
void pulse_free_cursor(Context *ctx);
void pulse_ai_decided(Context *ctx);
extern uint32_t pulse_words[];

#define IU_WORDS 512
/* the words the host reads and writes, by index:
 *  0 lines mode (0 off, 1 every enemy, 2 the cursor's)    host
 *  1 debug bits (1 predictions, 2 the real enemy phase)    host
 *  2 predictions run   3 abandoned   4 last one's microseconds
 *  5 lines            6 simulate moves (default 1)          host
 *  7 growth feature on                                      host
 *  8 the frame of the last free-cursor tick
 *  9 svcs seen during predictions  10 pages copied (last)
 * 11 the unit status draws (counts up)  12 its unit  13 the growth rates' unit
 * 14 the unit status' help box is up    15 the frame of the last move-choosing tick
 * 25, 26 the unit being moved and where it stands, for the lines shown
 * 27 the unit whose destination is being chosen
 * 28 a prediction is running now (vowperf: for the sampling profiler)
 * 29 the most instructions one AI call of a prediction ran (vowperf)
 * 30 predictions off (vowperf): 0 on, 1 for this map, 2 for the session
 * 31 the longest prediction run so far, microseconds (vowperf)
 * 420 guest calls that returned through other frames (predict)
 * 421 predictions given up at an access to nothing, 422 its address (predict)
 * 16.. growth rates [8] (HP, Str, Mag, Skl, Spd, Lck, Def, Res), signed
 * 24.. decisions predicted: count at 24, then 4 words each from 32:
 *      attacker index | target index << 8 | action << 16 | item << 24,
 *      move x | move y << 8 (signed bytes), from x | y << 8, flags
 */
uint32_t info_ui_words[IU_WORDS];

#define W_MODE 0
#define W_DEBUG 1
#define W_RUNS 2
#define W_ABORTS 3
#define W_MICROS 4
#define W_LINES 5
#define W_SIMULATE 6
#define W_GROWTH 7
#define W_FREE_FRAME 8
#define W_SVCS 9
#define W_PAGES 10
#define W_STATUS_DRAWS 11
#define W_STATUS_UNIT 12
#define W_GROWTH_UNIT 13
#define W_STATUS_HELP 14   /* the unit status' help box (a skill's, an item's) is up */
#define W_PICK_FRAME 15    /* the frame of the last tick choosing a unit's destination */
#define W_PLACED_UNIT 25   /* the lines shown have this unit (pool index) ... */
#define W_PLACED_XY 26     /* ... standing here (x | y << 8) */
#define W_PICK_UNIT 27     /* the unit whose destination is being chosen */
#define W_PREDICTING 28    /* vowperf: a prediction is running now */
#define W_STEP_MAX 29      /* vowperf: the most instructions one AI call of a prediction ran */
#define W_OFF 30           /* vowperf: predictions off, 1 for this map, 2 for the session */
#define W_RUN_MAX 31       /* vowperf: the longest prediction run, microseconds */
#define W_TEST_BUDGET 500  /* vowperf: tests (FATES_INFO_BUDGET): another budget for a prediction's calls */
#define W_NESTED 420       /* predict: guest calls that returned through other frames (gcall) */
#define W_BAD 421          /* predict: predictions given up at an access to nothing (unmapped, refused) */
#define W_BAD_ADDR 422     /* predict: the last such access's address */
#define W_GROWTH_RATES 16
#define W_DECISIONS 24
#define W_DECISION0 32
#define MAX_DECISIONS 96

/* addresses (USA Special Edition v2) */
#define SITUATION_PTR 0x006D7ED0u   /* map::Situation*, +8 the force whose phase it is */
#define MIND_PTR 0x006D7568u        /* map::Mind* */
#define UNIT_POOL_PTR 0x006D9F40u   /* Unit[], 0x140 each, index 1-based */
#define FORCES_PTR 0x006D9F44u      /* Force[3], 12 bytes each, first unit at +0 */
#define IMAGE_PTR 0x006D73E8u       /* map::Image*, unit indices at +8 + (x | y << 5) */
#define CURSOR_PTR 0x006D6EC4u      /* the map cursor, tile x, y floats at +0x18, +0x1C */
#define PANEL_PTR 0x006D70A8u       /* map::Panel*; the floats beside the pointer, +0x34 the tile size, +0x38 the panels' polygon offset */
#define SHARED_TEXTURE_PTR 0x006D7114u /* map::SharedTexture*, +0x10 the route arrow's texture */
#define FRAME_COUNTER 0x006DCAA0u   /* the counter the route arrow's shimmer follows */
#define HEIGHT_SCALE 0x00611E94u    /* float, HeightData's scale */
#define FIELD_WORLD_PTR 0x006BE404u /* FieldWorld*, +0x24 the height map */
#define COLOUR_TABLE 0x00717648u    /* the map's colours; +0x8C the attack panels' red */

#define F_OPERATOR_NEW 0x00113B6Cu
#define F_AI_CTOR 0x00223D44u
#define F_AI_PROCESSING 0x002238CCu
#define F_VB_CTOR 0x001C5AACu        /* VertexBuffer::VertexBuffer(int, int) */
#define F_VB_DELETE 0x001C5B10u      /* VertexBuffer's deleting destructor */
#define F_PUSH_STATE 0x00416CF0u
#define F_BEGIN_STATE 0x0041538Cu
#define F_SET_TARGET 0x00416D2Cu
#define F_SET_CAMERA 0x00416D00u
#define F_SET_PRIORITY 0x00415514u
#define F_SET_SHAPE 0x00416CB4u
#define F_SET_TEXTURE 0x00415430u
#define F_SET_DEPTH_TEST 0x004155ACu
#define F_SET_POLYGON_OFFSET 0x00415878u
#define F_END_STATE 0x00416C58u
#define F_DRAW_VERTEX_BUFFER 0x00415864u
#define F_POP_STATE 0x00416C90u
#define F_GET_GROW 0x005330B4u

#define MIND_END 0x24

static int iu_debug(int bit) { return (info_ui_words[W_DEBUG] & bit) != 0; }

/* ---- saving and restoring what a guest call changes in the context ----- */

typedef struct {
    uint32_t r[16];
    uint8_t n, z, c, v, q, thumb, ge, exclusive;
    int32_t budget;
    uint32_t exit, svc, depth, exclusive_address;
    uint32_t vfp[32], fpscr;
} Saved;

static void save(Context *ctx, Saved *s) {
    memcpy(s->r, ctx->r, sizeof s->r);
    s->n = ctx->n; s->z = ctx->z; s->c = ctx->c; s->v = ctx->v; s->q = ctx->q;
    s->thumb = ctx->thumb; s->ge = ctx->ge; s->exclusive = ctx->exclusive;
    s->budget = ctx->budget; s->exit = ctx->exit; s->svc = ctx->svc; s->depth = ctx->depth;
    s->exclusive_address = ctx->exclusive_address;
    memcpy(s->vfp, ctx->vfp, sizeof s->vfp);
    s->fpscr = *ctx->fpscr;
}

static void restore(Context *ctx, const Saved *s) {
    memcpy(ctx->r, s->r, sizeof s->r);
    ctx->n = s->n; ctx->z = s->z; ctx->c = s->c; ctx->v = s->v; ctx->q = s->q;
    ctx->thumb = s->thumb; ctx->ge = s->ge; ctx->exclusive = s->exclusive;
    ctx->budget = s->budget; ctx->exit = s->exit; ctx->svc = s->svc; ctx->depth = s->depth;
    ctx->exclusive_address = s->exclusive_address;
    memcpy(ctx->vfp, s->vfp, sizeof s->vfp);
    *ctx->fpscr = s->fpscr;
}

/* calls the guest function at fn with r0-r3, on the stack below the
   current sp, with all the budget it needs (gcall_budget); 1 when it
   returned.

   predict: when the function has returned is for the guest to say, not
   for the C call. The generated code hands control back to the host
   before the guest function returns whenever it can't go on in the C frames
   it is in: it entered another function midway (a jump from one function
   into another and back, as code mods' hooks do: V.O.W Edition's
   AI::Processing goes out to its own code after AIThink::Think and jumps
   back in; its BattleInfo::Calculate ends in its own code), a return came
   to an address its caller did not expect, the C calls nested too deep. The
   frames then unwind, r15 is where the guest is, and the host carries on
   from there; its stack and registers are all in the context. So does this
   call now: it carries on from r15 until the guest comes back to
   GCALL_RETURN (an address with no code, the function's return address)
   with the stack as it was. It used to take any return of the C call for
   the function's, and to give fn itself as the return address: a call that
   unwound came back early (V.O.W: AI::Processing "decided" nothing, the
   same unit again and again), or ran fn again from its start with the
   registers it had (V.O.W: a null this; the game's own AI on the copy,
   rarely: garbage pointers followed until the budget ran out, issue #12). */
#define GCALL_RETURN 0x00000FF0u
#define GCALL_RESUMES (1 << 20) /* each runs code: the budget ends them first */
static int32_t gcall_budget = 0x40000000;
static int cow_failed;
static Host cow_host;
static int gcall(Context *ctx, uint32_t fn, uint32_t a0, uint32_t a1, uint32_t a2, uint32_t a3) {
    uint32_t sp = ctx->r[13];
    ctx->r[0] = a0;
    ctx->r[1] = a1;
    ctx->r[2] = a2;
    ctx->r[3] = a3;
    ctx->r[14] = GCALL_RETURN;
    ctx->r[15] = fn;
    ctx->thumb = 0;
    ctx->budget = gcall_budget;
    /* tests (FATES_INFO_DEBUG bit 256): a prediction's calls start 8 C
       calls short of RECOMP_DEPTH_LIMIT, so the code unwinds whenever it
       nests deeper and this call carries on from r15 each time, as the host
       does: the decisions must be the same */
    uint32_t deep = ctx->host == &cow_host && iu_debug(256) && ctx->depth + 8 < RECOMP_DEPTH_LIMIT ? RECOMP_DEPTH_LIMIT - 8 - ctx->depth : 0;
    ctx->depth += 1 + deep;
    if (ctx->host == &cow_host && iu_debug(512)) {
        /* tests (bit 512): the call as it was before (1.3, vowperf), to show
           what it does with the code unwinding: fn its own return address,
           any return of the C call taken for the guest's */
        ctx->r[14] = fn;
        recomp_call(ctx);
        ctx->depth -= 1 + deep;
        return ctx->exit == 0;
    }
    for (int resumes = 0;; resumes++) {
        int32_t before = ctx->budget;
        recomp_call(ctx);
        if ((ctx->exit == 0 || ctx->exit == EXIT_UNWIND) && ctx->r[15] == GCALL_RETURN && !ctx->thumb && ctx->r[13] == sp) {
            ctx->exit = 0; /* returned (through the host when EXIT_UNWIND) */
            break;
        }
        /* the budget, an svc left for the host, an access the copy refused:
           the call ends there, not returned */
        if (ctx->exit != 0 && ctx->exit != EXIT_UNWIND)
            break;
        if (ctx->host == &cow_host && cow_failed)
            break;
        /* outside a prediction an svc may unwind for the host to switch
           threads, which this call can't do: only a plain unwind (exit 0)
           goes on there. On the copy an svc ends the prediction (cow_svc). */
        if (ctx->host != &cow_host && ctx->exit != 0)
            break;
        if (ctx->exit == EXIT_UNWIND && ctx->budget == before)
            break; /* no code at r15: nothing ran */
        if (resumes == GCALL_RESUMES || ctx->r[15] == GCALL_RETURN) {
            if (!ctx->exit)
                ctx->exit = EXIT_UNWIND;
            break;
        }
        /* the frames unwound with the guest elsewhere: on from r15 */
        ctx->exit = 0;
        info_ui_words[W_NESTED]++;
    }
    ctx->depth -= 1 + deep;
    return ctx->exit == 0;
}

static float f32_of(uint32_t bits) { float f; memcpy(&f, &bits, 4); return f; }
static uint32_t bits_of(float f) { uint32_t u; memcpy(&u, &f, 4); return u; }

/* ---- the copy-on-write view of guest memory ---------------------------- */

#define PAGE_COUNT (1u << 20)
#define COW_PAGES 8192 /* 32 MiB at most */

static uint8_t **cow_read;
static uint8_t **cow_write;
static uint8_t *cow_pool;
static uint32_t cow_dirty[COW_PAGES];
static uint32_t cow_used;
static uint8_t *const *orig_read;
static uint8_t *const *orig_write;
static const Host *orig_host;
static uint32_t cow_svcs;

static uint8_t *cow_page(uint32_t page) {
    if (cow_write[page])
        return cow_write[page];
    uint8_t *src = orig_write[page];
    if (!src)
        return 0;
    if (cow_used >= COW_PAGES) {
        cow_failed = 1;
        return 0;
    }
    uint8_t *copy = cow_pool + (size_t)cow_used * 4096;
    memcpy(copy, src, 4096);
    cow_dirty[cow_used++] = page;
    cow_write[page] = copy;
    cow_read[page] = copy;
    return copy;
}

/* predict: the first access a prediction made where the game's memory has
   nothing (an unmapped page, the read of a garbage pointer, a write the copy
   refuses): the address, 1 for a write, and the prediction stops there. */
static uint32_t cow_bad_addr;
static int cow_bad_write;
static uint32_t cow_bad_pc;
static int cow_bad_seen;

static void cow_bad(Context *ctx, uint32_t a, int write) {
    if (!cow_failed) {
        cow_bad_seen = 1;
        cow_bad_addr = a;
        cow_bad_write = write;
        cow_bad_pc = ctx->r[15];
    }
    cow_failed = 1;
    /* leave at the next call or return: the code goes no further on what it
       read (a value of 0) than to the end of the block it is in */
    if (!ctx->exit)
        ctx->exit = EXIT_UNWIND;
}

/* predict: a page whose read side is empty is the host's to read only where
   the GPU keeps what it drew (the linear heap and VRAM, guarded until the
   GPU writes it back); anywhere else it is unmapped, and the host would log
   the access (once per page: a run that follows garbage pointers logged a
   line for each of thousands of pages, issue #12). */
static int cow_host_page(uint32_t a) {
    return (a >= 0x14000000u && a < 0x1C000000u) || (a >= 0x1F000000u && a < 0x1F600000u) || (a >= 0x30000000u && a < 0x38000000u);
}

static uint8_t cow_r8(Context *ctx, uint32_t a) {
    uint8_t *page = cow_read[a >> 12];
    if (page)
        return page[a & 0xFFF];
    if (cow_host_page(a))
        return orig_host->read8(ctx, a);
    cow_bad(ctx, a, 0);
    return 0;
}
static uint16_t cow_r16(Context *ctx, uint32_t a) {
    return (uint16_t)(cow_r8(ctx, a) | cow_r8(ctx, a + 1) << 8);
}
static uint32_t cow_r32(Context *ctx, uint32_t a) {
    return (uint32_t)cow_r8(ctx, a) | (uint32_t)cow_r8(ctx, a + 1) << 8 | (uint32_t)cow_r8(ctx, a + 2) << 16 | (uint32_t)cow_r8(ctx, a + 3) << 24;
}
static void cow_w8(Context *ctx, uint32_t a, uint8_t v) {
    uint8_t *page = cow_page(a >> 12);
    if (page)
        page[a & 0xFFF] = v;
    else {
        if (iu_debug(1) && !cow_failed)
            fprintf(stderr, "info-ui: predicting: a write to %08X at %08X, %s, abandoned\n", a, ctx->r[15],
                    cow_used >= COW_PAGES ? "the copy is full" : "a page with no write side");
        cow_bad(ctx, a, 1); /* a device or read-only page: never let it through */
    }
}
static void cow_w16(Context *ctx, uint32_t a, uint16_t v) {
    cow_w8(ctx, a, (uint8_t)v);
    cow_w8(ctx, a + 1, (uint8_t)(v >> 8));
}
static void cow_w32(Context *ctx, uint32_t a, uint32_t v) {
    for (int i = 0; i < 4; i++)
        cow_w8(ctx, a + i, (uint8_t)(v >> 8 * i));
}
static void cow_svc(Context *ctx, uint32_t number) {
    /* the kernel's objects are not in the copy: leave at once */
    cow_svcs++;
    if (iu_debug(1))
        fprintf(stderr, "info-ui: predicting: svc 0x%02X at %08X, abandoned\n", number, ctx->r[15]);
    cow_failed = 1;
    ctx->exit = EXIT_UNWIND;
}

static int cow_begin(Context *ctx) {
    if (!cow_read) {
        cow_read = calloc(PAGE_COUNT, sizeof *cow_read);
        cow_write = calloc(PAGE_COUNT, sizeof *cow_write);
        cow_pool = malloc((size_t)COW_PAGES * 4096);
        if (!cow_read || !cow_write || !cow_pool)
            return 0;
    }
    orig_read = ctx->read_pages;
    orig_write = ctx->write_pages;
    orig_host = ctx->host;
    memcpy(cow_read, orig_read, PAGE_COUNT * sizeof *cow_read);
    cow_host = *orig_host;
    cow_host.read8 = cow_r8;
    cow_host.read16 = cow_r16;
    cow_host.read32 = cow_r32;
    cow_host.write8 = cow_w8;
    cow_host.write16 = cow_w16;
    cow_host.write32 = cow_w32;
    cow_host.svc = cow_svc;
    cow_used = 0;
    cow_failed = 0;
    cow_bad_addr = 0;
    cow_bad_write = 0;
    cow_bad_pc = 0;
    cow_bad_seen = 0;
    ctx->read_pages = (uint8_t *const *)cow_read;
    ctx->write_pages = (uint8_t *const *)cow_write;
    ctx->host = &cow_host;
    return 1;
}

static void cow_end(Context *ctx) {
    for (uint32_t i = 0; i < cow_used; i++)
        cow_write[cow_dirty[i]] = 0;
    ctx->read_pages = orig_read;
    ctx->write_pages = orig_write;
    ctx->host = orig_host;
}

/* ---- the prediction ------------------------------------------------------ */

static uint32_t unit_at_index(Context *ctx, uint32_t index) {
    uint32_t pool = mem_read32(ctx, UNIT_POOL_PTR);
    if (!pool || !index)
        return 0;
    return pool + (index - 1) * 0x140;
}

static uint32_t unit_force(Context *ctx, uint32_t unit) {
    uint32_t force = mem_read32(ctx, unit + 0xA4);
    return force ? mem_read8(ctx, force + 8) : 0xFF;
}

/* a hash of every unit on the map: when it changes, the prediction is due */
static uint64_t units_hash(Context *ctx) {
    uint64_t h = 1469598103934665603ull;
    uint32_t forces = mem_read32(ctx, FORCES_PTR);
    if (!forces)
        return 0;
    for (uint32_t f = 0; f < 3; f++) {
        uint32_t unit = mem_read32(ctx, forces + f * 12);
        for (int guard = 0; unit && guard < 256; guard++) {
            const uint8_t *bytes = mem_read_span(ctx, unit, 0x140);
            for (uint32_t i = 0; i < 0x140; i++) {
                h ^= bytes ? bytes[i] : mem_read8(ctx, unit + i);
                h *= 1099511628211ull;
            }
            unit = mem_read32(ctx, unit + 0x14);
        }
    }
    return h;
}

static uint32_t predicted_sit;

/* where a prediction stands the unit being moved (round 3): none for the
   map as it is, or the selected unit on the tile the cursor is on while its
   destination is chosen */
typedef struct {
    uint32_t unit; /* pool index, 0 for none */
    int x, y;
} Place;

/* one run of the enemy phase's AI on the copy: the decisions into out (4
   words each, as info_ui_words' decisions) and their count; 0 when
   abandoned. seed 0 keeps the seed AI::AI takes from the game's Random;
   another reseeds the AI's own Random with it (Random::Initialize). */
#define F_RANDOM_INIT 0x0044ABC4u
static void copy_move(Context *ctx, uint32_t unit, int x, int y) {
    uint32_t img = mem_read32(ctx, IMAGE_PTR);
    uint8_t w = mem_read8(ctx, img), h = mem_read8(ctx, img + 1);
    uint32_t fx = mem_read8(ctx, unit + 0xF4), fy = mem_read8(ctx, unit + 0xF5);
    if (fx < w && fy < h && mem_read8(ctx, img + 8 + (fx | fy << 5)) == mem_read8(ctx, unit + 0xF0))
        mem_write8(ctx, img + 8 + (fx | fy << 5), 0);
    if (x >= 0 && y >= 0 && x < w && y < h)
        mem_write8(ctx, img + 8 + ((uint32_t)x | (uint32_t)y << 5), mem_read8(ctx, unit + 0xF0));
    mem_write8(ctx, unit + 0xF4, (uint8_t)x);
    mem_write8(ctx, unit + 0xF5, (uint8_t)y);
}

/* vowperf: a run that failed in a way that comes back every time: the
   copy refused a write (a code mod's AI writing through a pointer the copy
   has no page for) or an AI call ran out of its budget */
static int fork_hard_failure;
static int fork_bad_access; /* predict: the run stopped at an access to nothing */

/* predict: where a run failed (the step, the instructions it ran), and for
   tests (FATES_INFO_DEBUG bit 16) a probe that runs one step on a budget of
   probe_budget and keeps the registers and the stack it left with */
static int fork_fail_step;
static int32_t fork_fail_used;
static int probe_step = -1;
static int32_t probe_budget;
static int probe_failed;
static int probe_returned;
static int fork_zero_step;   /* tests: the first step that decided nothing (action 0) */
static int32_t fork_zero_used;
static uint32_t probe_exit;
static uint32_t probe_r[16];
static uint32_t probe_stack[48];
static uint32_t probe_sp; /* the sp the run's calls start with */

static int run_fork(Context *ctx, uint32_t sp, uint32_t seed, const Place *place, uint32_t *out, uint32_t *count_out) {
    uint32_t sit = mem_read32(ctx, SITUATION_PTR);
    uint32_t count = 0;
    if (!cow_begin(ctx))
        return 0;
    int ok;
    /* the enemy's phase, in the copy */
    mem_write8(ctx, sit + 8, 1);
    /* the unit being moved, on the hovered tile, in the copy */
    if (place->unit) {
        uint32_t unit = unit_at_index(ctx, place->unit);
        if (unit)
            copy_move(ctx, unit, place->x, place->y);
    }
    ctx->r[13] = (sp - 0x40) & ~7u;
    ok = gcall(ctx, F_OPERATOR_NEW, 0x24, 0, 0, 0) && !cow_failed;
    if (!ok && iu_debug(1))
        fprintf(stderr, "info-ui: predicting: operator new left with exit %u at %08X\n", ctx->exit, ctx->r[15]);
    uint32_t ai = ok ? ctx->r[0] : 0;
    probe_sp = (sp - 0x40) & ~7u;
    if (ai) {
        ctx->r[13] = (sp - 0x40) & ~7u;
        ok = gcall(ctx, F_AI_CTOR, ai, 0, 0, 0) && !cow_failed;
        if (ok && seed) {
            ctx->r[13] = (sp - 0x40) & ~7u;
            ok = gcall(ctx, F_RANDOM_INIT, ai, seed, 0, 0) && !cow_failed;
        }
    } else {
        ok = 0;
    }
    fork_fail_step = -1;
    fork_zero_step = -1;
    for (int step = 0; ok && step < 200 && count < MAX_DECISIONS; step++) {
        ctx->r[13] = (sp - 0x40) & ~7u;
        int32_t step_budget = gcall_budget;
        if (step == probe_step)
            gcall_budget = probe_budget;
        int returned = gcall(ctx, F_AI_PROCESSING, ai, 0, 0, 0);
        uint32_t used = (uint32_t)(gcall_budget - ctx->budget);
        if (step == probe_step) {
            probe_failed = cow_failed;
            probe_returned = returned;
            probe_exit = ctx->exit;
            memcpy(probe_r, ctx->r, sizeof probe_r);
            for (int i = 0; i < 48; i++)
                probe_stack[i] = mem_read32(ctx, ctx->r[13] + 4u * (uint32_t)i);
            gcall_budget = step_budget;
            ok = 0;
            break;
        }
        if (returned && used > info_ui_words[W_STEP_MAX])
            info_ui_words[W_STEP_MAX] = used;
        if (!returned || cow_failed) {
            fork_fail_step = step;
            fork_fail_used = (int32_t)used;
            /* predict: an access to nothing (a read of an unmapped page, a
               write the copy refuses) stops the run where it is made; it
               is counted per map (predict_runs). The budget running out
               stays a hard failure (vowperf). */
            if (cow_bad_seen)
                fork_bad_access = 1;
            else if (ctx->exit == EXIT_BUDGET)
                fork_hard_failure = 1;
            if (iu_debug(1))
                fprintf(stderr, "info-ui: predicting: step %d left with exit %u at %08X (copy %s)\n", step, ctx->exit, ctx->r[15], cow_failed ? "failed" : "ok");
            ok = 0;
            break;
        }
        uint32_t mind = mem_read32(ctx, MIND_PTR);
        if (!mind) {
            ok = 0;
            break;
        }
        uint32_t action = mem_read8(ctx, mind + 6);
        if (action == MIND_END)
            break;
        if (action == 0) {
            /* predict: the real phase never gets a decision of nothing; a
               call that comes back with one did not finish (as V.O.W's did
               before gcall carried on from r15), and the AI would decide the
               same unit again for every step left: the run is given up, as
               one that ran out of its budget (it would every time) */
            if (fork_zero_step < 0) {
                fork_zero_step = step;
                fork_zero_used = (int32_t)used;
            }
            if (iu_debug(1))
                fprintf(stderr, "info-ui: predicting: step %d decided nothing (unit %u), given up\n", step, mem_read8(ctx, mind + 0));
            fork_hard_failure = 1;
            ok = 0;
            break;
        }
        uint32_t index = mem_read8(ctx, mind + 0);
        uint32_t unit = unit_at_index(ctx, index);
        int8_t mx = (int8_t)mem_read8(ctx, mind + 4), my = (int8_t)mem_read8(ctx, mind + 5);
        uint32_t fx = unit ? mem_read8(ctx, unit + 0xF4) : 0xFF, fy = unit ? mem_read8(ctx, unit + 0xF5) : 0xFF;
        uint32_t *d = &out[4 * count];
        d[0] = index | (uint32_t)mem_read8(ctx, mind + 9) << 8 | action << 16 | (uint32_t)mem_read8(ctx, mind + 8) << 24;
        d[1] = (uint8_t)mx | (uint32_t)(uint8_t)my << 8;
        d[2] = fx | fy << 8;
        d[3] = mem_read8(ctx, mind + 0xE) | (uint32_t)mem_read8(ctx, mind + 0xF) << 8;
        count++;
        if (iu_debug(64) && !seed)
            fprintf(stderr, "info-ui: step %d: %u instructions, nested returns so far %u, AI state %u\n", step, used,
                    info_ui_words[W_NESTED], mem_read8(ctx, ai + 0x20));
        if (iu_debug(1) && !seed)
            fprintf(stderr, "info-ui: predicted unit %u at %u,%u: action 0x%02X move %d,%d target %u item %d trade %u\n",
                    index, fx, fy, action, mx, my, mem_read8(ctx, mind + 9), (int8_t)mem_read8(ctx, mind + 8), mem_read8(ctx, mind + 0xE));
        /* the unit has acted (Unit +8 bit 0, as the phase marks it once
           its action is done; AIOrder skips such units) */
        if (unit && action != 0)
            mem_write32(ctx, unit + 8, mem_read32(ctx, unit + 8) | 1);
        /* the unit goes where it chose before the next one thinks, as in
           the phase itself */
        if (unit && info_ui_words[W_SIMULATE] && mx >= 0 && my >= 0 && (mx != (int)fx || my != (int)fy))
            copy_move(ctx, unit, mx, my);
    }
    info_ui_words[W_PAGES] = cow_used;
    int failed = cow_failed || !ok;
    cow_end(ctx);
    *count_out = count;
    return !failed;
}

/* the AI's own Random breaks ties (AIThink::AttackTo: two targets scored
   alike, a coin), and the game seeds it from its Random, which runs on
   every frame, as the phase begins: a tie is chance. So the phase is run
   with the game's seed and with FORK_SEEDS - 1 others; each enemy's target
   is the one most runs chose, and how many did is kept (d[3] bits 16-23 of
   bits 24-31), so a line the dice decide is drawn fainter.

   Predictions are kept per situation (the units' hash, and where the unit
   being moved stands): moving the cursor back over a tile shows its lines
   at once, and a new tile costs one run a frame, its first run's lines
   showing the frame after. */
#define FORK_SEEDS 5
#define CACHE 24
typedef struct {
    uint64_t hash;
    Place place;
    uint32_t stamp;
    int runs;   /* runs done */
    int failed;
    uint32_t micros;
    uint32_t out[FORK_SEEDS][4 * MAX_DECISIONS];
    uint32_t count[FORK_SEEDS];
} Prediction;
static Prediction cache[CACHE];
static uint32_t cache_stamp;
static Prediction *shown;       /* what the lines show now */
static int shown_runs;

static void cache_clear(void) {
    for (int i = 0; i < CACHE; i++) {
        cache[i].stamp = 0;
        cache[i].runs = 0;
        cache[i].failed = 0;
    }
    shown = 0;
    shown_runs = 0;
    info_ui_words[W_DECISIONS] = 0;
}

static Prediction *cache_get(uint64_t hash, const Place *place) {
    Prediction *oldest = &cache[0];
    for (int i = 0; i < CACHE; i++) {
        Prediction *p = &cache[i];
        if (p->stamp && p->hash == hash && p->place.unit == place->unit && p->place.x == place->x && p->place.y == place->y) {
            p->stamp = ++cache_stamp;
            return p;
        }
        if (p->stamp < oldest->stamp)
            oldest = p;
    }
    if (oldest == shown)
        shown = 0;
    oldest->hash = hash;
    oldest->place = *place;
    oldest->stamp = ++cache_stamp;
    oldest->runs = 0;
    oldest->failed = 0;
    oldest->micros = 0;
    return oldest;
}

/* the target a run gave a unit: 0 for no attack, ~0 when it did not act */
static uint32_t fork_target(const Prediction *p, int k, uint32_t unit, const uint32_t **decision) {
    for (uint32_t j = 0; j < p->count[k]; j++) {
        const uint32_t *d = &p->out[k][4 * j];
        if ((d[0] & 0xFF) == unit) {
            if (decision)
                *decision = d;
            return (d[0] >> 16 & 0xFF) == 0x03 ? (d[0] >> 8 & 0xFF) : 0;
        }
    }
    return 0xFFFFFFFFu;
}

/* the decisions to draw from a prediction's runs so far: each enemy's
   target is the one most runs chose, how many did is kept */
static void publish(const Prediction *p, int log) {
    int runs = p->runs;
    uint32_t count = p->count[0];
    for (uint32_t i = 0; i < count; i++) {
        uint32_t *d = &info_ui_words[W_DECISION0 + 4 * i];
        memcpy(d, &p->out[0][4 * i], 16);
        uint32_t unit = d[0] & 0xFF;
        uint32_t targets[FORK_SEEDS], best = 0xFFFFFFFFu, best_votes = 0;
        for (int k = 0; k < runs; k++)
            targets[k] = fork_target(p, k, unit, 0);
        for (int k = 0; k < runs; k++) {
            uint32_t votes = 0;
            for (int m = 0; m < runs; m++)
                votes += targets[m] == targets[k];
            if (votes > best_votes) {
                best_votes = votes;
                best = targets[k];
            }
        }
        if (best != targets[0]) {
            /* most runs did otherwise: take one of theirs */
            for (int k = 1; k < runs; k++) {
                const uint32_t *other = 0;
                if (targets[k] == best && fork_target(p, k, unit, &other) == best && other) {
                    memcpy(d, other, 16);
                    break;
                }
            }
        }
        d[3] = (d[3] & 0xFFFF) | best_votes << 16 | (uint32_t)runs << 24;
        if (log && iu_debug(1) && runs == FORK_SEEDS)
            fprintf(stderr, "info-ui: final unit %u at %u,%u: action 0x%02X move %d,%d target %u item %d votes %u/%d\n", unit,
                    d[2] & 0xFF, d[2] >> 8 & 0xFF, d[0] >> 16 & 0xFF, (int8_t)(d[1] & 0xFF), (int8_t)(d[1] >> 8 & 0xFF),
                    d[0] >> 8 & 0xFF, (int8_t)(d[0] >> 24), best_votes, runs);
        if (log && iu_debug(1) && runs == FORK_SEEDS && best_votes < (uint32_t)runs)
            fprintf(stderr, "info-ui: unit %u: target %u in %u of %d runs (the AI's coin)\n", unit, best, best_votes, runs);
    }
    info_ui_words[W_DECISIONS] = count;
    info_ui_words[W_PLACED_UNIT] = p->place.unit;
    info_ui_words[W_PLACED_XY] = (uint32_t)(p->place.x & 0xFF) | (uint32_t)(p->place.y & 0xFF) << 8;
}

/* vowperf: what a prediction may cost, on the game's thread, in the
   frame it runs in.

   - Each guest call of a prediction gets PREDICT_BUDGET instructions, not
     the 2^30 the drawing's calls get. An AI call of the game's own AI runs
     at most a few million (W_STEP_MAX keeps the most seen; the bench's
     maps up to Chapter 7 and Ghostly Gold); a code mod's AI that goes wrong
     on the copy (V.O.W Edition's AI::Processing writes through a null
     pointer there, the write is refused, and its code then loops) used to
     run the whole 2^30, about 600 ms, once for every new situation: every
     tile a moved unit was held over froze the game for most of a second.
     (predict: that was gcall running AI::Processing again from its start
     after a return through the host; see gcall. V.O.W's AI now runs on
     the copy, up to about 13M instructions in a call.)
   - A run that fails that way fails the same way for every situation, so
     the predictions stop for the rest of the map (W_OFF 1); after two such
     failures with no prediction ever made in this session, for the session
     (W_OFF 2). An svc stays what it was: that situation's prediction alone
     is given up.
   - A run that took longer than SLOW_RUN_US (a map, or a code mod's AI,
     heavy enough) makes that map's predictions lighter: one run instead of
     FORK_SEEDS, and none per tile while a unit's destination is chosen
     (the lines of the map as it is stay shown). The game's own AI runs 1
     to 9 ms on the bench's maps, under it. */
#define PREDICT_BUDGET (24 << 20)
#define SLOW_RUN_US 10000u
static uint32_t off_sit;      /* the map (map::Situation) predictions are off for */
static uint32_t slow_sit;     /* the map whose predictions are lightened */
static int hard_failures;     /* runs that failed hard, this session */
static int ever_predicted;    /* a prediction run succeeded this session */
/* predict: predictions given up at an access to nothing, on the map
   bad_sit; at BAD_PER_MAP of them the map's predictions are off */
#define BAD_PER_MAP 3
#define BAD_LINES 50
static uint32_t bad_sit;
static int bad_on_map;
static int bad_lines;

/* predict, tests (FATES_INFO_DEBUG bit 16): where a failed run's first bad
   access is. The run is made again with the failing step on smaller
   budgets: the smallest that still fails ends in the block holding the
   access, so one instruction less stops at that block's start, and r15, the
   registers and the stack (return addresses) say where it is. */
static void locate(Context *ctx, const Saved *saved, uint32_t sp, uint32_t seed, const Place *place) {
    static uint32_t scratch[4 * MAX_DECISIONS];
    uint32_t count;
    int step = fork_fail_step;
    uint32_t bad = cow_bad_addr;
    int write = cow_bad_write;
    int32_t lo = 0, hi = fork_fail_used + 64;
    probe_step = step;
    probe_budget = hi;
    run_fork(ctx, sp, seed, place, scratch, &count);
    restore(ctx, saved);
    if (!probe_failed) {
        fprintf(stderr, "info-ui: locate: step %d does not fail again on %d instructions\n", step, hi);
        probe_step = -1;
        return;
    }
    while (hi - lo > 1) {
        int32_t mid = lo + (hi - lo) / 2;
        probe_budget = mid;
        run_fork(ctx, sp, seed, place, scratch, &count);
        restore(ctx, saved);
        if (probe_failed)
            hi = mid;
        else
            lo = mid;
    }
    probe_budget = lo;
    run_fork(ctx, sp, seed, place, scratch, &count);
    restore(ctx, saved);
    probe_step = -1;
    fprintf(stderr, "info-ui: locate: step %d, the %s %08X is in the block at %08X (after %d instructions, exit %u)\n", step,
            write ? "write to" : "read of", bad, probe_r[15], lo, probe_exit);
    fprintf(stderr, "info-ui: locate: r0-r7 %08X %08X %08X %08X %08X %08X %08X %08X\n", probe_r[0], probe_r[1], probe_r[2],
            probe_r[3], probe_r[4], probe_r[5], probe_r[6], probe_r[7]);
    fprintf(stderr, "info-ui: locate: r8-r14 %08X %08X %08X %08X %08X %08X %08X\n", probe_r[8], probe_r[9], probe_r[10],
            probe_r[11], probe_r[12], probe_r[13], probe_r[14]);
    fprintf(stderr, "info-ui: locate: code addresses on the stack:");
    for (int i = 0; i < 48; i++)
        if (probe_stack[i] >= 0x00100000u && probe_stack[i] < 0x00830000u)
            fprintf(stderr, " +%02X:%08X", i * 4, probe_stack[i]);
    fprintf(stderr, "\n");
}

/* tests (FATES_INFO_DEBUG bit 128): where a step that decided nothing
   (action 0) returned from: the smallest budget it returns on, and the
   block one instruction less stops at */
static void locate_return(Context *ctx, const Saved *saved, uint32_t sp, uint32_t seed, const Place *place) {
    static uint32_t scratch[4 * MAX_DECISIONS];
    uint32_t count;
    int step = fork_zero_step;
    int32_t lo = 0, hi = fork_zero_used + 64;
    probe_step = step;
    while (hi - lo > 1) {
        int32_t mid = lo + (hi - lo) / 2;
        probe_budget = mid;
        run_fork(ctx, sp, seed, place, scratch, &count);
        restore(ctx, saved);
        if (probe_returned)
            hi = mid;
        else
            lo = mid;
    }
    probe_budget = hi;
    run_fork(ctx, sp, seed, place, scratch, &count);
    restore(ctx, saved);
    fprintf(stderr, "info-ui: return: at %d: returned %d, r15 %08X r13 %08X r14 %08X r0 %08X (the call's sp %08X), nested %u\n", hi,
            probe_returned, probe_r[15], probe_r[13], probe_r[14], probe_r[0], probe_sp, info_ui_words[W_NESTED]);
    probe_budget = lo;
    run_fork(ctx, sp, seed, place, scratch, &count);
    restore(ctx, saved);
    probe_step = -1;
    fprintf(stderr, "info-ui: return: step %d returns after %d instructions, from the block at %08X\n", step, hi, probe_r[15]);
    fprintf(stderr, "info-ui: return: r0-r7 %08X %08X %08X %08X %08X %08X %08X %08X\n", probe_r[0], probe_r[1], probe_r[2],
            probe_r[3], probe_r[4], probe_r[5], probe_r[6], probe_r[7]);
    fprintf(stderr, "info-ui: return: r8-r14 %08X %08X %08X %08X %08X %08X %08X\n", probe_r[8], probe_r[9], probe_r[10],
            probe_r[11], probe_r[12], probe_r[13], probe_r[14]);
    fprintf(stderr, "info-ui: return: code addresses on the stack:");
    for (int i = 0; i < 48; i++)
        if (probe_stack[i] >= 0x00100000u && probe_stack[i] < 0x00830000u)
            fprintf(stderr, " +%02X:%08X", i * 4, probe_stack[i]);
    fprintf(stderr, "\n");
}

/* runs runs [first, last) of a prediction */
static void predict_runs(Context *ctx, Prediction *p, int last) {
    uint32_t sit = mem_read32(ctx, SITUATION_PTR);
    uint32_t image = mem_read32(ctx, IMAGE_PTR);
    if (!sit || !image || !mem_read32(ctx, UNIT_POOL_PTR)) {
        p->failed = 1;
        return;
    }
    static const uint32_t seeds[FORK_SEEDS] = { 0, 0x9E3779B9u, 0x7F4A7C15u, 0x2545F491u, 0x6C8E9CF5u };
    Saved saved;
    save(ctx, &saved);
    struct timeval t0;
    gettimeofday(&t0, 0);
    uint32_t sp = ctx->r[13];
    if (p->runs == 0)
        cow_svcs = 0;
    info_ui_words[W_PREDICTING] = 1;
    int32_t budget_before = gcall_budget;
    gcall_budget = info_ui_words[W_TEST_BUDGET] ? (int32_t)info_ui_words[W_TEST_BUDGET] : PREDICT_BUDGET;
    fork_hard_failure = 0;
    fork_bad_access = 0;
    uint32_t bad_addr = 0, bad_pc = 0;
    int bad_write = 0, bad_step = -1;
    while (p->runs < last && !p->failed) {
        struct timeval r0, r1;
        gettimeofday(&r0, 0);
        p->failed = !run_fork(ctx, sp, seeds[p->runs], &p->place, p->out[p->runs], &p->count[p->runs]);
        restore(ctx, &saved);
        if (p->failed && cow_bad_seen) {
            bad_addr = cow_bad_addr;
            bad_write = cow_bad_write;
            bad_pc = cow_bad_pc;
            bad_step = fork_fail_step;
        }
        if (p->failed && iu_debug(16) && fork_fail_step >= 0 && cow_bad_seen)
            locate(ctx, &saved, sp, seeds[p->runs], &p->place);
        if (iu_debug(128) && fork_zero_step >= 0 && !seeds[p->runs])
            locate_return(ctx, &saved, sp, seeds[p->runs], &p->place);
        gettimeofday(&r1, 0);
        uint32_t run_us = (uint32_t)((r1.tv_sec - r0.tv_sec) * 1000000ll + (r1.tv_usec - r0.tv_usec));
        if (run_us > info_ui_words[W_RUN_MAX])
            info_ui_words[W_RUN_MAX] = run_us;
        if (!p->failed) {
            p->runs++;
            ever_predicted = 1;
            if (run_us > SLOW_RUN_US && slow_sit != sit) {
                slow_sit = sit;
                if (iu_debug(1))
                    fprintf(stderr, "info-ui: a run took %u us: this map's predictions are lighter\n", run_us);
            }
        }
    }
    gcall_budget = budget_before;
    restore(ctx, &saved);
    info_ui_words[W_PREDICTING] = 0;
    if (p->failed && fork_bad_access) {
        /* predict: one line for each prediction given up this way (at most
           BAD_LINES a session), nothing for each access */
        info_ui_words[W_BAD]++;
        info_ui_words[W_BAD_ADDR] = bad_addr;
        if (bad_sit != sit) {
            bad_sit = sit;
            bad_on_map = 0;
        }
        bad_on_map++;
        if (bad_lines < BAD_LINES) {
            bad_lines++;
            fprintf(stderr, "info-ui: a prediction stopped at a %s %08X (step %d, near %08X); its lines are left out%s\n",
                    bad_write ? "write to" : "read of unmapped", bad_addr, bad_step, bad_pc,
                    bad_on_map >= BAD_PER_MAP ? ", and this map's predictions are off" : "");
        }
        if (bad_on_map >= BAD_PER_MAP) {
            off_sit = sit;
            info_ui_words[W_OFF] = 1;
        }
    }
    if (p->failed && fork_hard_failure) {
        hard_failures++;
        off_sit = sit;
        info_ui_words[W_OFF] = !ever_predicted && hard_failures >= 2 ? 2 : 1;
        if (iu_debug(1))
            fprintf(stderr, "info-ui: the AI fails on the copy: predictions off %s\n", info_ui_words[W_OFF] == 2 ? "for the session" : "for this map");
    }
    struct timeval t1;
    gettimeofday(&t1, 0);
    p->micros += (uint32_t)((t1.tv_sec - t0.tv_sec) * 1000000ll + (t1.tv_usec - t0.tv_usec));
    if (p->failed) {
        info_ui_words[W_RUNS]++;
        info_ui_words[W_ABORTS]++;
        info_ui_words[W_SVCS] += cow_svcs;
        if (iu_debug(1))
            fprintf(stderr, "info-ui: prediction %u: abandoned\n", info_ui_words[W_RUNS]);
        return;
    }
    if (p->runs == FORK_SEEDS) {
        publish(p, 1); /* the vote, logged (debug) before the line that closes it */
        info_ui_words[W_RUNS]++;
        info_ui_words[W_SVCS] += cow_svcs;
        info_ui_words[W_MICROS] = p->micros;
        if (iu_debug(1))
            fprintf(stderr, "info-ui: prediction %u: done, %u decisions, %d runs, %u pages copied, %u us, unit %u at %d,%d\n",
                    info_ui_words[W_RUNS], p->count[0], FORK_SEEDS, info_ui_words[W_PAGES], p->micros, p->place.unit, p->place.x, p->place.y);
    }
}

/* once a tick of the free cursor or of choosing where a unit moves: the
   prediction for this situation, one run further, and the lines show it as
   soon as it has a run; until then the last one stays */
static void predict_tick(Context *ctx, const Place *place) {
    uint32_t sit = mem_read32(ctx, SITUATION_PTR);
    if (!sit || mem_read8(ctx, sit + 8) != 0)
        return;
    uint64_t h = units_hash(ctx) ^ (uint64_t)sit << 32;
    if (sit != predicted_sit) {
        predicted_sit = sit;
        cache_clear();
        if (info_ui_words[W_OFF] == 1 && sit != off_sit)
            info_ui_words[W_OFF] = 0;
    }
    /* vowperf: off for this map or the session, or lighter on this map */
    if (info_ui_words[W_OFF]) {
        info_ui_words[W_DECISIONS] = 0;
        return;
    }
    Place none = { 0, 0, 0 };
    int slow = sit == slow_sit;
    if (slow)
        place = &none;
    Prediction *p = cache_get(h, place);
    if (!p->failed && p->runs < (slow ? 1 : FORK_SEEDS))
        predict_runs(ctx, p, p->runs + 1);
    if (info_ui_words[W_OFF]) {
        info_ui_words[W_DECISIONS] = 0;
        return;
    }
    if (p->failed) {
        if (shown == p || !shown)
            info_ui_words[W_DECISIONS] = 0;
        return;
    }
    if (p->runs > 0 && (shown != p || shown_runs != p->runs)) {
        publish(p, 0);
        shown = p;
        shown_runs = p->runs;
    }
}

/* the whole prediction at once (the test at the enemy phase's start) */
static void info_predict(Context *ctx) {
    static Prediction test;
    Place none = { 0, 0, 0 };
    test.place = none;
    test.runs = 0;
    test.failed = 0;
    test.micros = 0;
    predict_runs(ctx, &test, FORK_SEEDS);
    shown = 0;
}

/* map::SequenceHuman::...::ProcSequence::FreeCursorTick(void) */
RECOMP_OVERRIDE(0x00351904) {
    pulse_free_cursor(ctx); /* pulse: the free cursor's frame (overrides/pulse.c) */
    /* cheats: the free cursor's frame, for Move Again (overrides/cheats.c) */
    {
        extern uint32_t cheats_words[];
        if (cheats_words[0] & 0x10)
            cheats_words[16] = mem_read32(ctx, FRAME_COUNTER);
    }
    if (info_ui_words[W_MODE]) {
        info_ui_words[W_FREE_FRAME] = mem_read32(ctx, FRAME_COUNTER);
        Place none = { 0, 0, 0 };
        predict_tick(ctx, &none);
    }
    CALL(RECOMP_ORIGINAL(0x00351904));
}

/* map::SequenceHuman::...::ProcSequence::PickCursorTick(void): choosing
   where the selected unit (map::Mind +0) moves. The lines are those of the
   phase with the unit on the tile under the cursor, when it can stop there
   (in its move range, map::Deploy's move image at *0x006D7364 +0x80C, and
   no other unit on it); elsewhere those of the map as it is. */
#define DEPLOY_PTR 0x006D7364u
RECOMP_OVERRIDE(0x00352800) {
    if (info_ui_words[W_MODE]) {
        uint32_t frame = mem_read32(ctx, FRAME_COUNTER);
        info_ui_words[W_FREE_FRAME] = frame;
        info_ui_words[W_PICK_FRAME] = frame;
        Place place = { 0, 0, 0 };
        uint32_t mind = mem_read32(ctx, MIND_PTR), cursor = mem_read32(ctx, CURSOR_PTR);
        uint32_t image = mem_read32(ctx, IMAGE_PTR), deploy = mem_read32(ctx, DEPLOY_PTR);
        uint32_t index = mind ? mem_read8(ctx, mind) : 0;
        info_ui_words[W_PICK_UNIT] = index;
        if (index && cursor && image && deploy) {
            int x = (int)f32_of(mem_read32(ctx, cursor + 0x18)), y = (int)f32_of(mem_read32(ctx, cursor + 0x1C));
            uint8_t w = mem_read8(ctx, image), h = mem_read8(ctx, image + 1);
            uint32_t move = mem_read32(ctx, deploy + 0x80C);
            if (x >= 0 && y >= 0 && x < w && y < h && move) {
                uint32_t at = (uint32_t)x | (uint32_t)y << 5;
                uint32_t on = mem_read8(ctx, image + 8 + at);
                if ((int8_t)mem_read8(ctx, move + at) >= 0 && (on == 0 || on == index)) {
                    place.unit = index;
                    place.x = x;
                    place.y = y;
                }
            }
        }
        predict_tick(ctx, &place);
    }
    CALL(RECOMP_ORIGINAL(0x00352800));
}

static void iu_pulse_decided(Context *ctx, const uint32_t *words) {
    (void)words;
    pulse_ai_decided(ctx);
}

/* AI::Processing(void): the real enemy phase, logged for comparison */
static void iu_real_mind(Context *ctx, const uint32_t *words) {
    (void)words;
    pulse_ai_decided(ctx); /* pulse */
    if (!iu_debug(2))
        return;
    uint32_t mind = mem_read32(ctx, MIND_PTR);
    if (!mind)
        return;
    uint32_t index = mem_read8(ctx, mind);
    uint32_t unit = unit_at_index(ctx, index);
    fprintf(stderr, "info-ui: enemy phase: unit %u at %u,%u: action 0x%02X move %d,%d target %u item %d trade %u\n", index,
            unit ? mem_read8(ctx, unit + 0xF4) : 0xFF, unit ? mem_read8(ctx, unit + 0xF5) : 0xFF, mem_read8(ctx, mind + 6),
            (int8_t)mem_read8(ctx, mind + 4), (int8_t)mem_read8(ctx, mind + 5), mem_read8(ctx, mind + 9),
            (int8_t)mem_read8(ctx, mind + 8), mem_read8(ctx, mind + 0xE));
}

RECOMP_OVERRIDE(0x002238CC) {
    /* for tests (debug bit 8) the prediction is also made as the enemy
       phase starts (the AI's first call, its state +0x20 still 0), from
       exactly the state the player ended the turn in: with Auto battle (no
       free cursor) every turn of a map is then checked */
    if (iu_debug(8) && info_ui_words[W_MODE] && ctx->read_pages != (uint8_t *const *)cow_read && mem_read8(ctx, ctx->r[0] + 0x20) == 0) {
        uint32_t sit = mem_read32(ctx, SITUATION_PTR);
        if (sit && mem_read8(ctx, sit + 8) == 1) {
            if (iu_debug(1))
                fprintf(stderr, "info-ui: the enemy phase begins: predicting from its first state\n");
            info_predict(ctx);
        }
    }
    if (iu_debug(2) && ctx->read_pages != (uint8_t *const *)cow_read) {
        CALL_THEN(RECOMP_ORIGINAL(0x002238CC), iu_real_mind, 0);
        return;
    }
    /* pulse: each real decision, once made (overrides/pulse.c) */
    if (pulse_words[0] && ctx->read_pages != (uint8_t *const *)cow_read) {
        CALL_THEN(RECOMP_ORIGINAL(0x002238CC), iu_pulse_decided, 0);
        return;
    }
    CALL(RECOMP_ORIGINAL(0x002238CC));
}

/* ---- drawing the lines ---------------------------------------------------- */

static uint32_t line_vb;     /* our VertexBuffer, made for the map's Panel */
static uint32_t line_panel;  /* the map::Panel it was made under */

typedef struct { float x, y, z; } V3;

static float tile_height(Context *ctx, int x, int y) {
    uint32_t world = mem_read32(ctx, FIELD_WORLD_PTR);
    uint32_t map = world ? mem_read32(ctx, world + 0x24) : 0;
    if (!map)
        return 0.0f;
    uint32_t cell = map + (uint32_t)y * 0x300 + (uint32_t)x * 0x18 + 0x3E0;
    float scale = f32_of(mem_read32(ctx, HEIGHT_SCALE));
    float sum = 0.0f;
    for (int v = 0; v < 4; v++)
        sum += (float)(int16_t)mem_read16(ctx, cell + 4 + 2 * v) * (1.0f / 64.0f) * scale;
    return sum * 0.25f;
}

typedef struct {
    uint32_t data;
    uint32_t capacity;
    uint32_t count;
    Context *ctx;
} Verts;

static void put_vertex(Verts *v, V3 p, float u, float t, uint32_t colour) {
    if (v->count >= v->capacity)
        return;
    uint32_t at = v->data + v->count * 0x18;
    mem_write32(v->ctx, at + 0x00, bits_of(p.x));
    mem_write32(v->ctx, at + 0x04, bits_of(p.y));
    mem_write32(v->ctx, at + 0x08, bits_of(p.z));
    mem_write32(v->ctx, at + 0x0C, bits_of(u));
    mem_write32(v->ctx, at + 0x10, bits_of(t));
    mem_write32(v->ctx, at + 0x14, colour);
    v->count++;
}

/* a quad as the map's panels give theirs: left far, left near, right far,
   right near; a colour each */
static void put_quad(Verts *v, V3 a0, V3 a1, V3 b0, V3 b1, uint32_t c_a0, uint32_t c_a1, uint32_t c_b0, uint32_t c_b1) {
    if (v->count + 4 > v->capacity)
        return;
    put_vertex(v, a0, 0.0f, 0.0f, c_a0);
    put_vertex(v, a1, 0.0f, 0.0f, c_a1);
    put_vertex(v, b0, 0.0f, 0.0f, c_b0);
    put_vertex(v, b1, 0.0f, 0.0f, c_b1);
}

static float clamp01(float a) { return a < 0.0f ? 0.0f : a > 1.0f ? 1.0f : a; }

static uint32_t with_alpha(uint32_t rgba, float a) {
    return (rgba & 0x00FFFFFFu) | (uint32_t)(clamp01(a) * 255.0f + 0.5f) << 24;
}

/* the rgb of a moved toward b's by k (a's alpha kept) */
static uint32_t mix_rgb(uint32_t a, uint32_t b, float k) {
    uint32_t out = a & 0xFF000000u;
    for (int s = 0; s < 24; s += 8) {
        float x = (float)(a >> s & 0xFF), y = (float)(b >> s & 0xFF);
        out |= (uint32_t)(x + (y - x) * clamp01(k) + 0.5f) << s;
    }
    return out;
}

static float smooth01(float e0, float e1, float x) {
    float t = clamp01((x - e0) / (e1 - e0));
    return t * t * (3.0f - 2.0f * t);
}

/* lines2: each line is a thin arc of light, drawn procedurally (no art): a
   parabola in the map's 3D space from the enemy to its target, lifted in
   the middle, as a ribbon that faces the camera and keeps its width on
   screen at every zoom. Across it are five vertices: a pale core in the
   middle, the pink-red at +-LINE_CORE (the anti-aliased edge) and nothing
   at +-LINE_GLOW (a soft glow). Along it, it fades in from the enemy, and a
   soft bright pulse runs to the target. Widths are in top-screen pixels
   (400 x 240; 6 window pixels each at 1440 lines). */
#define LINE_CORE 0.26f        /* half the bright line */
#define LINE_GLOW 0.85f        /* half the glow */
#define LINE_EDGE_ALPHA 0.50f  /* at +-LINE_CORE */
#define LINE_PINK 0xFF9A5AFFu  /* (255, 90, 154) as the colour table's bytes: R, G, B, A */

typedef struct {
    float vp[16];        /* the camera's view-projection (row-major, clip = VP * p) */
    float sy;            /* the projection's y scale (before the display rotation) */
    V3 eye;              /* the eye, or the view direction when orthographic */
    int ortho;
    uint32_t core, edge; /* colours */
} LineView;

static int invert44(const float *m, float *out) {
    float inv[16];
    inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15] + m[9] * m[7] * m[14] + m[13] * m[6] * m[11] - m[13] * m[7] * m[10];
    inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15] - m[8] * m[7] * m[14] - m[12] * m[6] * m[11] + m[12] * m[7] * m[10];
    inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15] + m[8] * m[7] * m[13] + m[12] * m[5] * m[11] - m[12] * m[7] * m[9];
    inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14] - m[8] * m[6] * m[13] - m[12] * m[5] * m[10] + m[12] * m[6] * m[9];
    inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15] - m[9] * m[3] * m[14] - m[13] * m[2] * m[11] + m[13] * m[3] * m[10];
    inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15] + m[8] * m[3] * m[14] + m[12] * m[2] * m[11] - m[12] * m[3] * m[10];
    inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15] - m[8] * m[3] * m[13] - m[12] * m[1] * m[11] + m[12] * m[3] * m[9];
    inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14] + m[8] * m[2] * m[13] + m[12] * m[1] * m[10] - m[12] * m[2] * m[9];
    inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15] + m[5] * m[3] * m[14] + m[13] * m[2] * m[7] - m[13] * m[3] * m[6];
    inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15] - m[4] * m[3] * m[14] - m[12] * m[2] * m[7] + m[12] * m[3] * m[6];
    inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15] + m[4] * m[3] * m[13] + m[12] * m[1] * m[7] - m[12] * m[3] * m[5];
    inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14] - m[4] * m[2] * m[13] - m[12] * m[1] * m[6] + m[12] * m[2] * m[5];
    inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11] - m[5] * m[3] * m[10] - m[9] * m[2] * m[7] + m[9] * m[3] * m[6];
    inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11] + m[4] * m[3] * m[10] + m[8] * m[2] * m[7] - m[8] * m[3] * m[6];
    inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11] - m[4] * m[3] * m[9] - m[8] * m[1] * m[7] + m[8] * m[3] * m[5];
    inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10] + m[4] * m[2] * m[9] + m[8] * m[1] * m[6] - m[8] * m[2] * m[5];
    float det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
    if (!(fabsf(det) > 1e-20f))
        return 0;
    for (int i = 0; i < 16; i++)
        out[i] = inv[i] / det;
    return 1;
}

/* the camera Route::Draw is given (an ICamera: +0xC8 the projection before
   the display rotation, +0x228 the view-projection prim draws with) */
static int line_view(Context *ctx, uint32_t camera, LineView *lv) {
    if (!camera)
        return 0;
    for (int i = 0; i < 16; i++)
        lv->vp[i] = f32_of(mem_read32(ctx, camera + 0x228 + 4 * i));
    lv->sy = f32_of(mem_read32(ctx, camera + 0xC8 + 4 * 5));
    if (!(fabsf(lv->sy) > 1e-6f))
        return 0;
    float inv[16];
    if (!invert44(lv->vp, inv))
        return 0;
    /* the eye is the point the projection sends to (0, 0, z, 0) */
    float ex = inv[2], ey = inv[6], ez = inv[10], ew = inv[14];
    if (fabsf(ew) > 1e-6f) {
        lv->eye = (V3){ ex / ew, ey / ew, ez / ew };
        lv->ortho = 0;
    } else {
        lv->eye = (V3){ ex, ey, ez };
        lv->ortho = 1;
    }
    return 1;
}

/* world units per top-screen pixel at p: the vertical scale, which the
   widescreen's wider field of view leaves as it is */
static float world_per_pixel(const LineView *lv, V3 p) {
    float w = lv->vp[12] * p.x + lv->vp[13] * p.y + lv->vp[14] * p.z + lv->vp[15];
    if (!(w > 1e-4f))
        w = 1e-4f;
    return 2.0f * w / (fabsf(lv->sy) * 240.0f);
}

static V3 v3_sub(V3 a, V3 b) { return (V3){ a.x - b.x, a.y - b.y, a.z - b.z }; }
static V3 v3_add(V3 a, V3 b) { return (V3){ a.x + b.x, a.y + b.y, a.z + b.z }; }
static V3 v3_scale(V3 a, float k) { return (V3){ a.x * k, a.y * k, a.z * k }; }
static V3 v3_cross(V3 a, V3 b) { return (V3){ a.y * b.z - a.z * b.y, a.z * b.x - a.x * b.z, a.x * b.y - a.y * b.x }; }

/* across the line on screen at p, for a line along tangent: one top-screen
   pixel long, at right angles to the tangent and the eye's ray */
static V3 screen_side(const LineView *lv, V3 p, V3 tangent) {
    V3 ray = lv->ortho ? lv->eye : v3_sub(p, lv->eye);
    V3 s = v3_cross(ray, tangent);
    float n = sqrtf(s.x * s.x + s.y * s.y + s.z * s.z);
    if (!(n > 1e-12f))
        return (V3){ 0, 0, 0 };
    return v3_scale(s, world_per_pixel(lv, p) / n);
}

typedef struct {
    V3 a, b;    /* the arc's ends */
    float apex; /* its lift in the middle */
} Arc;

static V3 arc_at(const Arc *arc, float t) {
    return (V3){ arc->a.x + (arc->b.x - arc->a.x) * t,
                 arc->a.y + (arc->b.y - arc->a.y) * t + arc->apex * 4.0f * t * (1.0f - t),
                 arc->a.z + (arc->b.z - arc->a.z) * t };
}

static V3 arc_tangent(const Arc *arc, float t) {
    return (V3){ arc->b.x - arc->a.x, arc->b.y - arc->a.y + arc->apex * 4.0f * (1.0f - 2.0f * t), arc->b.z - arc->a.z };
}

/* the five vertices across the arc at t */
static void arc_cross(const LineView *lv, const Arc *arc, float t, V3 out[5]) {
    V3 p = arc_at(arc, t);
    V3 side = screen_side(lv, p, arc_tangent(arc, t));
    out[0] = v3_add(p, v3_scale(side, LINE_GLOW));
    out[1] = v3_add(p, v3_scale(side, LINE_CORE));
    out[2] = p;
    out[3] = v3_sub(p, v3_scale(side, LINE_CORE));
    out[4] = v3_sub(p, v3_scale(side, LINE_GLOW));
}

/* the arc from tile a to tile b. sure: 1 when every run chose this target,
   less when the AI's coin decides; stack: how many lines drawn before aim
   at the same target (each arcs a little higher, so they stay apart);
   segments: the vertex budget's share */
static void put_line(Verts *v, Context *ctx, const LineView *lv, int ax, int ay, int bx, int by, float size, uint32_t frame,
                     float sure, int stack, int segments) {
    float ha = tile_height(ctx, ax, ay), hb = tile_height(ctx, bx, by);
    Arc arc;
    /* from the enemy's chest to the target's: the units, drawn after,
       stand over the ends */
    arc.a = (V3){ (ax + 0.5f) * size, (ha + 0.42f) * size, (ay + 0.5f) * size };
    arc.b = (V3){ (bx + 0.5f) * size, (hb + 0.36f) * size, (by + 0.5f) * size };
    float dx = arc.b.x - arc.a.x, dz = arc.b.z - arc.a.z;
    float len = sqrtf(dx * dx + dz * dz);
    if (len < 0.01f)
        return;
    arc.apex = (0.45f * size + 0.30f * len) * (1.0f + 0.16f * (float)stack);
    /* the pulse: a soft crest running from the enemy to its target */
    float phase = (float)(frame % 100u) / 100.0f;
    V3 prev[5];
    uint32_t prev_c[5];
    for (int i = 0; i <= segments; i++) {
        float t = (float)i / (float)segments;
        V3 cur[5];
        arc_cross(lv, &arc, t, cur);
        float wave = t * 0.7f - phase;
        wave -= floorf(wave);
        float crest = wave < 0.16f ? sinf(wave / 0.16f * 3.14159265f) : 0.0f;
        /* in from the enemy, a little softer onto the target */
        float along = (0.15f + 0.85f * smooth01(0.0f, 0.32f, t)) * (1.0f - 0.30f * smooth01(0.86f, 1.0f, t));
        float a = along * sure;
        uint32_t c[5];
        c[0] = with_alpha(lv->edge, 0.0f);
        c[1] = with_alpha(lv->edge, a * (LINE_EDGE_ALPHA + 0.20f * crest));
        c[2] = with_alpha(mix_rgb(lv->core, 0xFFFFFFFFu, 0.35f * crest), a);
        c[3] = c[1];
        c[4] = c[0];
        if (i > 0)
            for (int k = 0; k < 4; k++)
                put_quad(v, prev[k], prev[k + 1], cur[k], cur[k + 1], prev_c[k], prev_c[k + 1], c[k], c[k + 1]);
        memcpy(prev, cur, sizeof prev);
        memcpy(prev_c, c, sizeof prev_c);
    }
}

/* the decisions that draw a line: an attack on a unit of another force */
static int is_attack(Context *ctx, const uint32_t *d, uint32_t *attacker, uint32_t *target) {
    uint32_t a = unit_at_index(ctx, d[0] & 0xFF), t = unit_at_index(ctx, d[0] >> 8 & 0xFF);
    if (!a || !t || (d[0] >> 8 & 0xFF) == 0)
        return 0;
    uint32_t fa = unit_force(ctx, a), ft = unit_force(ctx, t);
    if (fa == ft || fa != 1 || ft == 1)
        return 0;
    if ((int8_t)mem_read8(ctx, t + 0xF4) < 0 || (int8_t)mem_read8(ctx, t + 0xF5) < 0) /* not on the map */
        return 0;
    *attacker = a;
    *target = t;
    return 1;
}

static void draw_lines(Context *ctx, uint32_t camera) {
    uint32_t panel = mem_read32(ctx, PANEL_PTR);
    if (!panel)
        return;
    uint32_t count = info_ui_words[W_DECISIONS];
    if (!count)
        return;
    /* the cursor's tile and the unit on it, for mode 2 */
    uint32_t cursor_unit = 0;
    int picking = mem_read32(ctx, FRAME_COUNTER) - info_ui_words[W_PICK_FRAME] <= 1u;
    if (info_ui_words[W_MODE] == 2) {
        uint32_t cursor = mem_read32(ctx, CURSOR_PTR), image = mem_read32(ctx, IMAGE_PTR);
        if (picking) {
            /* choosing where a unit moves: the lines aimed at it */
            cursor_unit = unit_at_index(ctx, info_ui_words[W_PICK_UNIT]);
        } else if (cursor && image) {
            int cx = (int)f32_of(mem_read32(ctx, cursor + 0x18)), cy = (int)f32_of(mem_read32(ctx, cursor + 0x1C));
            if (cx >= 0 && cy >= 0 && cx < 32 && cy < 32)
                cursor_unit = unit_at_index(ctx, mem_read8(ctx, image + 8 + ((uint32_t)cx | (uint32_t)cy << 5)));
        }
        if (!cursor_unit)
            return;
    }
    if (line_panel != panel || !line_vb) {
        /* a new map: a buffer of our own, two halves as the panels' */
        line_vb = 0;
        ctx->r[13] = (ctx->r[13] - 0x40) & ~7u;
        if (!gcall(ctx, F_OPERATOR_NEW, 0x18, 0, 0, 0) || !ctx->r[0])
            return;
        uint32_t vb = ctx->r[0];
        if (!gcall(ctx, F_VB_CTOR, vb, 0x1000, 2, 0))
            return;
        line_vb = vb;
        line_panel = panel;
    }
    /* VertexBuffer::Swap, then our vertices straight into its half */
    uint32_t halves = mem_read32(ctx, line_vb + 4);
    if (!halves)
        return;
    uint32_t index = (mem_read32(ctx, line_vb + 8) + 1) % halves;
    uint32_t array = mem_read32(ctx, line_vb + 0x10 + 4 * index);
    if (!array)
        return;
    mem_write32(ctx, line_vb + 8, index);
    mem_write32(ctx, line_vb + 0xC, array);
    Verts v = { mem_read32(ctx, array + 0xC), mem_read32(ctx, array + 0x10), 0, ctx };
    if (!v.data)
        return;
    float size = f32_of(mem_read32(ctx, PANEL_PTR + 0x34));
    uint32_t frame = mem_read32(ctx, FRAME_COUNTER);
    LineView lv;
    if (!line_view(ctx, camera, &lv))
        return;
    /* lines2: the attack panels' red, tinted toward a pink-red; the core
       paler still */
    uint32_t red = mem_read32(ctx, COLOUR_TABLE + 0x8C) | 0xFF000000u;
    lv.edge = mix_rgb(red, LINE_PINK, 0.65f);
    lv.core = mix_rgb(lv.edge, 0xFFFFFFFFu, 0.30f);
    /* the lines to draw first, then the vertex budget shared out */
    struct { int ax, ay, bx, by, stack; uint32_t target, i; float sure; } todo[MAX_DECISIONS];
    uint32_t lines = 0;
    for (uint32_t i = 0; i < count && i < MAX_DECISIONS; i++) {
        const uint32_t *d = &info_ui_words[W_DECISION0 + 4 * i];
        uint32_t attacker, target;
        if (!is_attack(ctx, d, &attacker, &target))
            continue;
        if (cursor_unit && cursor_unit != attacker && cursor_unit != target)
            continue;
        /* from where the enemy stands now (the copy moved it only there) */
        int ax = mem_read8(ctx, attacker + 0xF4), ay = mem_read8(ctx, attacker + 0xF5);
        int bx = mem_read8(ctx, target + 0xF4), by = mem_read8(ctx, target + 0xF5);
        /* the unit being moved, where the prediction stood it */
        if (info_ui_words[W_PLACED_UNIT] && (d[0] >> 8 & 0xFF) == info_ui_words[W_PLACED_UNIT]) {
            bx = info_ui_words[W_PLACED_XY] & 0xFF;
            by = info_ui_words[W_PLACED_XY] >> 8 & 0xFF;
        }
        uint32_t votes = d[3] >> 16 & 0xFF, runs = d[3] >> 24 & 0xFF;
        int stack = 0;
        for (uint32_t k = 0; k < lines; k++)
            if (todo[k].target == target)
                stack++;
        todo[lines].ax = ax;
        todo[lines].ay = ay;
        todo[lines].bx = bx;
        todo[lines].by = by;
        todo[lines].stack = stack;
        todo[lines].target = target;
        todo[lines].i = i;
        todo[lines].sure = runs ? 0.4f + 0.6f * (float)votes / (float)runs : 1.0f;
        lines++;
    }
    /* 16 vertices a segment (4 quads across); 24 segments a line at most,
       fewer when many lines share the buffer */
    int segments = lines ? (int)(v.capacity / 16u / lines) : 0;
    if (segments > 24)
        segments = 24;
    for (uint32_t k = 0; k < lines && segments >= 4; k++)
        put_line(&v, ctx, &lv, todo[k].ax, todo[k].ay, todo[k].bx, todo[k].by, size, frame + todo[k].i * 23, todo[k].sure,
                 todo[k].stack, segments);
    if (iu_debug(4) && lines) {
        /* where the first line's ends land in clip space, and the camera */
        V3 p = { (todo[0].ax + 0.5f) * size, 0.0f, (todo[0].ay + 0.5f) * size };
        float cx = lv.vp[0] * p.x + lv.vp[1] * p.y + lv.vp[2] * p.z + lv.vp[3];
        float cy = lv.vp[4] * p.x + lv.vp[5] * p.y + lv.vp[6] * p.z + lv.vp[7];
        float cw = lv.vp[12] * p.x + lv.vp[13] * p.y + lv.vp[14] * p.z + lv.vp[15];
        fprintf(stderr, "info-ui: lines2 camera: eye %.1f %.1f %.1f (%s), sy %f, %d segments, colours %08X %08X; tile %d,%d at ndc %.3f %.3f, %.4f world/px\n",
                lv.eye.x, lv.eye.y, lv.eye.z, lv.ortho ? "orthographic" : "perspective", lv.sy, segments, lv.edge, lv.core,
                todo[0].ax, todo[0].ay, cx / cw, cy / cw, world_per_pixel(&lv, p));
    }
    mem_write32(ctx, array + 0x14, v.count);
    info_ui_words[W_LINES] = lines;
    if (iu_debug(4))
        fprintf(stderr, "info-ui: %u lines, %u vertices, tile size %f, capacity %u\n", lines, v.count, size, v.capacity);
    if (!v.count)
        return;
    uint32_t offset = mem_read32(ctx, PANEL_PTR + 0x38);
    int ok = gcall(ctx, F_PUSH_STATE, 0, 0, 0, 0)
        && gcall(ctx, F_BEGIN_STATE, 0, 0, 0, 0)
        && gcall(ctx, F_SET_TARGET, 0, 0, 0, 0)
        && gcall(ctx, F_SET_CAMERA, camera, 0, 0, 0)
        && gcall(ctx, F_SET_PRIORITY, 0xFFFFFC1Cu, 0, 0, 0)
        && gcall(ctx, F_SET_SHAPE, 0, 0, 0, 0)
        /* lines2: no texture, the vertex colours alone */
        && gcall(ctx, F_SET_TEXTURE, 0, 0, 0, 0)
        /* ALWAYS (prim's TestFunc 1), no depth write: the arcs stay whole over
           walls and roofs; the units, drawn after, still stand over them */
        && gcall(ctx, F_SET_DEPTH_TEST, 1, 0, 0, 0);
    if (ok) {
        ctx->vfp[0] = offset;
        ok = gcall(ctx, F_SET_POLYGON_OFFSET, 0, 0, 0, 0)
            && gcall(ctx, F_END_STATE, 0, 0, 0, 0)
            && gcall(ctx, F_DRAW_VERTEX_BUFFER, line_vb, 0, 0, 0)
            && gcall(ctx, F_POP_STATE, 0, 0, 0, 0);
    }
    if (iu_debug(4)) {
        uint32_t danger = mem_read32(ctx, panel + 0xC), dvb = danger ? mem_read32(ctx, danger) : 0;
        uint32_t darr = dvb ? mem_read32(ctx, dvb + 0xC) : 0, ddata = darr ? mem_read32(ctx, darr + 0xC) : 0;
        fprintf(stderr, "info-ui: drew %u lines, %u vertices, ok %d, size %f; ours %f %f %f uv %f %f col %08X; danger's (%u) %f %f %f uv %f %f col %08X\n",
                lines, v.count, ok, size, f32_of(mem_read32(ctx, v.data)), f32_of(mem_read32(ctx, v.data + 4)), f32_of(mem_read32(ctx, v.data + 8)),
                f32_of(mem_read32(ctx, v.data + 12)), f32_of(mem_read32(ctx, v.data + 16)), mem_read32(ctx, v.data + 20),
                darr ? mem_read32(ctx, darr + 0x14) : 0,
                ddata ? f32_of(mem_read32(ctx, ddata)) : 0.0f, ddata ? f32_of(mem_read32(ctx, ddata + 4)) : 0.0f, ddata ? f32_of(mem_read32(ctx, ddata + 8)) : 0.0f,
                ddata ? f32_of(mem_read32(ctx, ddata + 12)) : 0.0f, ddata ? f32_of(mem_read32(ctx, ddata + 16)) : 0.0f, ddata ? mem_read32(ctx, ddata + 20) : 0);
    }
}

/* map::Panel::Route::Draw(const ICamera*) */
RECOMP_OVERRIDE(0x0038FC5C) {
    uint32_t mode = info_ui_words[W_MODE];
    if (mode && info_ui_words[W_DECISIONS]) {
        uint32_t frame = mem_read32(ctx, FRAME_COUNTER);
        if (frame - info_ui_words[W_FREE_FRAME] <= 1u) {
            Saved saved;
            save(ctx, &saved);
            draw_lines(ctx, saved.r[1]);
            int lost = ctx->exit != 0;
            restore(ctx, &saved);
            if (lost && iu_debug(1))
                fprintf(stderr, "info-ui: drawing the lines left for the host\n");
        }
    }
    CALL(RECOMP_ORIGINAL(0x0038FC5C));
}

/* map::Panel::Finalize(void): the map ends, so does our buffer */
RECOMP_OVERRIDE(0x00394848) {
    if (line_vb) {
        uint32_t vb = line_vb;
        line_vb = 0;
        line_panel = 0;
        Saved saved;
        save(ctx, &saved);
        ctx->r[13] = (ctx->r[13] - 0x40) & ~7u;
        gcall(ctx, F_VB_DELETE, vb, 0, 0, 0);
        restore(ctx, &saved);
    }
    cache_clear();
    predicted_sit = 0;
    CALL(RECOMP_ORIGINAL(0x00394848));
}

/* ---- growth rates ----------------------------------------------------------- */

/* ViewerUnitStatus::Draw(void) */
RECOMP_OVERRIDE(0x001F580C) {
    if (info_ui_words[W_GROWTH]) {
        uint32_t viewer = ctx->r[0];
        uint32_t unit = mem_read32(ctx, viewer + 8);
        info_ui_words[W_STATUS_DRAWS]++;
        info_ui_words[W_STATUS_UNIT] = unit;
        /* a help box (a skill's, an item's) is up: drawn by InfoHelp (this
           +0x188) while this +0x184 is 0, and only with an item (+0x10) */
        info_ui_words[W_STATUS_HELP] = mem_read8(ctx, viewer + 0x184) == 0 && mem_read32(ctx, viewer + 0x188 + 0x10) != 0;
        if (unit && mem_read32(ctx, unit + 0x9C)) {
            Saved saved;
            save(ctx, &saved);
            ctx->r[13] = (ctx->r[13] - 0x40) & ~7u;
            int ok = 1;
            for (uint32_t i = 0; i < 8 && ok; i++) {
                ok = gcall(ctx, F_GET_GROW, unit, i, 0, 0);
                if (ok)
                    info_ui_words[W_GROWTH_RATES + i] = ctx->r[0];
            }
            restore(ctx, &saved);
            info_ui_words[W_GROWTH_UNIT] = ok ? unit : 0;
        }
    }
    CALL(RECOMP_ORIGINAL(0x001F580C));
}

/* pulsefix: what of this file is the console's state, kept with Divine
   Pulse's moments and saved states (overrides/keep.c) */
void info_ui_keep(void (*add)(const char *, void *, uint32_t)) {
    add("info_ui.line_vb", &line_vb, sizeof line_vb);
    add("info_ui.line_panel", &line_panel, sizeof line_panel);
}
