/* drawdist: Full Draw Distance. Grass and the field's objects drawn
 * wherever they are, at their full detail, instead of popping in as the
 * camera nears them.
 *
 * ---- grass (FieldPlant) ----
 *
 * A field's grass is a FieldPlant (FieldWorldImpl +0x14), read from
 * bp/<field>.bin.lz: a 48x48 grid of 50-unit cells, each listing its tufts
 * (6 bytes: x and z in 3.125-unit steps from the cell's corner, a height,
 * the kind, a sun byte). Up to 8 kinds, each a PlantFactory (+0x1E8) with a
 * small pool of PlantModels (a "low" and a "high" model each).
 * FieldPlant::OnUpdate (0x001762BC) visits only the 11x11 cells round the
 * eye; a cell within the "grass draw distance" (GP, 350) of the eye and in
 * the camera's view takes models from the pools, one per tuft, and gives
 * them back when it leaves the view or the distance. The pools are tiny:
 * My Castle has 716 tufts in 333 cells, pools of 30/12/3/5/1/1/1 (cap 128),
 * so a cell often gets only some of its tufts, or none, and the rest never
 * show until a walk lets the pools go round. Within the "grass LOD
 * distance" (150) a model is switched to its high model (and swayed by the
 * wind); farther ones keep whichever model they last had.
 *
 * With Full Draw Distance on (drawdist_full, set by the host from
 * pc.full_draw_distance) the game's own bookkeeping is left exactly as it is
 * (OnUpdate is the game's: same cells, same pools, same draws on the
 * system random generator, which the save holds), and the drawing is ours.
 * FieldPlant::OnDrawOpa (0x00176ADC), overridden, draws every tuft of the
 * field (but those in cells the game leaves bare on purpose, FieldArea
 * +0x534's mask over the map's 32x32 tiles) at its high model, the game's
 * models not drawn (their tufts are among ours):
 *   - the tufts by kind, light (the sun byte in DD_LEVELS steps near,
 *     DD_FAR_LEVELS far, where a step of light is not seen) and
 *     distance (nearer or farther than DD_FAR), each group one draw: a
 *     model of the kind lent for it (the free one on top of its pool, else
 *     its first, put back where it was after) placed at the group's first
 *     tuft, lit and swayed as the game would, its skeleton updated
 *     (PlantModel::Update) and drawn (Model::DrawOpa), with the group's other
 *     tufts in drawdist_batch: the renderer makes that draw an instanced
 *     one (source/gfx/src/drawdist.rs), each tuft at its place with its own
 *     wind. Far groups are alpha-tested at one sample a pixel (MSAA).
 *   - cells out of the camera's view widened by about 24 degrees a side
 *     are left out (the camera's own and neighbouring cells never), so a
 *     turn, even a fast mouse turn, and the pictures in between never show
 *     grass arrive; a batch is a "loose" draw for the pictures in between
 *     (matched by program and state), so its tufts changing is no cut.
 *   - FATES_DRAWDIST_NEAR=r draws the tufts in view nearer than r one by one
 *     instead (dev: the exact look to compare batches with).
 * Prologue battle field: 3,969 tufts in about 80 draws, 4-7 ms of the game's
 * thread a frame (the game's own 128 models cost about as much).
 *
 * The guest calls are chained: each returns into this override (r14 =
 * 0x00176ADC) with r11 = DD_MAGIC, which the callees keep (it is
 * callee-saved), and the work's state lives in a record on the guest stack
 * below the caller's frame, so a call that leaves for the host (the budget,
 * a saved state) carries on where it was when it comes back.
 *
 * Off, everything is the game's. FATES_DRAWDIST=0/1 overrides the setting
 * (tests); FATES_DRAWDIST_LOG=1 logs the grass per field and frame.
 *
 * ---- the field's objects ----
 *
 * FieldData::Tick's distances (low detail, fade) are made unreachable in
 * overrides/cameras.c (gfxcastle's free-camera rule, now for every camera
 * while drawdist_full is set). */
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <x86intrin.h>
#include "overrides.h"

/* drawdist: Full Draw Distance, the host's word (1 on, 0 the game's own) */
uint32_t drawdist_full = 1;
/* drawdist: tufts drawn by the last frame's FieldPlant draws (lent models), for the host's stats */
uint32_t drawdist_stamps = 0;

static int dd_log = -1;
/* FATES_DRAWDIST_LOG: time spent in the grass draws (TSC, read as 3.2 GHz) */
static unsigned long long dd_cycles, dd_draws, dd_stamps_sum, dd_upd, dd_drw, dd_gather, dd_batches, dd_near;
static int logging_on(void) {
    if (dd_log < 0) dd_log = getenv("FATES_DRAWDIST_LOG") != 0;
    return dd_log;
}

int drawdist_on(void) {
    static int env = -2;
    if (env == -2) {
        const char *v = getenv("FATES_DRAWDIST");
        env = v ? (strcmp(v, "0") != 0) : -1;
    }
    if (env >= 0) return env;
    return *(volatile uint32_t *)&drawdist_full != 0;
}

static float rf(Context *ctx, uint32_t a) { return vfp_from_s(mem_read32(ctx, a)); }
static void wf(Context *ctx, uint32_t a, float v) {
    uint32_t bits;
    memcpy(&bits, &v, 4);
    mem_write32(ctx, a, bits);
}

#define ON_DRAW_OPA      0x00176ADCu /* FieldPlant::OnDrawOpa(RenderState&) */
#define PLANT_UPDATE     0x0041D5D0u /* PlantModel::Update(const ICamera*) */
#define MODEL_DRAW_OPA   0x0041D85Cu /* Model::DrawOpa(RenderState&, const MTX34&) */
#define FIELD_VIEWING    0x006BE3C0u /* byte: 1 in the 3D views */
#define FIELD_WORLD_PTR  0x006BE404u /* *: FieldWorld(Impl); +0x18 FieldArea (GetArea) */

#define DD_MAGIC 0xD7A3D157u /* r11 while a chained call is out */
#define DD_SIG   0x44524157u /* "WARD": the record's first word */
#define CELLS    48
#define MAX_STAMPS 6000

/* the record on the guest stack (sp while the work runs) */
enum {
    R_SIG = 0x00, R_LR = 0x04, R_R11 = 0x08, R_FP = 0x0C, R_RS = 0x10, R_CAM = 0x14,
    R_PHASE = 0x18, R_I = 0x1C, R_K = 0x20, R_SUB = 0x24, R_LENT = 0x28, R_COUNT = 0x2C,
    R_TYPES = 0x30, /* 8 x 10 words: model, translate x y z, sun low high, the high model's wind x y z */
    R_TYPE_SIZE = 0x28,
    R_SIZE = 0x30 + 8 * 0x28,
};
enum { P_GATHER, P_STAMPS, P_BATCHES, P_RESTORE, P_DONE };

/* drawdist: the batch the renderer turns the carrier's draw into
 * (source/gfx/src/drawdist.rs `Batch`, registered by main.rs) */
#define DD_BATCH_MAX 4096
typedef struct {
    uint32_t active, count, wind_reg, far;
    float inst[DD_BATCH_MAX][8]; /* offset x y z, 0, wind x y z, 0 */
} DrawdistBatch;
DrawdistBatch drawdist_batch;

/* the far tufts by kind and light (sun byte in DD_LEVELS steps), and the
 * near ones drawn one by one: cell << 4 | tuft, worked out each draw */
#define DD_LEVELS 4
#define DD_FAR_LEVELS 2
#define DD_GROUPS (8 * DD_LEVELS * 2) /* kind, then light near, or light far */
#define DD_FAR 300.0f /* far tufts are alpha-tested at a sample a pixel */
#define DD_GROUP_MAX 8192
static uint32_t group_ref[DD_GROUPS][DD_GROUP_MAX];
static uint32_t group_n[DD_GROUPS];
static uint32_t near_ref[MAX_STAMPS];
static uint32_t near_n;

/* the field's grass data */
typedef struct {
    uint32_t fp, hdr, table, area, wind;
    float base, yscale, sun_lo, sun_step;
    uint32_t factory[8];
} Plants;

static int plants_get(Context *ctx, uint32_t fp, Plants *p) {
    memset(p, 0, sizeof *p);
    p->fp = fp;
    if (!mem_read8(ctx, fp + 0x248)) return 0;
    uint32_t file = mem_read32(ctx, fp + 0x1A4);
    uint32_t d0 = file ? mem_read32(ctx, file + 0x70) : 0;
    if (!d0) return 0;
    p->hdr = d0 + 0x20;
    p->table = mem_read32(ctx, p->hdr + 0x10);
    if (!p->table) return 0;
    p->base = rf(ctx, p->hdr + 8);
    p->yscale = rf(ctx, p->hdr + 0xC);
    uint32_t fw = mem_read32(ctx, FIELD_WORLD_PTR);
    p->area = fw ? mem_read32(ctx, fw + 0x18) : 0;
    p->wind = mem_read32(ctx, fp + 0x1B8);
    p->sun_lo = rf(ctx, fp + 0x1B0);
    p->sun_step = (rf(ctx, fp + 0x1B4) - p->sun_lo) * (1.0f / 255.0f);
    for (int t = 0; t < 8; t++) p->factory[t] = mem_read32(ctx, fp + 0x1E8 + t * 4);
    return 1;
}

/* the cells the game has models in, this frame: their records */
static void active_cells(Context *ctx, uint32_t fp, uint32_t *active) {
    memset(active, 0, CELLS * CELLS * sizeof *active);
    uint32_t cell = mem_read32(ctx, fp + 0x1C4);
    for (int guard = 0; cell && guard < 256; guard++) {
        int x = (int8_t)mem_read8(ctx, cell + 0x28), z = (int8_t)mem_read8(ctx, cell + 0x29);
        if (x >= 0 && x < CELLS && z >= 0 && z < CELLS) active[z * CELLS + x] = cell;
        cell = mem_read32(ctx, cell + 4);
    }
}

/* the view test: the camera's six planes (ICamera::IsReject's), the four
 * through the eye widened by a margin of about 24 degrees (a fast mouse
 * turn is 21 degrees a tick), so a cell enters the lists before it can be
 * seen, in a frame or in the pictures in between */
typedef struct {
    float n[6][3], d[6], margin[6];
    int corner[6][3];
    float eye[3];
} View;

static void view_get(Context *ctx, uint32_t cam, View *v) {
    for (int k = 0; k < 3; k++) v->eye[k] = rf(ctx, cam + 4 + k * 4);
    for (int i = 0; i < 6; i++) {
        for (int k = 0; k < 3; k++) {
            v->n[i][k] = rf(ctx, cam + 0x478 + i * 0xC + k * 4);
            v->corner[i][k] = mem_read8(ctx, cam + 0x520 + i + k * 6) ? 1 : 0;
        }
        v->d[i] = rf(ctx, cam + 0x508 + i * 4);
        float at_eye = v->n[i][0] * v->eye[0] + v->n[i][1] * v->eye[1] + v->n[i][2] * v->eye[2] + v->d[i];
        v->margin[i] = fabsf(at_eye) < 1.0f ? 0.4f : 0.0f;
    }
}

/* nonzero when the box (min, max) is out of the widened view */
static int view_rejects(const View *v, const float *lo, const float *hi) {
    for (int i = 0; i < 6; i++) {
        float p[3], dist2 = 0.0f;
        for (int k = 0; k < 3; k++) {
            p[k] = v->corner[i][k] ? hi[k] : lo[k];
            float c = (lo[k] + hi[k]) * 0.5f - v->eye[k];
            dist2 += c * c;
        }
        float s = v->n[i][0] * p[0] + v->n[i][1] * p[1] + v->n[i][2] * p[2] + v->d[i];
        if (s > 0.0f && (v->margin[i] == 0.0f || s > v->margin[i] * sqrtf(dist2))) return 1;
    }
    return 0;
}

/* the cell's box as FieldPlant::OnUpdate makes it */
static void cell_box(const Plants *p, Context *ctx, uint32_t data, int x, int z, float *lo, float *hi) {
    float k = p->yscale * 250.0f;
    lo[0] = (float)x * 50.0f - 403.0f;
    lo[2] = (float)z * 50.0f - 403.0f;
    hi[0] = (float)x * 50.0f - 347.0f;
    hi[2] = (float)z * 50.0f - 347.0f;
    lo[1] = p->base + (float)mem_read8(ctx, data) * k;
    hi[1] = p->base + (float)mem_read8(ctx, data + 1) * k + 4.0f;
}

/* whether the cell is one the game leaves bare (FieldPlant::OnUpdate's mask) */
static int cell_masked(Context *ctx, const Plants *p, int x, int z) {
    if (!p->area || x < 8 || z < 8 || x >= 40 || z >= 40) return 0;
    int bit = (x - 8) + (z - 8) * 32;
    return (mem_read8(ctx, p->area + 0x534 + (bit >> 3)) >> (bit & 7)) & 1;
}

/* a cell this frame's draw adds to: tufts, not bare, in view (a cell the
 * game shows is in its view) */
static uint32_t stamp_cell(Context *ctx, const Plants *p, const uint32_t *active, const View *v, int i) {
    uint32_t data = mem_read32(ctx, p->table + i * 4);
    if (!data || !mem_read8(ctx, data + 2)) return 0;
    if (active[i]) return data;
    int x = i % CELLS, z = i / CELLS;
    if (cell_masked(ctx, p, x, z)) return 0;
    float lo[3], hi[3];
    cell_box(p, ctx, data, x, z, lo, hi);
    /* the tufts stand up to a few units out of the cell's box */
    lo[0] -= 8.0f, lo[2] -= 8.0f, hi[0] += 8.0f, hi[2] += 8.0f, hi[1] += 16.0f;
    if (view_rejects(v, lo, hi)) return 0;
    return data;
}

static int no_wind(void);
static void set_wind(Context *ctx, uint32_t model, uint32_t wind, uint32_t phase) {
    if (!wind) return;
    if (no_wind()) {
        uint32_t cur = mem_read32(ctx, model + 0x30), extra = cur ? mem_read32(ctx, cur + 4) : 0;
        if (extra)
            for (int c = 0; c < 4; c++) mem_write32(ctx, extra + 4 + c * 4, 0);
        return;
    }
    uint32_t idx = (phase + mem_read32(ctx, wind)) & 0x3F;
    uint32_t src = wind + 4 + idx * 12;
    uint32_t cur = mem_read32(ctx, model + 0x30);
    uint32_t extra = cur ? mem_read32(ctx, cur + 4) : 0;
    if (!extra) return;
    mem_write32(ctx, extra + 4, mem_read32(ctx, src));
    mem_write32(ctx, extra + 8, mem_read32(ctx, src + 4));
    mem_write32(ctx, extra + 0xC, mem_read32(ctx, src + 8));
    mem_write32(ctx, extra + 0x10, 0);
}

static void set_sun(Context *ctx, uint32_t model, uint32_t colour) {
    uint32_t a = mem_read32(ctx, model + 0x38), b = mem_read32(ctx, model + 0xBC);
    if (a) mem_write32(ctx, a + 0x14, colour);
    if (b) mem_write32(ctx, b + 0x14, colour);
}

static uint32_t sun_colour(const Plants *p, uint32_t sun) {
    float s = p->sun_lo + p->sun_step * (float)sun;
    uint32_t c = (uint32_t)(int)(s * 255.0f) & 0xFF;
    return c | c << 8 | c << 16 | 0xFF000000u;
}

/* a cell's wind phase: the game draws one at random for its cells; ours
 * come from the cell, so the system random generator is not touched */
static uint32_t cell_phase(int i) {
    uint32_t h = (uint32_t)i * 2654435761u;
    return (h >> 13) & 0x3F;
}

/* where a tuft stands (FieldPlant::OnUpdate's placing) */
static void tuft_pos(Context *ctx, const Plants *p, uint32_t cell, uint32_t k, float *pos) {
    uint32_t data = mem_read32(ctx, p->table + cell * 4), e = data + 4 + k * 6;
    float cx = (float)(int)(cell % CELLS) * 50.0f - 403.0f, cz = (float)(int)(cell / CELLS) * 50.0f - 403.0f;
    pos[0] = cx + (float)mem_read8(ctx, e) * 3.125f;
    pos[1] = p->base + (float)mem_read16(ctx, e + 2) * p->yscale;
    pos[2] = cz + (float)mem_read8(ctx, e + 1) * 3.125f;
}

/* the wind generator's velocity for a phase (as set_wind) */
/* dev: FATES_DRAWDIST_NOWIND=1, still grass (to compare pictures) */
static int no_wind(void) {
    static int on = -1;
    if (on < 0) {
        const char *v = getenv("FATES_DRAWDIST_NOWIND");
        on = v && strcmp(v, "0") != 0;
    }
    return on;
}

static void wind_of(Context *ctx, uint32_t wind, uint32_t phase, float *out) {
    out[0] = out[1] = out[2] = 0.0f;
    if (!wind || no_wind()) return;
    uint32_t idx = (phase + mem_read32(ctx, wind)) & 0x3F;
    for (int c = 0; c < 3; c++) out[c] = rf(ctx, wind + 4 + idx * 12 + c * 4);
}

/* the tufts nearer than this to the eye are drawn one by one, exactly as
 * the game would (their own light and billboards); the rest in batches */
static float near_radius(void) {
    static float r = -1.0f;
    if (r < 0.0f) {
        const char *v = getenv("FATES_DRAWDIST_NEAR");
        r = v ? (float)atof(v) : 0.0f;
    }
    return r;
}

/* a model of kind t lent for the draw: the kind's free model on top of its
 * pool, else its first one; where it was is kept in the record */
static uint32_t lend(Context *ctx, uint32_t rec, const Plants *p, uint32_t t) {
    if (t >= 8 || !p->factory[t]) return 0;
    uint32_t slot = rec + R_TYPES + t * R_TYPE_SIZE;
    uint32_t m = mem_read32(ctx, slot);
    if (m) return m;
    uint32_t f = p->factory[t];
    if ((int32_t)mem_read32(ctx, f + 4) < 1) return 0;
    uint32_t top = mem_read32(ctx, f + 0x10);
    m = top ? mem_read32(ctx, top) : 0;
    if (!m) m = mem_read32(ctx, f + 8);
    if (!m) return 0;
    mem_write32(ctx, slot, m);
    mem_write32(ctx, slot + 4, mem_read32(ctx, m + 0xC));
    mem_write32(ctx, slot + 8, mem_read32(ctx, m + 0x1C));
    mem_write32(ctx, slot + 0xC, mem_read32(ctx, m + 0x2C));
    uint32_t a = mem_read32(ctx, m + 0x38), b = mem_read32(ctx, m + 0xBC);
    mem_write32(ctx, slot + 0x10, a ? mem_read32(ctx, a + 0x14) : 0);
    mem_write32(ctx, slot + 0x14, b ? mem_read32(ctx, b + 0x14) : 0);
    for (int w = 0; w < 3; w++) mem_write32(ctx, slot + 0x18 + w * 4, b ? mem_read32(ctx, b + 4 + w * 4) : 0);
    mem_write32(ctx, rec + R_LENT, mem_read32(ctx, rec + R_LENT) | 1u << t);
    if (logging_on()) {
        static uint32_t told;
        uint32_t hi = m + 0xB8;
        if (!(told >> t & 1)) {
            told |= 1u << t;
            fprintf(stderr, "[drawdist] kind %u: high model %08X skeleton flags %02X (billboard %d), bones %08X\n", t, hi, mem_read8(ctx, hi + 0x24), mem_read8(ctx, hi + 0x24) & 1, mem_read32(ctx, hi + 0x2C));
            uint32_t sk = hi + 0x24, nb = mem_read32(ctx, sk + 8), arr = mem_read32(ctx, sk + 0xC);
            for (uint32_t b = 0; b < nb && b < 8; b++) fprintf(stderr, "[drawdist]   bone %u flags %08X mode %u\n", b, mem_read32(ctx, arr + b * 0x40), (mem_read32(ctx, arr + b * 0x40) >> 16) & 7);
        }
    }
    return m;
}

/* this draw's lists: every tuft of the field the game does not show
 * itself, except in bare cells; the near ones in view one by one, the rest
 * by kind and light */
static void gather(Context *ctx, const Plants *p, uint32_t rec, uint32_t fp, uint32_t cam) {
    static uint32_t active[CELLS * CELLS];
    View v;
    active_cells(ctx, fp, active);
    view_get(ctx, cam, &v);
    float r = near_radius(), r2 = r * r;
    near_n = 0;
    memset(group_n, 0, sizeof group_n);
    for (uint32_t i = 0; i < CELLS * CELLS; i++) {
        uint32_t data = mem_read32(ctx, p->table + i * 4);
        if (!data) continue;
        uint32_t n = mem_read8(ctx, data + 2);
        if (!n || cell_masked(ctx, p, (int)(i % CELLS), (int)(i / CELLS))) continue;
        /* cells out of the widened view are left out; the camera's own
         * cell and its neighbours always count (the boxes are the grid's) */
        int ci = (int)(i % CELLS), cz = (int)(i / CELLS);
        int ex = (int)((v.eye[0] + 403.0f) / 50.0f), ez = (int)((v.eye[2] + 403.0f) / 50.0f);
        int close = ci >= ex - 1 && ci <= ex + 1 && cz >= ez - 1 && cz <= ez + 1;
        if (!close && !stamp_cell(ctx, p, active, &v, (int)i)) continue;
        int in_view = 1;
        for (uint32_t k = 0; k < n && k < 16; k++) {
            uint32_t e = data + 4 + k * 6, t = mem_read8(ctx, e + 4);
            if (t >= 8 || !p->factory[t]) continue;
            float pos[3];
            tuft_pos(ctx, p, i, k, pos);
            float dx = pos[0] - v.eye[0], dy = pos[1] - v.eye[1], dz = pos[2] - v.eye[2];
            if (dx * dx + dy * dy + dz * dz < r2) {
                if (in_view && near_n < MAX_STAMPS) near_ref[near_n++] = i << 4 | k;
                continue;
            }
            uint32_t far = dx * dx + dy * dy + dz * dz > DD_FAR * DD_FAR;
            uint32_t g = t * DD_LEVELS * 2 + (far ? DD_LEVELS + mem_read8(ctx, e + 5) * DD_FAR_LEVELS / 256 : mem_read8(ctx, e + 5) * DD_LEVELS / 256);
            if (group_n[g] < DD_GROUP_MAX) group_ref[g][group_n[g]++] = i << 4 | k;
        }
    }
}

/* a model's mesh boxes (PlantModel::Update: per node group at +0x5C, the
 * resource's counts at +0x20 -> +0x68) made to cover everything */
static void unbox(Context *ctx, uint32_t model) {
    uint32_t groups = mem_read32(ctx, model + 0x60), boxes = mem_read32(ctx, model + 0x5C), res = mem_read32(ctx, model + 0x20);
    if (!boxes || !res || groups > 64) return;
    for (uint32_t i = 0; i < groups; i++) {
        uint32_t arr = mem_read32(ctx, boxes + i * 8), n = mem_read32(ctx, mem_read32(ctx, res + 0x68) + i * 0x1C + 4);
        for (uint32_t u = 0; arr && u < n && u < 256; u++)
            for (int c = 0; c < 6; c++) wf(ctx, arr + u * 0x18 + c * 4, c < 3 ? -1.0e8f : 1.0e8f);
    }
}

/* a guest call returning into this override */
static void chained(Context *ctx, uint32_t code, uint32_t r0, uint32_t r1, uint32_t r2) {
    ctx->r[0] = r0;
    ctx->r[1] = r1;
    ctx->r[2] = r2;
    ctx->r[11] = DD_MAGIC;
    ctx->r[14] = ON_DRAW_OPA;
    ctx->r[15] = code & ~1u;
    ctx->thumb = code & 1;
    CALL(recomp_call);
}

static void finish(Context *ctx, uint32_t rec) {
    uint32_t lr = mem_read32(ctx, rec + R_LR);
    uint32_t count = mem_read32(ctx, rec + R_COUNT);
    ctx->r[11] = mem_read32(ctx, rec + R_R11);
    mem_write32(ctx, rec + R_SIG, 0);
    ctx->r[13] = rec + R_SIZE;
    drawdist_stamps = count;
    if (logging_on()) {
        dd_draws++;
        dd_stamps_sum += count;
        if (dd_draws % 60 == 0) {
            fprintf(stderr, "[drawdist] grass: %u tufts added this draw; last 60 draws %.1f tufts, %.3f ms each (updates %.3f, draws %.3f)\n", count,
                    dd_stamps_sum / 60.0, dd_cycles / 60.0 / 3.2e6, dd_upd / 60.0 / 3.2e6, dd_drw / 60.0 / 3.2e6);
            fprintf(stderr, "[drawdist]   gather %.3f ms, near %.1f, batches %.1f\n", dd_gather / 60.0 / 3.2e6, dd_near / 60.0, dd_batches / 60.0);
            dd_gather = dd_near = dd_batches = 0;
            dd_cycles = dd_stamps_sum = dd_upd = dd_drw = 0;
        }
    }
    RETURN_TO(lr);
}

/* the work, from wherever the record says */
static void run(Context *ctx, uint32_t rec) {
    uint32_t fp = mem_read32(ctx, rec + R_FP), rs = mem_read32(ctx, rec + R_RS), cam = mem_read32(ctx, rec + R_CAM);
    Plants p;
    if (!plants_get(ctx, fp, &p)) {
        mem_write32(ctx, rec + R_PHASE, P_DONE);
    }
    for (;;) {
        uint32_t phase = mem_read32(ctx, rec + R_PHASE);
        if (phase == P_GATHER) {
            unsigned long long tg = __rdtsc();
            gather(ctx, &p, rec, fp, cam);
            dd_gather += __rdtsc() - tg;
            dd_near += near_n;
            mem_write32(ctx, rec + R_PHASE, P_STAMPS);
            continue;
        }
        if (phase == P_STAMPS) {
            /* the near tufts, one draw each: R_I the next of the list */
            uint32_t i = mem_read32(ctx, rec + R_I), sub = mem_read32(ctx, rec + R_SUB);
            uint32_t count = mem_read32(ctx, rec + R_COUNT);
            for (; i < near_n; i++, sub = 0) {
                uint32_t cell = near_ref[i] >> 4, k = near_ref[i] & 15;
                uint32_t data = mem_read32(ctx, p.table + cell * 4), e = data + 4 + k * 6;
                uint32_t t = mem_read8(ctx, e + 4);
                uint32_t m = lend(ctx, rec, &p, t);
                if (!m) continue;
                if (sub == 0) {
                    float pos[3];
                    tuft_pos(ctx, &p, cell, k, pos);
                    wf(ctx, m + 0xC, pos[0]);
                    wf(ctx, m + 0x1C, pos[1]);
                    wf(ctx, m + 0x2C, pos[2]);
                    set_sun(ctx, m, sun_colour(&p, mem_read8(ctx, e + 5)));
                    mem_write32(ctx, m + 0x30, m + 0xB8);
                    set_wind(ctx, m, p.wind, cell_phase((int)cell));
                    mem_write32(ctx, rec + R_I, i);
                    mem_write32(ctx, rec + R_SUB, 1);
                    unsigned long long t0 = __rdtsc();
                    chained(ctx, PLANT_UPDATE, m, cam, 0);
                    if (ctx->exit) return;
                    dd_upd += __rdtsc() - t0;
                }
                count++;
                mem_write32(ctx, rec + R_COUNT, count);
                mem_write32(ctx, rec + R_I, i + 1);
                mem_write32(ctx, rec + R_SUB, 0);
                unsigned long long t1 = __rdtsc();
                chained(ctx, MODEL_DRAW_OPA, mem_read32(ctx, m + 0x30), rs, m);
                if (ctx->exit) return;
                dd_drw += __rdtsc() - t1;
            }
            mem_write32(ctx, rec + R_PHASE, P_BATCHES);
            mem_write32(ctx, rec + R_I, 0);
            mem_write32(ctx, rec + R_K, 0);
            mem_write32(ctx, rec + R_SUB, 0);
            continue;
        }
        if (phase == P_BATCHES) {
            /* the rest by kind and light, one instanced draw a batch: R_I
             * the group, R_K the batch's first tuft in it, R_SUB the step */
            uint32_t g = mem_read32(ctx, rec + R_I), k = mem_read32(ctx, rec + R_K), sub = mem_read32(ctx, rec + R_SUB);
            if (sub == 2) {
                /* back from the batch's draw */
                drawdist_batch.active = 0;
                k += DD_BATCH_MAX;
                sub = 0;
            }
            while (g < DD_GROUPS) {
                uint32_t t = g / (DD_LEVELS * 2), far = g % (DD_LEVELS * 2) >= DD_LEVELS;
                uint32_t levels = far ? DD_FAR_LEVELS : DD_LEVELS, level = g % (DD_LEVELS * 2) - (far ? DD_LEVELS : 0);
                uint32_t m = k < group_n[g] ? lend(ctx, rec, &p, t) : 0;
                if (!m) {
                    g++, k = 0, sub = 0;
                    continue;
                }
                uint32_t n = group_n[g] - k;
                if (n > DD_BATCH_MAX) n = DD_BATCH_MAX;
                /* the carrier stands at the field's corner, the same for
                 * every batch and frame: a picture in between, which
                 * matches a batch with one of the frame before by program
                 * and state, then moves only the view, never a batch's
                 * tufts from one carrier's place to another's */
                float p0[3] = {-403.0f, p.base, -403.0f};
                uint32_t r0 = group_ref[g][k];
                if (sub == 0) {
                    wf(ctx, m + 0xC, p0[0]);
                    wf(ctx, m + 0x1C, p0[1]);
                    wf(ctx, m + 0x2C, p0[2]);
                    set_sun(ctx, m, sun_colour(&p, (2 * level + 1) * 128 / levels));
                    mem_write32(ctx, m + 0x30, m + 0xB8);
                    set_wind(ctx, m, p.wind, cell_phase((int)(r0 >> 4)));
                    mem_write32(ctx, rec + R_I, g);
                    mem_write32(ctx, rec + R_K, k);
                    mem_write32(ctx, rec + R_SUB, 1);
                    chained(ctx, PLANT_UPDATE, m, cam, 0);
                    if (ctx->exit) return;
                }
                /* the carrier stands for the whole field: its meshes' boxes
                 * (PlantModel::Update's) cover everything, so Model::DrawOpa
                 * culls none of them by the view */
                unbox(ctx, mem_read32(ctx, m + 0x30));
                /* the instances: where each stands from the carrier, its wind */
                uint32_t cur = mem_read32(ctx, m + 0x30), extra = cur ? mem_read32(ctx, cur + 4) : 0;
                drawdist_batch.count = n;
                drawdist_batch.far = far;
                drawdist_batch.wind_reg = extra ? mem_read8(ctx, extra + 0x1A) : 0xFFu;
                for (uint32_t j = 0; j < n; j++) {
                    uint32_t r = group_ref[g][k + j];
                    float q[3];
                    tuft_pos(ctx, &p, r >> 4, r & 15, q);
                    float *o = drawdist_batch.inst[j];
                    o[0] = q[0] - p0[0], o[1] = q[1] - p0[1], o[2] = q[2] - p0[2];
                    o[3] = 0.0f;
                    wind_of(ctx, p.wind, cell_phase((int)(r >> 4)), o + 4);
                    o[7] = 0.0f;
                }
                drawdist_batch.active = 1;
                dd_batches++;
                mem_write32(ctx, rec + R_COUNT, mem_read32(ctx, rec + R_COUNT) + n);
                mem_write32(ctx, rec + R_I, g);
                mem_write32(ctx, rec + R_K, k);
                mem_write32(ctx, rec + R_SUB, 2);
                unsigned long long t1 = __rdtsc();
                chained(ctx, MODEL_DRAW_OPA, cur, rs, m);
                if (ctx->exit) return;
                dd_drw += __rdtsc() - t1;
                drawdist_batch.active = 0;
                k += DD_BATCH_MAX;
                sub = 0;
            }
            mem_write32(ctx, rec + R_PHASE, P_RESTORE);
            mem_write32(ctx, rec + R_I, 0);
            continue;
        }
        if (phase == P_RESTORE) {
            uint32_t t = mem_read32(ctx, rec + R_I);
            uint32_t lent = mem_read32(ctx, rec + R_LENT);
            for (; t < 8; t++) {
                if (!(lent >> t & 1)) continue;
                uint32_t slot = rec + R_TYPES + t * R_TYPE_SIZE;
                uint32_t m = mem_read32(ctx, slot);
                mem_write32(ctx, m + 0xC, mem_read32(ctx, slot + 4));
                mem_write32(ctx, m + 0x1C, mem_read32(ctx, slot + 8));
                mem_write32(ctx, m + 0x2C, mem_read32(ctx, slot + 0xC));
                uint32_t a = mem_read32(ctx, m + 0x38), b = mem_read32(ctx, m + 0xBC);
                if (a) mem_write32(ctx, a + 0x14, mem_read32(ctx, slot + 0x10));
                if (b) mem_write32(ctx, b + 0x14, mem_read32(ctx, slot + 0x14));
                /* it keeps the high model (a model the game shows has it from
                 * step 1; a free one then shows it when the game takes it) */
                mem_write32(ctx, m + 0x30, m + 0xB8);
                if (b)
                    for (int w = 0; w < 3; w++) mem_write32(ctx, b + 4 + w * 4, mem_read32(ctx, slot + 0x18 + w * 4));
                mem_write32(ctx, rec + R_I, t + 1);
                chained(ctx, PLANT_UPDATE, m, cam, 0);
                if (ctx->exit) return;
            }
            mem_write32(ctx, rec + R_PHASE, P_DONE);
            continue;
        }
        finish(ctx, rec);
        return;
    }
}

/* FieldPlant::OnDrawOpa(RenderState&) */
static void run_timed(Context *ctx, uint32_t rec) {
    if (!logging_on()) {
        run(ctx, rec);
        return;
    }
    unsigned long long t0 = __rdtsc();
    run(ctx, rec);
    dd_cycles += __rdtsc() - t0;
}

RECOMP_OVERRIDE(0x00176ADC) {
    uint32_t sp = ctx->r[13];
    if (ctx->r[11] == DD_MAGIC && mem_read32(ctx, sp + R_SIG) == DD_SIG) {
        /* back from a chained call */
        run_timed(ctx, sp);
        return;
    }
    uint32_t fp = ctx->r[0], rs = ctx->r[1];
    uint32_t cam = rs ? mem_read32(ctx, rs + 8) : 0;
    if (!drawdist_on() || !cam || !mem_read8(ctx, fp + 0x248) || (mem_read8(ctx, fp + 0x14) & 1) || !mem_read8(ctx, FIELD_VIEWING)) {
        CALL(RECOMP_ORIGINAL(0x00176ADC));
        return;
    }
    uint32_t rec = sp - R_SIZE;
    for (uint32_t o = 0; o < R_SIZE; o += 4) mem_write32(ctx, rec + o, 0);
    mem_write32(ctx, rec + R_SIG, DD_SIG);
    mem_write32(ctx, rec + R_LR, ctx->r[14]);
    mem_write32(ctx, rec + R_R11, ctx->r[11]);
    mem_write32(ctx, rec + R_FP, fp);
    mem_write32(ctx, rec + R_RS, rs);
    mem_write32(ctx, rec + R_CAM, cam);
    mem_write32(ctx, rec + R_PHASE, P_GATHER);
    ctx->r[13] = rec;
    run_timed(ctx, rec);
}

/* FieldPlant::OnUpdate(RenderState&): the game's; FATES_DRAWDIST_LOG=1 logs
 * the field's grass once and its pools now and then */
RECOMP_OVERRIDE(0x001762BC) {
    if (logging_on()) {
        static uint32_t seen;
        static int frames;
        uint32_t fp = ctx->r[0];
        Plants p;
        if (fp != seen && plants_get(ctx, fp, &p)) {
            seen = fp;
            int per[256] = {0}, cells = 0, total = 0, most = 0, bare = 0;
            for (int i = 0; i < CELLS * CELLS; i++) {
                uint32_t c = mem_read32(ctx, p.table + i * 4);
                if (!c) continue;
                int n = mem_read8(ctx, c + 2);
                if (cell_masked(ctx, &p, i % CELLS, i / CELLS)) {
                    bare += n;
                    continue;
                }
                cells++;
                total += n;
                if (n > most) most = n;
                for (int k = 0; k < n; k++) per[mem_read8(ctx, c + 4 + k * 6 + 4)]++;
            }
            fprintf(stderr, "[drawdist] grass %08X: cap %d, %d cells, %d tufts (most %d in a cell, %d more in bare cells), pool/tufts by kind", fp,
                    (int16_t)mem_read16(ctx, p.hdr + 6), cells, total, most, bare);
            for (int t = 0; t < 8; t++) fprintf(stderr, " %d/%d", mem_read8(ctx, p.hdr + 0x54 + t), per[t]);
            fprintf(stderr, "\n");
        }
        if ((++frames % 60) == 0) {
            uint32_t cam = mem_read32(ctx, ctx->r[1] + 8);
            fprintf(stderr, "[drawdist] game's grass: %d models in %d cells, eye %.0f %.0f %.0f, viewing %d\n", (int)mem_read32(ctx, fp + 0x1A8),
                    (int)mem_read32(ctx, fp + 0x1CC), rf(ctx, cam + 4), rf(ctx, cam + 8), rf(ctx, cam + 12), mem_read8(ctx, FIELD_VIEWING));
        }
    }
    CALL(RECOMP_ORIGINAL(0x001762BC));
}
