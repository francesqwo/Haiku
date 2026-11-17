/* cameras: a free camera for the battle scenes, and walking in My Castle's
 * third-person view.
 *
 * Both are off unless the host says otherwise (fates_cameras_hook, set by
 * Haiku's main to zakuro::cameras::game_hook). With them off, or with no
 * hook, every override below runs the game's own function and nothing else.
 *
 * ---- 1. The battle free camera -------------------------------------------
 *
 * The 3D field (battles, My Castle's third-person view, cutscenes) is drawn
 * through FieldWorld's camera, *(*0x006BE404 + 0x34), a FieldCamera. Every
 * frame the battle's camera controller (BattleCamera::Update, 0x001A9924)
 * copies the scripted CameraParam into it and calls its ICamera::Update
 * (vtable slot 0, 0x004DBB50), which makes the view, the projection, the
 * stereo matrices and the culling frustum from the eye, target and up at
 * +0x04/+0x10/+0x1C. A camera animation (CamAnim at +0x5C8, FieldCamera::
 * Tick) puts them at +0x618/+0x624/+0x630 instead.
 *
 * The override of ICamera::Update, for that camera while a battle runs (a
 * live "BattleSeq" proc), puts the free camera's eye, target and up in
 * those fields, runs the game's Update, and puts the game's values back
 * (CALL_THEN). So everything the game derives from the camera for the
 * frame follows the free camera: what is drawn, the culling of units, cast
 * and field (ICamera::IsReject, BattleUnit/CastManager::
 * UpdateCameraFrustumVisibility test the planes UpdateFrustum makes), the
 * 2D placed over 3D (GetWorldToScreen reads the view-projection), and
 * widescreen.c's wider projection. The scripted camera itself is never
 * touched: it goes on underneath, and the next frame without the free
 * camera is the game's again.
 *
 * The free camera orbits a centre: the middle of the two fighters' focus
 * points, as BattleCamera::UpdateUnitPos smooths them (BattleCamera
 * +0x88 + 12 i, the units at BattleWorld +8 + 4 i), plus a pan. It starts
 * where the game's camera is at the moment the player first moves it, so
 * nothing jumps, and a reset (or a new battle) gives the game's camera back.
 *
 * ---- 2. Walking in My Castle's third-person view -------------------------
 *
 * In free roam, castle::Sequence::TickMoveUnit (0x00498A78) moves Corrin with
 * castle::Actor::MovePlayer (0x0048F8A4): the circle pad or D-pad (Pad::
 * GetSumX/Y, which read the D-pad bits of +0x0C and the stick at +0x04/+0x08
 * of the Pad *0x006BD96C), the castle's collision (castle::ColsEnumerate:
 * every actor, area object and building, then the field's collision tree
 * and height map), the footstep sounds and dust, and castle::Actor::SetPos.
 * L opens castle::ProcSubjectView, the third-person view, drawn by the
 * battle engine: the people near Corrin are made battle units (Load), the
 * camera becomes a SubjectCamera (Encount), and ProcSubjectView::WaitTick
 * (0x00459248) runs each frame until A, B or Start. WaitTick only turns
 * Corrin's head and body toward the camera; SubjectCamera::Tick turns the
 * camera with the circle pad (CameraInput::Tick, 0x00194178). Nothing moves
 * Corrin: the free roam's tick does not run under the view.
 *
 * With walking on, WaitTick first calls MovePlayer itself, with the stick
 * turned from the camera's frame to the castle's, so Corrin walks with the
 * castle's own movement and collision, and the castle actor is where the
 * battle unit walks to (so the free roam goes on from there afterwards).
 * Then the battle unit, the SubjectCamera's centre (+0xC8) and FieldWorld's
 * focus point (+0x50C0, which SubjectCamera::Enter sets) follow, the unit
 * plays the walk ("歩き") or run ("Run") animation, and the game's WaitTick
 * runs as before. The camera is turned with the right stick and the mouse
 * instead (CameraInput::Tick, for the SubjectCamera's input only, reads them
 * in place of the circle pad).
 *
 * - The view loads the subject animations only (BattleUnitResName::
 *   ClearExceptSubjectAnims, 0x001F9400): for Corrin (its call from Load at
 *   0x00458E1C) the walk and run animations are kept too.
 * - The view shows only the people within 200 units of where it opened; the
 *   castle's collision would still stop Corrin at the others, unseen. While
 *   Corrin walks, ColsArray::Add (0x004FC9DC) leaves out castle actors the
 *   view does not show.
 * - The guest calls (MovePlayer, the action state machine's clear,
 *   BattleUnit::PlayAnim, AnimClip::IndexByName) return into the override's
 *   own address, which goes on from a per-thread record (mods.c's pattern),
 *   so a call that leaves for the host loses nothing.
 */
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "overrides.h"

/* ---- the host ---- */

enum {
    CAM_QUERY_BATTLE = 0x10, /* only: is the free camera on */
    CAM_QUERY_CASTLE = 0x20, /* only: is walking on */
    CAM_BATTLE = 1,          /* a battle scene is drawn this frame: input */
    CAM_CASTLE = 2,          /* the castle's third-person view: input */
    CAM_CASTLE_ROAM = 3,     /* qol5: My Castle, walking around (free roam or the view): the free camera's input */
    CAM_CASTLE_IDLE = 4,     /* qol5: My Castle, but not walking around (a menu, a building, a scene) */
};

typedef struct {
    uint32_t size;    /* sizeof(HaikuCamFrame) */
    uint32_t context; /* in: CAM_* */
    uint32_t enabled; /* out: the feature for the context is on */
    uint32_t reset;   /* out: give the game's camera back */
    float orbit[2];   /* out: radians, x around (yaw), y up (pitch) */
    float pan[2];     /* out: in view heights, x right, y up */
    float zoom;       /* out: steps, + nearer */
    float look[2];    /* out: castle: the right stick, -1..1, y up */
    float turn[2];    /* out: castle: mouse turns, radians */
    uint32_t log;     /* out: FATES_CAMERAS_LOG */
    uint32_t mode;    /* out: battle: CAM_FLY or CAM_ORBIT */
    float move[3];    /* out: battle fly: seconds of full input right, up, ahead */
    float dolly;      /* out: battle fly: wheel steps ahead */
} HaikuCamFrame;

enum { CAM_ORBIT = 0, CAM_FLY = 1 };

void (*fates_cameras_hook)(HaikuCamFrame *frame) = 0;

static int ask(HaikuCamFrame *frame, uint32_t context) {
    memset(frame, 0, sizeof *frame);
    frame->size = sizeof *frame;
    frame->context = context;
    if (!fates_cameras_hook) return 0;
    fates_cameras_hook(frame);
    return frame->enabled != 0;
}

static float rf(Context *ctx, uint32_t a) { return vfp_from_s(mem_read32(ctx, a)); }
static void wf(Context *ctx, uint32_t a, float v) {
    uint32_t bits;
    memcpy(&bits, &v, 4);
    mem_write32(ctx, a, bits);
}

static int logging;

static float env_float_once(const char *name, float fallback) {
    static int read;
    static float value;
    if (!read) {
        const char *v = getenv(name);
        value = v ? (float)atof(v) : fallback;
        read = 1;
    }
    return value;
}

/* ---- the game's globals (USA Special Edition v2) ---- */
#define FIELD_WORLD_PTR  0x006BE404u /* *: FieldWorld; +0x34 its camera, +0x50C0 focus point */
#define BATTLE_WORLD_PTR 0x006BC2F8u /* *: BattleWorld; +4 the BattleCamera (CameraStateMachine), +8.. 4 units */
#define PAD_PTR          0x006BD96Cu /* *: Pad; +4 stick x, +8 stick y, +0xC held, +0x14 pressed */
#define MAIN_SEQ_PTR     0x006D1B70u /* *: castle::MainSeq; +0x74 the actors, linked at +0x10 */
#define PROC_ROOTS       0x00752B48u /* 3 proc trees */
#define SUBJECT_CAMERA_VT 0x0068F0E4u

static uint32_t field_camera(Context *ctx) {
    uint32_t fw = mem_read32(ctx, FIELD_WORLD_PTR);
    return fw ? mem_read32(ctx, fw + 0x34) : 0;
}

/* CameraStateMachine::GetCurrentState */
static uint32_t current_state(Context *ctx, uint32_t machine) {
    uint32_t state = mem_read32(ctx, machine + 8);
    for (int guard = 0; state && guard < 64; guard++) {
        if (mem_read8(ctx, state + 0x10) != 2 && mem_read8(ctx, state + 0x12) == 0) return state;
        state = mem_read32(ctx, state + 8);
    }
    return 0;
}

static uint32_t battle_camera(Context *ctx) {
    uint32_t bw = mem_read32(ctx, BATTLE_WORLD_PTR);
    return bw ? mem_read32(ctx, bw + 4) : 0;
}

/* a proc's name hash, as ProcInst::Create keeps it at +0x2C */
static uint32_t name_hash(const char *name) {
    uint32_t h = 0;
    for (; *name; name++) h = h * 31u + (uint32_t)(int32_t)(signed char)*name;
    return h ? h : 1;
}

/* the live proc of that name, 0 for none: the proc trees depth first
 * (node +0x14 first child, +0x1C next sibling, +0x28 bit 0 dying) */
static uint32_t find_proc(Context *ctx, uint32_t hash) {
    uint32_t stack[64];
    int depth = 0, seen = 0;
    for (int root = 0; root < 3; root++) {
        uint32_t proc = mem_read32(ctx, PROC_ROOTS + 4 * root);
        if (proc) stack[depth++] = proc;
        while (depth > 0 && seen < 4096) {
            uint32_t p = stack[--depth];
            seen++;
            if (mem_read32(ctx, p + 0x2C) == hash && !(mem_read32(ctx, p + 0x28) & 1)) return p;
            uint32_t next = mem_read32(ctx, p + 0x1C), child = mem_read32(ctx, p + 0x14);
            if (next && depth < 64) stack[depth++] = next;
            if (child && depth < 64) stack[depth++] = child;
        }
        depth = 0;
    }
    return 0;
}

typedef struct { float x, y, z; } V3;
static V3 v3(float x, float y, float z) { V3 v = {x, y, z}; return v; }
static V3 rv(Context *ctx, uint32_t a) { return v3(rf(ctx, a), rf(ctx, a + 4), rf(ctx, a + 8)); }
static void wv(Context *ctx, uint32_t a, V3 v) { wf(ctx, a, v.x); wf(ctx, a + 4, v.y); wf(ctx, a + 8, v.z); }
static V3 add(V3 a, V3 b) { return v3(a.x + b.x, a.y + b.y, a.z + b.z); }
static V3 sub(V3 a, V3 b) { return v3(a.x - b.x, a.y - b.y, a.z - b.z); }
static V3 mul(V3 a, float s) { return v3(a.x * s, a.y * s, a.z * s); }
static float len(V3 a) { return sqrtf(a.x * a.x + a.y * a.y + a.z * a.z); }
static V3 cross(V3 a, V3 b) { return v3(a.y * b.z - a.z * b.y, a.z * b.x - a.x * b.z, a.x * b.y - a.y * b.x); }
static V3 norm(V3 a) { float l = len(a); return l > 1e-6f ? mul(a, 1.0f / l) : v3(0, 0, 0); }

/* ======================== 1. the battle free camera ======================== */

static struct {
    uint32_t battle;   /* the BattleSeq proc it was set up in */
    int engaged;
    float yaw, pitch, dist, dist0;
    V3 pan;            /* the centre's offset from the fighters' middle */
    int enabled;       /* the host's last answer */
    uint32_t mode;     /* the style it was engaged in */
    uint32_t handed;   /* cameras3: the battle whose comeback the free camera handed over to */
    int hand_n;        /* cameras3: handing over to the comeback: updates so far */
    V3 hand_eye, hand_target; /* cameras3: the free view it hands over from */
    V3 shown_eye;      /* cameras3: the eye the free camera showed last */
    V3 shown_target;   /* qol5: and where it looked */
    int shown;
    int bypass;        /* cameras3: the game's own view for one update (CalcComebackCamera) */
    V3 pos;            /* fly: where the camera is */
    float look_yaw, look_pitch; /* fly: where it looks */
} fc;

/* the saved fields of the camera while its Update runs */
static struct { uint32_t cam, base; uint32_t words[9]; uint8_t mode; int bind; uint32_t far_at, far; } fc_saved;

/* cameras3: the battle's way back to the map. BattleSeq::PlayComebackCamera
 * makes a SilentComebackCamera (a ComebackCamera, a TransitionCamera) the
 * battle camera's state: from the field camera's CameraParam as it is (+4,
 * which the free camera never keeps: the game's is put back after each
 * Update) it blends, along the comeback curve, to the map's camera
 * (BattleUtil::CalcComebackCamera). With the free camera engaged, its view
 * went on over that zoom out until the battle's proc ended, then the game's
 * camera came back half way through it: "not zoom out at all", then a jump.
 * Now the free camera hands over as the comeback starts: from its last
 * view to the game's (which is then the comeback's, zooming out), eased
 * over HAND_FRAMES updates, after which it lets go and does not engage
 * again in that battle. (Writing the free view into the comeback's starting
 * CameraParam instead was tried: CameraParam::BlendParam blends the views'
 * Euler angles (GfxUtil::GetRotRad), and from a free view they could take
 * the long way round, rolling the picture on its side.) */
#define COMEBACK_CAMERA_VT        0x0068F438u
#define SILENT_COMEBACK_CAMERA_VT 0x00692C68u
static uint32_t comeback_state(Context *ctx);
#define HAND_FRAMES 36
/* qol5: fc.battle while My Castle's free camera has it (no proc is at 1) */
#define CASTLE_MARK 1u

/* the middle of the fighters' focus points, as the battle camera smooths
 * them; the game's target when there are none */
static V3 fighters(Context *ctx, V3 fallback) {
    uint32_t bw = mem_read32(ctx, BATTLE_WORLD_PTR);
    uint32_t bc = bw ? mem_read32(ctx, bw + 4) : 0;
    if (!bw || !bc) return fallback;
    V3 sum = v3(0, 0, 0);
    int n = 0;
    for (int i = 0; i < 2; i++) {
        if (!mem_read32(ctx, bw + 8 + 4 * i)) continue;
        V3 p = rv(ctx, bc + 0x88 + 12 * i);
        if (p.x == 0.0f && p.y == 0.0f && p.z == 0.0f) continue;
        sum = add(sum, p);
        n++;
    }
    return n ? mul(sum, 1.0f / (float)n) : fallback;
}

static void fc_restore(Context *ctx, const uint32_t *words) {
    uint32_t cam = words[0];
    if (cam != fc_saved.cam) return;
    for (int i = 0; i < 9; i++) mem_write32(ctx, fc_saved.base + 4 * i, fc_saved.words[i]);
    if (!fc_saved.bind) mem_write8(ctx, cam + 0x40, fc_saved.mode);
    if (fc_saved.far_at) mem_write32(ctx, fc_saved.far_at, fc_saved.far);
    fc_saved.cam = 0;
}

/* the free camera's eye, target and up into the camera, its Update, and
 * the game's put back */
static void apply_view(Context *ctx, uint32_t cam, int bind, uint32_t base, V3 eye, V3 target, float far_scale) {
    fc_saved.cam = cam;
    fc_saved.base = base;
    fc_saved.bind = bind;
    for (int i = 0; i < 9; i++) fc_saved.words[i] = mem_read32(ctx, base + 4 * i);
    fc_saved.mode = mem_read8(ctx, cam + 0x40);
    fc_saved.far_at = 0;
    if (far_scale > 1.0f) {
        /* far from the fight the far plane goes back with the camera */
        fc_saved.far_at = cam + (bind ? 0x648 : 0x34);
        fc_saved.far = mem_read32(ctx, fc_saved.far_at);
        wf(ctx, fc_saved.far_at, rf(ctx, fc_saved.far_at) * far_scale);
    }
    wv(ctx, base, eye);
    wv(ctx, base + 12, target);
    wv(ctx, base + 24, v3(0, 1, 0));
    if (!bind) mem_write8(ctx, cam + 0x40, 0); /* look at the target */
    /* cameras3: the comeback has started: the free camera hands over from
     * this view (see comeback_state); qol5: battles only (My Castle's free
     * camera hands over when its walking around ends) */
    if (fc.engaged && !fc.hand_n && fc.battle != CASTLE_MARK && comeback_state(ctx)) {
        fc.hand_eye = eye;
        fc.hand_target = target;
        fc.hand_n = 1;
        if (logging)
            fprintf(stderr, "[cameras] the battle's comeback starts: the free camera eases into it from eye %.1f %.1f %.1f target %.1f %.1f %.1f\n", eye.x,
                    eye.y, eye.z, target.x, target.y, target.z);
    }
    fc.shown_eye = eye;
    fc.shown_target = target;
    fc.shown = 1;
    ctx->r[0] = cam;
    CALL_THEN(RECOMP_ORIGINAL(0x004DBB50), fc_restore, cam);
}

/* the fly camera: it starts where the game's camera is, looking where it
 * looks; the left stick moves it ahead, back and sideways along the view,
 * the shoulders or triggers up and down, the right stick or the mouse turn
 * it, the wheel moves it ahead in steps. Its speed grows with its distance
 * from the fight, so it goes far out quickly. */
/* cameras3: the battle's backdrop is the field's sky dome (FieldSky,
 * "sky/Btl_...": a 512x512 panorama with no smaller sizes). The fly camera
 * goes where the player likes (the user asked for no limits: pitch +-86
 * degrees and 60 times its start distance, as before); instead the dome is
 * centred on the free camera's eye (FieldSky::OnUpdate below), as it is on
 * the game's, so its horizon stays at eye level and the camera never leaves
 * it. */
static void fly(Context *ctx, uint32_t cam, int bind, uint32_t base, V3 eye, V3 target, V3 middle, const HaikuCamFrame *frame, int moved) {
    if (!fc.engaged && moved && (fc.battle == CASTLE_MARK || (fc.handed != fc.battle && !comeback_state(ctx)))) {
        V3 d = norm(sub(target, eye));
        if (len(d) < 0.5f) d = v3(0, 0, -1);
        fc.pos = eye;
        fc.look_yaw = atan2f(d.x, d.z);
        fc.look_pitch = asinf(fmaxf(-1.0f, fminf(1.0f, d.y)));
        fc.dist0 = fmaxf(len(sub(eye, middle)), 10.0f);
        fc.engaged = 1;
        if (logging)
            fprintf(stderr, "[cameras] fly camera engaged: eye %.1f %.1f %.1f target %.1f %.1f %.1f fighters %.1f %.1f %.1f bind %d\n",
                    eye.x, eye.y, eye.z, target.x, target.y, target.z, middle.x, middle.y, middle.z, bind);
    }
    if (!fc.engaged) {
        CALL(RECOMP_ORIGINAL(0x004DBB50));
        return;
    }
    fc.look_yaw -= frame->orbit[0];
    fc.look_pitch += frame->orbit[1];
    if (fc.look_pitch < -1.5f) fc.look_pitch = -1.5f;
    if (fc.look_pitch > 1.5f) fc.look_pitch = 1.5f;
    V3 dir = v3(cosf(fc.look_pitch) * sinf(fc.look_yaw), sinf(fc.look_pitch), cosf(fc.look_pitch) * cosf(fc.look_yaw));
    V3 right = norm(cross(dir, v3(0, 1, 0)));
    V3 up = cross(right, dir);
    float away = len(sub(fc.pos, middle));
    /* units a second */
    float speed = env_float_once("FATES_CAMERAS_FLY", 1.0f) * (0.6f * fc.dist0 + 0.7f * away);
    fc.pos = add(fc.pos, mul(right, frame->move[0] * speed));
    fc.pos = add(fc.pos, mul(v3(0, 1, 0), frame->move[1] * speed));
    fc.pos = add(fc.pos, mul(dir, frame->move[2] * speed));
    fc.pos = add(fc.pos, mul(dir, frame->dolly * fmaxf(0.15f * away, 0.1f * fc.dist0)));
    if (frame->pan[0] != 0.0f || frame->pan[1] != 0.0f) {
        float fovy = bind ? rf(ctx, cam + 0x63C) : rf(ctx, cam + 0x28);
        if (!(fovy > 0.01f && fovy < 3.0f)) fovy = 0.6f;
        float height = 2.0f * fmaxf(away, fc.dist0) * tanf(fovy * 0.5f);
        fc.pos = add(fc.pos, add(mul(right, -frame->pan[0] * height), mul(up, frame->pan[1] * height)));
    }
    /* no further than 60 times its start from the fight */
    V3 off = sub(fc.pos, middle);
    if (len(off) > 60.0f * fc.dist0) fc.pos = add(middle, mul(norm(off), 60.0f * fc.dist0));
    away = len(sub(fc.pos, middle));
    float look = fmaxf(away, fc.dist0);
    float far_scale = away > 2.0f * fc.dist0 ? (away + 4.0f * fc.dist0) / (3.0f * fc.dist0) : 1.0f;
    apply_view(ctx, cam, bind, base, fc.pos, add(fc.pos, mul(dir, look)), far_scale);
}

/* the orbit camera (Free Camera Style: Orbit) round `middle`: it starts
 * where the game's camera is at the first input (qol5: moved out of the
 * override, so My Castle's free camera orbits too) */
static void orbit(Context *ctx, uint32_t cam, int bind, uint32_t base, V3 eye, V3 target, V3 middle, const HaikuCamFrame *frame, int moved) {
    if (!fc.engaged && moved && (fc.battle == CASTLE_MARK || (fc.handed != fc.battle && !comeback_state(ctx)))) {
        V3 v = sub(eye, target);
        float d = len(v);
        if (d < 1e-3f) d = 1e-3f;
        fc.dist = fc.dist0 = d;
        fc.pitch = asinf(fmaxf(-1.0f, fminf(1.0f, v.y / d)));
        fc.yaw = atan2f(v.x, v.z);
        fc.pan = sub(target, middle);
        fc.engaged = 1;
        if (logging)
            fprintf(stderr, "[cameras] free camera engaged: eye %.1f %.1f %.1f target %.1f %.1f %.1f fighters %.1f %.1f %.1f bind %d\n",
                    eye.x, eye.y, eye.z, target.x, target.y, target.z, middle.x, middle.y, middle.z, bind);
    }
    if (!fc.engaged) {
        CALL(RECOMP_ORIGINAL(0x004DBB50));
        return;
    }
    /* the player's input */
    fc.yaw -= frame->orbit[0];
    fc.pitch += frame->orbit[1];
    if (fc.pitch < -0.3f) fc.pitch = -0.3f;
    if (fc.pitch > 1.45f) fc.pitch = 1.45f;
    fc.dist *= powf(0.88f, frame->zoom);
    if (fc.dist < fc.dist0 * 0.2f) fc.dist = fc.dist0 * 0.2f;
    if (fc.dist > fc.dist0 * 3.0f) fc.dist = fc.dist0 * 3.0f;
    V3 dir = v3(cosf(fc.pitch) * sinf(fc.yaw), sinf(fc.pitch), cosf(fc.pitch) * cosf(fc.yaw));
    if (frame->pan[0] != 0.0f || frame->pan[1] != 0.0f) {
        V3 forward = mul(dir, -1.0f);
        V3 right = norm(cross(forward, v3(0, 1, 0)));
        V3 up = cross(right, forward);
        float fovy = bind ? rf(ctx, cam + 0x63C) : rf(ctx, cam + 0x28);
        if (!(fovy > 0.01f && fovy < 3.0f)) fovy = 0.6f;
        float height = 2.0f * fc.dist * tanf(fovy * 0.5f);
        fc.pan = add(fc.pan, add(mul(right, -frame->pan[0] * height), mul(up, frame->pan[1] * height)));
        float most = fc.dist0 * 2.0f;
        if (len(fc.pan) > most) fc.pan = mul(norm(fc.pan), most);
    }
    V3 centre = add(middle, fc.pan);
    V3 new_eye = add(centre, mul(dir, fc.dist));
    /* the game's are put back once Update has made the matrices */
    apply_view(ctx, cam, bind, base, new_eye, centre, 1.0f);
}

static uint32_t comeback_state(Context *ctx) {
    uint32_t bc = battle_camera(ctx);
    uint32_t state = bc ? current_state(ctx, bc) : 0;
    uint32_t vt = state ? mem_read32(ctx, state) : 0;
    return vt == COMEBACK_CAMERA_VT || vt == SILENT_COMEBACK_CAMERA_VT ? state : 0;
}

static void castle_frame(Context *ctx);

/* cameras3: FATES_CAMERAS_FIELDLOG=1, a debugging aid: the field's objects
 * (FieldData +0x58, linked at +0xC) whose level (+0x59: which of their
 * models is attached) or fade (+0xCC wanted, +0xD0 now) changed since the
 * last camera update, with where the camera is */
static void field_log(Context *ctx) {
    static int on = -1;
    static uint32_t seen_obj[512];
    static uint32_t seen_state[512];
    static int frames;
    if (on < 0) on = getenv("FATES_CAMERAS_FIELDLOG") != 0;
    if (!on) return;
    uint32_t fw = mem_read32(ctx, FIELD_WORLD_PTR);
    uint32_t data = fw ? mem_read32(ctx, fw + 8) : 0;
    if (!data) return;
    frames++;
    uint32_t cam = mem_read32(ctx, fw + 0x34);
    V3 eye = rv(ctx, cam + 4), p68 = rv(ctx, cam + 0x68), focus = rv(ctx, fw + 0x50C0);
    uint32_t obj = mem_read32(ctx, data + 0x58);
    int printed = 0;
    for (int i = 0; obj && i < 512; i++) {
        uint32_t state = mem_read8(ctx, obj + 0x59) | (uint32_t)(mem_read8(ctx, obj + 0xB8) & 1) << 8;
        float want = rf(ctx, obj + 0xCC), now = rf(ctx, obj + 0xD0);
        state |= (uint32_t)(want * 100.0f) << 16;
        if (seen_obj[i] != obj || seen_state[i] != state) {
            if (!printed++)
                fprintf(stderr, "[cameras3] field %d: eye %.1f %.1f %.1f cam+68 %.1f %.1f %.1f focus %.1f %.1f %.1f viewing %d\n", frames, eye.x, eye.y,
                        eye.z, p68.x, p68.y, p68.z, focus.x, focus.y, focus.z, mem_read8(ctx, 0x006BE3C0));
            V3 lo = rv(ctx, obj + 0x74), hi = rv(ctx, obj + 0x80);
            fprintf(stderr, "[cameras3]   object %d %08X level %d -> %d, fade %.2f (now %.2f), box %.0f %.0f %.0f .. %.0f %.0f %.0f\n", i, obj,
                    seen_obj[i] == obj ? (int)(int8_t)(seen_state[i] & 0xFF) : -9, (int)(int8_t)mem_read8(ctx, obj + 0x59), want, now, lo.x, lo.y, lo.z,
                    hi.x, hi.y, hi.z);
            seen_obj[i] = obj;
            seen_state[i] = state;
        }
        obj = mem_read32(ctx, obj + 0xC);
    }
}

/* ===================== qol5: My Castle's free camera =====================
 *
 * The Free Camera key in My Castle takes the camera out (and back). It is
 * the battle free camera's (same styles, controls and code: fly() and
 * orbit()), on the same FieldWorld camera: in free roam the castle's
 * camera (castle::FocusCamera, a game::graphics::CameraBase) is copied into
 * the field camera by CameraBase::Update, which then runs the field
 * camera's ICamera::Update, the override below; in the third-person view
 * (L) the battle engine's camera does the same. The free view is put in for
 * that Update and the game's put back after (apply_view), so the castle's
 * own camera is never touched and goes on underneath.
 *
 * - It is out only while Corrin walks around: castle::Sequence::
 *   TickMoveUnit (free roam on the castle map) or ProcSubjectView::WaitTick
 *   (the close-up third-person view, CastleWalk's) ran in the last
 *   ROAM_GRACE camera updates. Anything else (a building, a person or a
 *   menu opened with A, the castle menu, a cutscene, a visit, leaving My
 *   Castle) ends it: the picture eases back into the game's camera over
 *   CASTLE_HAND updates, and the host turns it off. The key works again
 *   once Corrin walks around.
 * - In the close-up view the castle's own map camera still updates the
 *   field camera every frame besides the view's camera (two Updates a
 *   frame); the map camera's (mode 2) passes untouched and only the view's
 *   takes the free view. Put back with the key, the view's follow camera is
 *   where it was (Corrin stood, the right stick and the mouse were the free
 *   camera's). Leaving the view (A, B or Start) while it is out drops it at
 *   once: the game's own way out starts from the view's camera.
 * - While it is out the host keeps the left stick, the D-pad and L/R from
 *   the game (cameras::mask_game_input): Corrin stands, the view can't be
 *   opened, the castle's walking (and CastleWalk's in the view) see no
 *   input. A, B, X, Y and Start still reach the game.
 * - It engages at once where the game's camera is, so nothing jumps; its
 *   "middle" (the fly camera's speed and range, the orbit's centre) is where
 *   the game's camera looked then (Corrin). Camera Reset puts it back there.
 * - Leaving My Castle (no castle::MainSeq) drops it at once. */
#define CASTLE_HAND 20
#define ROAM_GRACE 4
static struct {
    uint32_t updates;  /* the field camera's Updates in My Castle */
    uint32_t roam_at;  /* the Update count when Corrin last walked around */
    int roam_set;
    uint32_t view_at;  /* the Update count when the close-up view's WaitTick last ran */
    int view_set;
    int out_in_view;   /* the free camera came out in the close-up view */
    V3 middle;
    int hand_k;        /* the hand-over's length */
    /* the castle's camera (castle::FocusCamera, in the castle's own units)
     * is copied into the field camera by CameraBase::Update in its "look
     * from a rotation" mode (+0x40 = 2), with its target (+0x10) and distance
     * (+0x48) scaled by CASTLE_SCALE (the float at *0x003E0CA4, 50) but its
     * eye (+0x04) not: ICamera::UpdateView makes the real eye from the
     * scaled target, the rotation and the scaled distance, which is the
     * castle's eye scaled the same way. So the game's view before its Update
     * is the eye times the scale (game_view). */
    int last_free;
} cf;

#define CASTLE_SCALE_PTR 0x003E0CA4u /* the literal CameraBase::Update scales by */

/* the game's view this frame: its eye and target as its Update will make
 * them */
static void game_view(Context *ctx, uint32_t cam, int bind, uint32_t base, V3 *eye, V3 *target) {
    *eye = rv(ctx, base);
    *target = rv(ctx, base + 12);
    if (bind || mem_read8(ctx, cam + 0x40) != 2) return;
    float scale = rf(ctx, mem_read32(ctx, CASTLE_SCALE_PTR));
    if (scale > 0.01f && scale < 1000.0f) *eye = mul(*eye, scale);
}

static void castle_roams(void) {
    cf.roam_at = cf.updates;
    cf.roam_set = 1;
}

/* the close-up view's WaitTick ran (Corrin walks in the third person) */
static void castle_view_ticks(void) {
    cf.view_at = cf.updates;
    cf.view_set = 1;
}

static void castle_drop(void) {
    if (fc.battle != CASTLE_MARK) return;
    fc.battle = 0;
    fc.engaged = 0;
    fc.hand_n = 0;
    fc.shown = 0;
}

/* 1 when it drew the frame's view (the game's Update ran inside) */
static int castle_free(Context *ctx, uint32_t cam) {
    if (!mem_read32(ctx, MAIN_SEQ_PTR)) {
        castle_drop();
        cf.roam_set = cf.view_set = 0;
        return 0;
    }
    /* the close-up view: the castle's own camera still updates the field
     * camera each frame (the map camera, mode 2) besides the view's; only
     * the view's is the picture, so the castle's passes untouched */
    int in_view = cf.view_set && cf.updates - cf.view_at <= ROAM_GRACE;
    int bind0 = mem_read32(ctx, cam + 0x5C8) != 0;
    if (in_view && !bind0 && mem_read8(ctx, cam + 0x40) == 2) return 0;
    cf.updates++;
    int roam = (cf.roam_set && cf.updates - cf.roam_at <= ROAM_GRACE) || in_view;
    /* out in the view and the view is closing (A, B or Start): the game's
     * own way out starts from the view's camera; the free one is dropped at
     * once rather than eased across two cameras */
    if (fc.battle == CASTLE_MARK && cf.out_in_view && !in_view) {
        castle_drop();
        cf.out_in_view = 0;
        if (logging) fprintf(stderr, "[qol5] castle free camera: the close-up view closes, the game's camera\n");
    }
    HaikuCamFrame frame;
    int on = ask(&frame, roam ? CAM_CASTLE_ROAM : CAM_CASTLE_IDLE);
    logging = frame.log;
    int mine = fc.battle == CASTLE_MARK;
    if (!mine && !(on && roam)) return 0;
    int bind = mem_read32(ctx, cam + 0x5C8) != 0;
    uint32_t base = bind ? cam + 0x618 : cam + 0x04; /* eye, target, up */
    V3 eye, target;
    game_view(ctx, cam, bind, base, &eye, &target);
    if (on && roam) {
        if (!mine || frame.mode != fc.mode) {
            fc.battle = CASTLE_MARK;
            fc.mode = frame.mode;
            fc.engaged = 0;
            fc.handed = 0;
        }
        /* engaged again while easing back: from the view shown */
        if (fc.hand_n && fc.shown) {
            eye = fc.shown_eye;
            target = fc.shown_target;
            fc.engaged = 0;
        }
        fc.hand_n = 0;
        if (frame.reset && fc.engaged) {
            fc.engaged = 0;
            if (logging) fprintf(stderr, "[qol5] castle free camera reset: from the game's camera again\n");
        }
        if (!fc.engaged) {
            cf.middle = target;
            cf.out_in_view = in_view;
            if (logging)
                fprintf(stderr, "[qol5] castle free camera out (%s): eye %.1f %.1f %.1f target %.1f %.1f %.1f bind %d mode %d dist %.1f\n",
                        frame.mode == CAM_FLY ? "fly" : "orbit", eye.x, eye.y, eye.z, target.x, target.y, target.z, bind, mem_read8(ctx, cam + 0x40),
                        rf(ctx, cam + 0x48));
        }
        if (frame.mode == CAM_FLY)
            fly(ctx, cam, bind, base, eye, target, cf.middle, &frame, 1);
        else
            orbit(ctx, cam, bind, base, eye, target, cf.middle, &frame, 1);
        return 1;
    }
    /* switched off, or the walking around is over: ease into the game's
     * camera, which goes on as the game moves it */
    if (fc.engaged && fc.shown && !fc.hand_n) {
        fc.hand_eye = fc.shown_eye;
        fc.hand_target = fc.shown_target;
        fc.hand_n = 1;
        fc.engaged = 0;
        cf.hand_k = CASTLE_HAND;
        if (logging)
            fprintf(stderr, "[qol5] castle free camera back (%s): eases into the game's camera\n", roam ? "switched off" : "no longer walking around");
    }
    if (!fc.hand_n || fc.hand_n > cf.hand_k) {
        castle_drop();
        if (logging) fprintf(stderr, "[qol5] castle free camera: the game's camera\n");
        return 0;
    }
    float k = (float)fc.hand_n / (float)cf.hand_k;
    float e = k * k * k * (k * (k * 6.0f - 15.0f) + 10.0f);
    fc.hand_n++;
    apply_view(ctx, cam, bind, base, add(fc.hand_eye, mul(sub(eye, fc.hand_eye), e)), add(fc.hand_target, mul(sub(target, fc.hand_target), e)), 1.0f);
    return 1;
}

/* castle::Sequence::TickMoveUnit(void): free roam. Only noted (qol5). */
RECOMP_OVERRIDE(0x00498A78) {
    castle_roams();
    CALL(RECOMP_ORIGINAL(0x00498A78));
}

/* ICamera::Update(void) */
RECOMP_OVERRIDE(0x004DBB50) {
    uint32_t cam = ctx->r[0];
    if (!fates_cameras_hook || cam != field_camera(ctx) || fc.bypass) {
        CALL(RECOMP_ORIGINAL(0x004DBB50));
        return;
    }
    field_log(ctx);
    castle_frame(ctx);
    /* qol5: My Castle's free camera */
    if (castle_free(ctx, cam)) return;
    if (fc.battle == CASTLE_MARK) {
        /* the castle's own map camera under the close-up view: the game's */
        CALL(RECOMP_ORIGINAL(0x004DBB50));
        return;
    }
    /* a battle is shown: only looked for while the free camera is on */
    static uint32_t battle_seq;
    if (!battle_seq) battle_seq = name_hash("BattleSeq");
    uint32_t battle = fc.enabled ? find_proc(ctx, battle_seq) : 0;
    HaikuCamFrame frame;
    fc.enabled = ask(&frame, battle ? CAM_BATTLE : CAM_QUERY_BATTLE);
    logging = frame.log;
    if (!battle || !fc.enabled) {
        fc.engaged = 0;
        fc.battle = 0;
        fc.handed = 0;
        fc.hand_n = 0;
        fc.shown = 0;
        CALL(RECOMP_ORIGINAL(0x004DBB50));
        return;
    }
    if (battle != fc.battle || frame.mode != fc.mode) {
        fc.battle = battle;
        fc.mode = frame.mode;
        fc.engaged = 0;
        fc.hand_n = 0;
    }
    if (frame.reset && fc.engaged) {
        fc.engaged = 0;
        if (logging) fprintf(stderr, "[cameras] free camera reset: the game's camera again\n");
    }
    int bind = mem_read32(ctx, cam + 0x5C8) != 0;
    uint32_t base = bind ? cam + 0x618 : cam + 0x04; /* eye, target, up */
    V3 eye = rv(ctx, base), target = rv(ctx, base + 12);
    int moved = frame.orbit[0] != 0.0f || frame.orbit[1] != 0.0f || frame.pan[0] != 0.0f || frame.pan[1] != 0.0f || frame.zoom != 0.0f
                || frame.move[0] != 0.0f || frame.move[1] != 0.0f || frame.move[2] != 0.0f || frame.dolly != 0.0f;
    /* cameras3: handing over to the comeback: from the free camera's last
     * view to the game's, eased (smootherstep) over HAND_FRAMES updates, the
     * game's own zoom out going on meanwhile; then the game's camera */
    if (fc.hand_n) {
        if (fc.hand_n > HAND_FRAMES) {
            fc.hand_n = 0;
            fc.engaged = 0;
            fc.handed = fc.battle;
            if (logging) fprintf(stderr, "[cameras] the free camera has handed over: the game's camera\n");
            CALL(RECOMP_ORIGINAL(0x004DBB50));
            return;
        }
        float k = (float)fc.hand_n / (float)HAND_FRAMES;
        float e = k * k * k * (k * (k * 6.0f - 15.0f) + 10.0f);
        fc.hand_n++;
        apply_view(ctx, cam, bind, base, add(fc.hand_eye, mul(sub(eye, fc.hand_eye), e)), add(fc.hand_target, mul(sub(target, fc.hand_target), e)), 1.0f);
        return;
    }
    V3 middle = fighters(ctx, target);
    if (frame.mode == CAM_FLY) {
        fly(ctx, cam, bind, base, eye, target, middle, &frame, moved);
        return;
    }
    orbit(ctx, cam, bind, base, eye, target, middle, &frame, moved);
}

/* BattleCamera::Update(void): the game's own, kept as a place for later
 * work on the battle camera without changing overrides.h */
RECOMP_OVERRIDE(0x001A9924) {
    CALL(RECOMP_ORIGINAL(0x001A9924));
}

/* cameras3: FieldSky::OnUpdate(RenderState&) centres the field's sky dome
 * on the eye of the RenderState's camera (+8: the field camera, its eye at
 * +4; FieldViewing 1, battles and the castle view: scaled to 0.9 of the far
 * plane), then IActor::OnUpdate makes the drawn matrix from it. The free
 * camera's eye is the game's again by the time the scene updates (it is put
 * back after ICamera::Update), so the dome stayed round the game's eye: the
 * free camera could fly out of it (black beyond its edge) or rise above
 * its horizon. While the free camera shows the battle, the camera's eye is
 * the free camera's for the dome's update, and put back after: the dome
 * moves with the free camera, skybox-style (its orientation and size are
 * the game's), and the game's own camera is untouched. */
static void sky_restore(Context *ctx, const uint32_t *words) {
    uint32_t cam = words[0];
    if (!cam) return;
    mem_write32(ctx, cam + 4, words[1]);
    mem_write32(ctx, cam + 8, words[2]);
    mem_write32(ctx, cam + 12, words[3]);
}

RECOMP_OVERRIDE(0x004E6384) {
    uint32_t sky = ctx->r[0], state = ctx->r[1];
    uint32_t fw = mem_read32(ctx, FIELD_WORLD_PTR);
    uint32_t cam = state ? mem_read32(ctx, state + 8) : 0;
    if (fc.shown && (fc.engaged || fc.hand_n) && fw && sky == mem_read32(ctx, fw + 0x10) && cam && cam == field_camera(ctx)) {
        uint32_t x = mem_read32(ctx, cam + 4), y = mem_read32(ctx, cam + 8), z = mem_read32(ctx, cam + 12);
        wv(ctx, cam + 4, fc.shown_eye);
        CALL_THEN(RECOMP_ORIGINAL(0x004E6384), sky_restore, cam, x, y, z);
        return;
    }
    CALL(RECOMP_ORIGINAL(0x004E6384));
}

/* gfxcastle: FieldData::Tick(FieldObject::Args) runs FieldObject::Update for
 * each of the field's objects (the battle's buildings, windmill, rocks,
 * ground patches), from FieldWorld::Tick right after the camera's Update.
 * Args (r1..r3, then the stack): +4 the squared distance past which an
 * object takes its low-detail model, +8 the squared distance from the
 * camera's position (FieldCamera +0x68, made by Update: with the free camera
 * that is the free eye) past which it fades out. Flying the free camera
 * back from the fight, the buildings and the windmill faded out one after
 * another (`FATES_CAMERAS_FIELDLOG=1`, Chapter 8: wanted fade 0 as the eye
 * passed a few hundred units), "objects disappear when you zoom out".
 * While the free camera's view is shown (engaged or handing over) neither
 * applies: every object stays, at its full detail; the game's own view
 * (and the hand-over's end) is the game's again, faded as it fades. */
/* drawdist: with Full Draw Distance on (overrides/drawdist.c) this holds for
 * every camera, the game's own included: the castle's view, battles, the
 * map. Off, the game's own views fade as the game fades them. */
int drawdist_on(void);
RECOMP_OVERRIDE(0x004FF41C) {
    if ((fc.shown && (fc.engaged || fc.hand_n) && fates_cameras_hook) || drawdist_on()) {
        float far = 1.0e12f;
        uint32_t bits;
        memcpy(&bits, &far, 4);
        ctx->r[2] = bits; /* no low-detail models */
        ctx->r[3] = bits; /* no fading out */
    }
    CALL(RECOMP_ORIGINAL(0x004FF41C));
}

/* ==================== 2. walking in the third-person view ==================== */

#define WAIT_TICK        0x00459248u /* castle::ProcSubjectView::WaitTick */
#define MOVE_PLAYER      0x0048F8A4u /* castle::Actor::MovePlayer(ProcInst*, bool) */
#define PLAY_ANIM_NAME   0x00171348u /* BattleUnit::PlayAnim(const char*) */
#define PLAY_ANIM_TYPE   0x001713E4u /* BattleUnit::PlayAnim(AnimClip::Type) */
#define INDEX_BY_NAME    0x004E2E10u /* AnimClip::IndexByName(const char*) */
#define CLEAR_EXCEPT_SUBJECT 0x001F9400u
#define LOAD_PLAYER_CLEAR_RET 0x00458E20u /* Load's call for Corrin's unit */
#define NAME_WALK        0x0050A56Cu /* "歩き", ActionEvWalkTo::GetAnimName's */
#define NAME_RUN         0x0050A188u /* "Run", ActionEvRunTo::GetAnimName's */
#define GET_UNIT         0x00541940u /* castle::Actor::GetUnit */
#define FORCE_MOVE       0x003E04B0u /* castle::FocusCamera::ForceMoveToTargetPos */
#define FOCUS_CAMERA_PTR 0x006D2208u /* **: FocusCamera::Get's proc; its +0x30 the camera */

enum { STAND, WALK, RUN };

/* castle units a frame at full stick: MovePlayer's 6 (x1.75 with B or Y) */
#define CASTLE_SPEED 6.0f

static float env_float(const char *name, float fallback) {
    const char *v = getenv(name);
    return v ? (float)atof(v) : fallback;
}

/* the walk: per view */
static struct {
    uint32_t proc, subject, actor;
    int enabled, started, anim, moving;
    float look[2], turn[2];
    V3 dir;           /* the way Corrin walks, world */
    float pull;       /* how far the camera has backed off */
    float zoom;       /* the player's zoom, castle units back */
    V3 cam;           /* the camera's centre, following Corrin damped */
    int cam_set;
    int blocked;      /* frames the castle's collision has held Corrin */
    int walking_call; /* MovePlayer is running: ColsArray::Add leaves out the unseen */
    float gain;       /* castle units moved per unit asked, measured */
    float natural[3]; /* the walk's and run's own speed, units a tick */
    float rate;       /* the animation's playback rate */
    V3 render, vel, prev; /* where Corrin's unit is drawn, its speed, the castle's last place */
    int render_set;
    /* cameras3 */
    int walk_on;           /* walking was on when the view loaded (Load's ask) */
    uint32_t calm_state;   /* the transition camera whose blur was taken out */
    V3 face;               /* the way Corrin faces while she stands */
    int face_set;
    float push;            /* the stick's push, 0..1 past the dead zone */
    float owed;            /* castle units the walk is behind (+) or ahead (-) of its speed */
    int waited;            /* WaitTick has run: the view is open */
    uint32_t shadow_logged;
    /* castlecam: HitCheck's lift of the eye over what is under it, as the
     * game has it this tick and as the view is drawn (eased) */
    float lift_raw, lift;
    int lift_set;
    uint32_t hit_param;
    V3 hit_eye, hit_up;
    uint32_t hit_fov;      /* the param's tenth number (the view angle HitCheck's third test sets) */
} wk;

/* the chained calls, per thread */
typedef struct {
    uint32_t tls, sp, lr, proc, step;
    uint32_t pad, held, sx, sy;
    int want, run;
    float speed;
    float asked; /* cameras3: castle units asked of MovePlayer this tick */
} WalkStep;
static WalkStep walk_steps[2];

static WalkStep *step_find(Context *ctx) {
    for (int i = 0; i < 2; i++)
        if (walk_steps[i].step && walk_steps[i].tls == ctx->tls && walk_steps[i].sp == ctx->r[13]) return &walk_steps[i];
    return 0;
}
static WalkStep *step_new(Context *ctx) {
    for (int i = 0; i < 2; i++)
        if (!walk_steps[i].step || walk_steps[i].tls == ctx->tls) {
            memset(&walk_steps[i], 0, sizeof walk_steps[i]);
            walk_steps[i].tls = ctx->tls;
            walk_steps[i].sp = ctx->r[13];
            return &walk_steps[i];
        }
    return 0;
}

/* calls code with the arguments in r0..r2, returning to the override at me */
static void chain(Context *ctx, uint32_t me, uint32_t code, uint32_t r0, uint32_t r1, uint32_t r2) {
    ctx->r[0] = r0;
    ctx->r[1] = r1;
    ctx->r[2] = r2;
    ctx->r[14] = me;
    ctx->r[15] = code & ~1u;
    ctx->thumb = code & 1;
}

/* Corrin's castle actor: the first of MainSeq's actors of type 0 */
static uint32_t player_actor(Context *ctx) {
    uint32_t seq = mem_read32(ctx, MAIN_SEQ_PTR);
    if (!seq) return 0;
    uint32_t actor = mem_read32(ctx, seq + 0x74);
    for (int guard = 0; actor && guard < 512; guard++) {
        if (mem_read8(ctx, actor + 0x2C) == 0) return actor;
        actor = mem_read32(ctx, actor + 0x10);
    }
    return 0;
}

static int is_castle_actor(Context *ctx, uint32_t object) {
    uint32_t seq = mem_read32(ctx, MAIN_SEQ_PTR);
    if (!seq) return 0;
    uint32_t actor = mem_read32(ctx, seq + 0x74);
    for (int guard = 0; actor && guard < 512; guard++) {
        if (actor == object) return 1;
        actor = mem_read32(ctx, actor + 0x10);
    }
    return 0;
}

/* the SubjectCamera, when it is the battle camera's current state */
static uint32_t subject_camera(Context *ctx) {
    uint32_t bc = battle_camera(ctx);
    uint32_t state = bc ? current_state(ctx, bc) : 0;
    return state && mem_read32(ctx, state) == SUBJECT_CAMERA_VT ? state : 0;
}

/* where the camera looks, on the ground: x and z, normalised */
static void camera_forward(Context *ctx, float *fx, float *fz) {
    uint32_t cam = field_camera(ctx);
    *fx = 0.0f;
    *fz = -1.0f;
    if (!cam) return;
    V3 f = sub(rv(ctx, cam + 0x10), rv(ctx, cam + 0x04));
    float l = sqrtf(f.x * f.x + f.z * f.z);
    if (l > 1e-4f) {
        *fx = f.x / l;
        *fz = f.z / l;
    }
}

/* after the game's WaitTick: while walking, Corrin looks where it goes
 * rather than where the camera does (WaitTick aims the head, +0xD8 = 4, at
 * +0xE4) */
static void walk_gaze(Context *ctx, const uint32_t *words) {
    uint32_t unit = words[0];
    if (!unit) return;
    V3 at = rv(ctx, unit + 0x1A4);
    if (wk.moving) {
        wv(ctx, unit + 0xE4, add(at, v3(wk.dir.x * 50.0f, 20.0f, wk.dir.z * 50.0f)));
        return;
    }
    /* cameras3: standing, Corrin looks where the camera looks (WaitTick's
     * aim) while that is ahead of her; with the camera in front of her she
     * looks ahead rather than twist her head round */
    if (!wk.face_set) return;
    float fx, fz;
    camera_forward(ctx, &fx, &fz);
    if (fx * wk.face.x + fz * wk.face.z < 0.25f) wv(ctx, unit + 0xE4, add(at, v3(wk.face.x * 50.0f, 20.0f, wk.face.z * 50.0f)));
}

/* cameras3: WaitTick turns Corrin's body (a step-turn, ActionEvTurnTo)
 * whenever the camera has turned past a limit from the way she last faced
 * (+0x6C), as the 3DS's look-around from her head wants. Round a third
 * person camera that is her jumping from step-turn to step-turn while the
 * camera turns: the way she last faced is kept at the camera's, so no turn
 * starts, and she stands as she stood. */
/* cameras3: the castle's third-person view is open (loaded with walking on),
 * its proc still alive, in My Castle */
static int castle_view(Context *ctx) {
    return wk.walk_on && wk.proc && mem_read32(ctx, MAIN_SEQ_PTR) && !(mem_read32(ctx, wk.proc + 0x28) & 1);
}

/* cameras3: the castle view's way in and out are TransitionCameras (the
 * SubjectCamera itself, and the ComebackCamera castle::ProcSubjectView::
 * Comeback makes). Their first curve (+0x14, keys counted at +0x1C) drives
 * the radial blur's strength (the post effect parameters at 0x00700914, +9,
 * BloomContext::Draw), and the radial blur feeds each picture the one
 * before (RadialBlur's capture): pictures in between can't be drawn for
 * such frames (fates-gfx hfr.rs `carries`), so the whole flight down to
 * Corrin and back up was shown at the view's 30 pictures a second,
 * cross-faded. With walking on (the PC's third-person view) the blur's
 * curve is emptied as each of those cameras starts, so the flight is drawn
 * in between like the rest of the view: smooth at the display's rate. The
 * camera's own path (the game's curves) is unchanged. */
#define BLUR_PARAMS 0x00700914u
static void castle_frame(Context *ctx) {
    static int dbg;
    if (getenv("FATES_CAMERAS3_DEBUG") && dbg++ % 20 == 0)
        fprintf(stderr, "[cameras3] castle_frame walk_on %d proc %08X mainseq %08X flags %08X state vt %08X\n", wk.walk_on, wk.proc, mem_read32(ctx, MAIN_SEQ_PTR),
                wk.proc ? mem_read32(ctx, wk.proc + 0x28) : 0, battle_camera(ctx) && current_state(ctx, battle_camera(ctx)) ? mem_read32(ctx, current_state(ctx, battle_camera(ctx))) : 0);
    if (!castle_view(ctx)) return;
    uint32_t bc = battle_camera(ctx);
    uint32_t state = bc ? current_state(ctx, bc) : 0;
    if (!state || state == wk.calm_state) return;
    uint32_t vt = mem_read32(ctx, state);
    if (vt != SUBJECT_CAMERA_VT && vt != COMEBACK_CAMERA_VT) return;
    wk.calm_state = state;
    mem_write32(ctx, state + 0x1C, 0);
    mem_write8(ctx, BLUR_PARAMS + 9, 0);
    if (logging) fprintf(stderr, "[cameras] the view's %s camera: no radial blur, so it is drawn in between\n", vt == SUBJECT_CAMERA_VT ? "opening" : "closing");
}

static void keep_facing(Context *ctx, uint32_t proc) {
    uint32_t cam = field_camera(ctx);
    if (cam)
        for (int i = 0; i < 3; i++) mem_write32(ctx, proc + 0x6C + 4 * i, mem_read32(ctx, cam + 0x74 + 4 * i));
}

/* the playing animation's own speed, castle units a tick: its speed signal
 * (AnimClipSignals type 0x13, which ActionEvRunTo::Enter reads: hundredths
 * of a unit a time step) times the unit's time step; 0 without one */
static float natural_speed(Context *ctx, uint32_t unit) {
    uint32_t signals = mem_read32(ctx, unit + 0xAC);
    if (!signals) return 0.0f;
    int count = mem_read16(ctx, signals + 2);
    for (int i = 0; i < count && i < 64; i++) {
        if ((mem_read16(ctx, signals + 8 * i + 6) & 0xFF) == 0x13) {
            float top = (float)(int16_t)mem_read16(ctx, signals + 8 * i + 8) * 0.01f;
            float dt = rf(ctx, unit + 0x164);
            if (!(dt > 0.01f && dt < 8.0f)) dt = 1.0f;
            return top > 0.0f ? top * dt : 0.0f;
        }
    }
    return 0.0f;
}

/* AnimCtrl::GetAnimObj(type 0) of a model's controller (+0x104) */
static uint32_t anim_obj(Context *ctx, uint32_t model) {
    if (!model) return 0;
    uint32_t ctrl = model + 0x104;
    uint32_t obj = mem_read32(ctx, ctrl + mem_read8(ctx, ctrl + 0x61) * 0x10 + 0x28);
    for (int guard = 0; obj && guard < 16; guard++) {
        if (mem_read8(ctx, obj + 0x10) == 0) return obj;
        obj = mem_read32(ctx, obj + 0x58);
    }
    return 0;
}

/* the walk's or run's legs at the pace Corrin really moves: its animation
 * played at the moved distance over its own speed (BattleUnit::PlayAnim
 * sets the speed, +0x2C, to half the unit's time rate) */
static float anim_rate(Context *ctx, uint32_t unit, float moved_by) {
    float natural = natural_speed(ctx, unit);
    /* an animation without a speed signal (the female walk's): the male's */
    if (!(natural > 0.0f)) natural = (wk.anim == RUN ? 1.8f : 0.4f) * (rf(ctx, unit + 0x164) > 0.01f ? rf(ctx, unit + 0x164) : 1.0f);
    float rate = moved_by / natural;
    /* cameras3: the run is never slower than a jog (at the slowest analog
     * run the legs go a little faster than the ground) */
    if (rate < (wk.anim == RUN ? 0.6f : 0.35f)) rate = wk.anim == RUN ? 0.6f : 0.35f;
    if (rate > 2.6f) rate = 2.6f;
    float base = 0.5f * rf(ctx, unit + 0x10);
    uint32_t models[2] = {mem_read32(ctx, unit + 0x30), mem_read32(ctx, unit + 0x40)};
    for (int i = 0; i < 2; i++) {
        uint32_t obj = anim_obj(ctx, models[i]);
        if (obj) wf(ctx, obj + 0x2C, base * rate);
    }
    return rate;
}

static void wait_tick_original(Context *ctx, uint32_t proc, uint32_t lr) {
    uint32_t unit = mem_read32(ctx, proc + 0x5C);
    ctx->r[0] = proc;
    ctx->r[14] = lr;
    ctx->r[15] = WAIT_TICK;
    ctx->thumb = 0;
    CALL_THEN(RECOMP_ORIGINAL(0x00459248), walk_gaze, unit);
}

/* castle::ProcSubjectView::WaitTick(void) */
RECOMP_OVERRIDE(0x00459248) {
    WalkStep *s = step_find(ctx);
    /* a fresh call has the proc in r0, a return from a chained call not */
    if (s && ctx->r[0] == s->proc) {
        s->step = 0;
        s = 0;
    }
    if (!s) {
        uint32_t proc = ctx->r[0];
        uint32_t unit = mem_read32(ctx, proc + 0x5C);
        castle_view_ticks(); /* qol5: Corrin walks in the close-up view */
        HaikuCamFrame frame;
        int enabled = unit && ask(&frame, CAM_CASTLE);
        logging = frame.log;
        uint32_t subject = enabled ? subject_camera(ctx) : 0;
        uint32_t actor = subject ? player_actor(ctx) : 0;
        if (proc != wk.proc) {
            memset(&wk, 0, sizeof wk);
            wk.proc = proc;
        }
        wk.enabled = enabled && subject && actor;
        wk.waited = 1;
        wk.subject = subject;
        wk.actor = actor;
        wk.look[0] = frame.look[0];
        wk.look[1] = frame.look[1];
        wk.turn[0] = frame.turn[0];
        wk.turn[1] = frame.turn[1];
        if (!wk.enabled) {
            CALL(RECOMP_ORIGINAL(0x00459248));
            return;
        }
        uint32_t pad = mem_read32(ctx, PAD_PTR);
        uint32_t held = mem_read32(ctx, pad + 0x0C);
        /* Pad::GetSumX / GetSumY */
        float sx = (held & 0x20) ? -1.0f : (held & 0x10) ? 1.0f : rf(ctx, pad + 0x04);
        float sy = (held & 0x40) ? 1.0f : (held & 0x80) ? -1.0f : rf(ctx, pad + 0x08);
        float mag = sqrtf(sx * sx + sy * sy);
        if (mag > 1.0f) {
            sx /= mag;
            sy /= mag;
            mag = 1.0f;
        }
        wk.zoom -= frame.zoom * 6.0f;
        if (wk.zoom < -12.0f) wk.zoom = -12.0f;
        if (wk.zoom > 60.0f) wk.zoom = 60.0f;
        /* A, B or Start leaves the view: the castle's camera is put over
         * Corrin first, where the view's way back (ComebackCamera) and the
         * free roam then take it from */
        uint32_t pressed = mem_read32(ctx, pad + 0x14);
        if (wk.started && (pressed & 0xB)) {
            uint32_t holder = mem_read32(ctx, FOCUS_CAMERA_PTR);
            uint32_t focus = holder ? mem_read32(ctx, holder + 0x30) : 0;
            if (focus && (s = step_new(ctx))) {
                s->proc = proc;
                s->lr = ctx->r[14];
                s->step = 5;
                uint32_t type = mem_read8(ctx, focus + 0x68A);
                /* in tiles, as the actor keeps them at +0x84 (SetPos) */
                wv(ctx, focus + 0x68C + 12 * (type < 3 ? type : 0), rv(ctx, actor + 0x84));
                if (logging) fprintf(stderr, "[cameras] leaving the view: the castle's camera over Corrin\n");
                chain(ctx, WAIT_TICK, FORCE_MOVE, focus, 0, 0);
                CALL(recomp_call);
                return;
            }
        }
        if (!wk.face_set) {
            /* cameras3: the way she faces when the view opens: the way the
             * game turns her unit (UpdateDir's target), else the camera's */
            float tx = rf(ctx, unit + 0x1BC), tz = rf(ctx, unit + 0x1C0);
            if (tx * tx + tz * tz > 0.25f) {
                wk.face = norm(v3(tx, 0, tz));
            } else {
                float cx, cz;
                camera_forward(ctx, &cx, &cz);
                wk.face = v3(cx, 0, cz);
            }
            wk.face_set = 1;
        }
        if (mag < 0.08f && wk.anim == STAND) {
            wk.moving = 0;
            wk.push = 0.0f;
            /* Corrin's unit settles on the castle's place after a walk */
            if (wk.render_set) {
                V3 at = rv(ctx, actor + 0x5C);
                wk.vel = v3(0, 0, 0);
                wk.render = add(wk.render, mul(sub(at, wk.render), 0.4f));
                wk.render.y = at.y;
                wv(ctx, unit + 0x1A4, wk.render);
            }
            keep_facing(ctx, proc);
            CALL_THEN(RECOMP_ORIGINAL(0x00459248), walk_gaze, unit);
            return;
        }
        if (!(s = step_new(ctx))) {
            CALL(RECOMP_ORIGINAL(0x00459248));
            return;
        }
        s->proc = proc;
        s->lr = ctx->r[14];
        /* the stick in the camera's frame: up is where it looks */
        float fx, fz;
        camera_forward(ctx, &fx, &fz);
        float mx = sx * -fz + sy * fx, mz = sx * fx + sy * fz;
        /* castle units a tick: the walk's and run's own speeds (their
         * animations' speed signal, see anim_rate) */
        float dt = rf(ctx, unit + 0x164);
        if (!(dt > 0.01f && dt < 8.0f)) dt = 1.0f;
        float walk = wk.natural[WALK] > 0.0f ? wk.natural[WALK] : 0.4f * dt;
        float run = wk.natural[RUN] > 0.0f ? wk.natural[RUN] : 1.8f * dt;
        walk *= env_float("FATES_CAMERAS_WALK", 2.0f);
        run *= env_float("FATES_CAMERAS_RUN", 1.3f);
        /* cameras3: analog movement. How far the stick is pushed sets the
         * speed: a slight push walks ("歩き"), a full one runs ("Run"), and
         * the speed goes smoothly from one to the other. The animation
         * changes past the middle (with a margin each way, so a stick held
         * there does not flip it), with the game's own cross-fade
         * (BattleUnit::PlayAnim fades the old pose out over the new
         * animation's blend time), and the legs keep the pace Corrin moves
         * at (anim_rate). No button sprints. The D-pad (the keyboard's
         * arrows, WASD) is all or nothing: it runs, and Y (V, Shift) held
         * with it walks instead. */
        float push = mag < 0.08f ? 0.0f : (mag - 0.08f) / 0.92f;
        if (push > 1.0f) push = 1.0f;
        if ((held & 0xF0) && (held & 0x800)) push = 0.4f;
        wk.push = push;
        const float walk_top = 0.55f; /* the push at which the walk is at its fastest */
        float speed = 0.0f;
        s->want = STAND;
        if (mag >= 0.08f) {
            if (push <= walk_top) {
                speed = walk * (0.3f + 0.7f * push / walk_top);
            } else {
                float k = (push - walk_top) / (1.0f - walk_top);
                k = k * k * (3.0f - 2.0f * k);
                speed = walk + (run - walk) * k;
            }
            /* the run from past the middle, back to the walk below it */
            s->run = wk.anim == RUN ? push > walk_top - 0.07f : push > walk_top + 0.07f;
            s->want = s->run ? RUN : WALK;
            s->speed = speed;
            wk.dir = norm(v3(mx, 0, mz));
            wk.face = wk.dir;
            wk.face_set = 1;
        }
        /* MovePlayer's stick: x and -z, castle units / 6, over what it
         * gives for what is asked (its slope factor) */
        if (!(wk.gain > 0.2f && wk.gain < 5.0f)) wk.gain = 1.0f;
        /* cameras3: the castle moves in whole units (CalcMove rounds), so a
         * slow walk asked as it is moved a whole unit every tick, twice its
         * speed. What is asked carries what the last moves owed or overshot
         * (wk.owed), and below half a unit nothing is asked that tick: on
         * average the castle moves at the speed asked, and Corrin's unit is
         * drawn on the smoothed path at that mean speed. */
        float ask = s->want != STAND ? speed + wk.owed : 0.0f;
        if (ask < 0.5f) ask = 0.0f;
        s->asked = ask;
        float v = ask / (CASTLE_SPEED * wk.gain);
        float px = wk.dir.x * v, py = -wk.dir.z * v;
        if (!wk.started) {
            /* the castle actor is put where the view placed Corrin's unit
             * (castle::cut::GetRescuePos may have moved it off a tile
             * someone stands on); SetPos at the end of MovePlayer makes the
             * rest of it follow */
            V3 at = rv(ctx, unit + 0x1A4);
            wf(ctx, actor + 0x5C, at.x);
            wf(ctx, actor + 0x64, at.z);
            wk.started = 1;
        }
        s->pad = pad;
        s->held = held;
        s->sx = mem_read32(ctx, pad + 0x04);
        s->sy = mem_read32(ctx, pad + 0x08);
        mem_write32(ctx, pad + 0x0C, held & ~0xF0u);
        wf(ctx, pad + 0x04, px);
        wf(ctx, pad + 0x08, py);
        /* where the castle has Corrin before the move, and the drawn path's
         * start */
        wk.prev = rv(ctx, actor + 0x5C);
        if (!wk.render_set) {
            wk.render = rv(ctx, unit + 0x1A4);
            wk.vel = v3(0, 0, 0);
            wk.render_set = 1;
        }
        wk.walking_call = 1;
        s->step = 1;
        /* no dash in MovePlayer: the speed is the stick's here */
        chain(ctx, WAIT_TICK, MOVE_PLAYER, actor, proc, 0);
        CALL(recomp_call);
        return;
    }
    uint32_t proc = s->proc;
    uint32_t unit = mem_read32(ctx, proc + 0x5C);
    if (s->step == 1) {
        /* back from MovePlayer */
        wk.walking_call = 0;
        mem_write32(ctx, s->pad + 0x0C, s->held);
        mem_write32(ctx, s->pad + 0x04, s->sx);
        mem_write32(ctx, s->pad + 0x08, s->sy);
        uint32_t actor = wk.actor;
        V3 at = rv(ctx, actor + 0x5C);
        V3 was = rv(ctx, unit + 0x1A4);
        /* held by a wall or a building: Corrin stands rather than run on
         * the spot (after a few frames, so a corner slid along does not
         * flicker) */
        float dx = at.x - wk.prev.x, dz = at.z - wk.prev.z;
        float moved_by = sqrtf(dx * dx + dz * dz);
        if (s->want != STAND) {
            wk.owed = s->speed + wk.owed - moved_by;
            if (wk.owed > 2.0f) wk.owed = 2.0f;
            if (wk.owed < -2.0f) wk.owed = -2.0f;
        } else {
            wk.owed = 0.0f;
        }
        if (s->want != STAND && s->asked > 0.0f && moved_by < 0.2f * s->asked) {
            if (++wk.blocked >= 4) {
                s->want = STAND;
                wk.owed = 0.0f;
            }
        } else if (s->asked > 0.0f) {
            wk.blocked = 0;
            /* MovePlayer's slope factor, learnt while nothing is in the way
             * (from the faster moves: whole units, rounded) */
            if (s->want != STAND && s->asked >= 1.5f && moved_by > 0.6f * s->asked)
                wk.gain += (wk.gain * moved_by / s->asked - wk.gain) * 0.3f;
        }
        /* the castle keeps whole units (castle::ColsEnumerate::CalcMove
         * rounds), so its steps come 2, 3, 2, 2, 3 units long: at the
         * castle's small scale nothing shows, under a full-size Corrin and
         * a camera following her it shakes. Her unit is drawn on a smoothed
         * path instead: the castle's mean speed, pulled back onto its place,
         * never more than 6 units off it. */
        V3 step = sub(at, wk.prev);
        wk.prev = at;
        step.y = 0.0f;
        wk.vel = add(wk.vel, mul(sub(step, wk.vel), 0.25f));
        if (s->want == STAND) wk.vel = mul(wk.vel, 0.5f);
        wk.render = add(wk.render, wk.vel);
        wk.render = add(wk.render, mul(sub(at, wk.render), 0.1f));
        wk.render.y = at.y;
        V3 off = sub(at, wk.render);
        off.y = 0.0f;
        if (len(off) > 8.0f) wk.render = at;
        float pace = sqrtf(wk.vel.x * wk.vel.x + wk.vel.z * wk.vel.z);
        wk.rate = s->want != STAND && wk.anim == s->want ? anim_rate(ctx, unit, pace) : 1.0f;
        wk.moving = s->want != STAND;
        wv(ctx, unit + 0x1A4, wk.render);
        if (wk.moving) {
            /* the unit turns toward where it goes (BattleUnit::UpdateDir),
             * and does not move by itself (UpdatePos: speed 0, the most 0) */
            wf(ctx, unit + 0x1BC, wk.dir.x);
            wf(ctx, unit + 0x1C0, wk.dir.z);
            wf(ctx, unit + 0x1B0, 0.0f);
            wf(ctx, unit + 0x1B4, 0.0f);
        }
        /* WaitTick keeps the body still while the camera is within its
         * limit of the way it last faced: that is the camera's now (also as
         * she stops: cameras3) */
        keep_facing(ctx, proc);
        /* the view's centre follows (SubjectCamera::UpdateParam) */
        wv(ctx, proc + 0x60, at);
        if (logging) {
            uint32_t cam = field_camera(ctx);
            V3 eye = cam ? rv(ctx, cam + 4) : v3(0, 0, 0), look = cam ? rv(ctx, cam + 0x10) : v3(0, 0, 0);
            fprintf(stderr, "[cameras] walk %s: push %.2f speed %.2f moved %.2f rate %.2f castle %.1f %.1f %.1f (unit was %.1f %.1f %.1f) dir %.2f %.2f eye %.1f %.1f %.1f target %.1f %.1f %.1f\n",
                    s->want == RUN ? "run" : s->want == WALK ? "walk" : "stop", wk.push, s->speed, pace, wk.rate, at.x, at.y, at.z, was.x, was.y, was.z, wk.dir.x, wk.dir.z,
                    eye.x, eye.y, eye.z, look.x, look.y, look.z);
        }
        if (s->want == wk.anim) {
            s->step = 0;
            wait_tick_original(ctx, proc, s->lr);
            return;
        }
        if (wk.anim == STAND) {
            /* a turn in place (ActionEvTurnTo) would aim the unit and end
             * with the standing animation: the action state machine is
             * cleared first, as WaitTick does before it starts one */
            uint32_t machine = unit + 0x8C;
            uint32_t clear = mem_read32(ctx, mem_read32(ctx, machine) + 0x10);
            s->step = 2;
            if (clear) {
                chain(ctx, WAIT_TICK, clear, machine, 0, 0);
                CALL(recomp_call);
                return;
            }
        }
        s->step = 2; /* straight to the animation */
    }
    if (s->step == 2) {
        s->step = 3;
        if (s->want == STAND)
            chain(ctx, WAIT_TICK, PLAY_ANIM_TYPE, unit, (uint32_t)(int32_t)(int16_t)mem_read16(ctx, unit + 0x15C), 0);
        else
            chain(ctx, WAIT_TICK, PLAY_ANIM_NAME, unit, s->want == RUN ? NAME_RUN : NAME_WALK, 0);
        CALL(recomp_call);
        return;
    }
    if (s->step == 5) {
        /* back from FocusCamera::ForceMoveToTargetPos: the game's WaitTick
         * leaves the view */
        s->step = 0;
        wait_tick_original(ctx, proc, s->lr);
        return;
    }
    /* step 3: back from PlayAnim */
    wk.anim = s->want;
    if (s->want != STAND) {
        float natural = natural_speed(ctx, unit);
        if (natural > 0.0f) wk.natural[s->want] = natural;
    }
    if (logging)
        fprintf(stderr, "[cameras] walk animation %d (anim index now %d), its own speed %.3f a tick (dt %.3f, time rate %.3f, turn rate %.3f)\n", s->want,
                (int16_t)mem_read16(ctx, unit + 0x14A), natural_speed(ctx, unit), rf(ctx, unit + 0x164), rf(ctx, unit + 0x10), rf(ctx, unit + 0x160));
    s->step = 0;
    wait_tick_original(ctx, proc, s->lr);
}

/* CameraInput::Tick(Type, float, float, float): the SubjectCamera's (its
 * +0xD4) reads the right stick and the mouse in place of the circle pad
 * while walking is on */
static struct { uint32_t pad, sx, sy; } look_saved;
static void look_restore(Context *ctx, const uint32_t *words) {
    if (!words[0]) return;
    mem_write32(ctx, words[0] + 0x04, words[1]);
    mem_write32(ctx, words[0] + 0x08, words[2]);
}

RECOMP_OVERRIDE(0x00194178) {
    uint32_t input = ctx->r[0];
    if (!wk.enabled || !wk.subject || input != wk.subject + 0xD4 || subject_camera(ctx) != wk.subject) {
        CALL(RECOMP_ORIGINAL(0x00194178));
        return;
    }
    uint32_t pad = mem_read32(ctx, PAD_PTR);
    look_saved.pad = pad;
    look_saved.sx = mem_read32(ctx, pad + 0x04);
    look_saved.sy = mem_read32(ctx, pad + 0x08);
    wf(ctx, pad + 0x04, wk.look[0]);
    wf(ctx, pad + 0x08, wk.look[1]);
    /* a mouse turn goes straight onto the turn the stick builds up */
    if (wk.turn[0] != 0.0f) wf(ctx, input + 0x04, rf(ctx, input + 0x04) - wk.turn[0]);
    wk.turn[0] = wk.turn[1] = 0.0f;
    CALL_THEN(RECOMP_ORIGINAL(0x00194178), look_restore, pad, look_saved.sx, look_saved.sy);
}

/* ColsArray::Add(const ColsBase*): while Corrin walks in the view, the
 * castle's people the view does not show are no obstacle */
RECOMP_OVERRIDE(0x004FC9DC) {
    uint32_t object = ctx->r[1];
    if (wk.walking_call && object && object != wk.actor && is_castle_actor(ctx, object)) {
        uint32_t proc = wk.proc;
        int count = (int)mem_read32(ctx, proc + 0x34), shown = 0;
        uint32_t list = mem_read32(ctx, proc + 0x38);
        for (int i = 0; i < count && i < 8; i++)
            if (mem_read32(ctx, list + 4 * i) == object) shown = 1;
        if (!shown) {
            ctx->budget -= 8;
            RETURN_TO(ctx->r[14]);
        }
    }
    CALL(RECOMP_ORIGINAL(0x004FC9DC));
}

/* BattleUnitResName::ClearExceptSubjectAnims(void), called by Load for
 * Corrin's unit first (at 0x00458E1C, Load's r4 the view's proc). With
 * walking on, before the game's clear:
 *
 * - the people the view makes battle units of become the 7 nearest to
 *   Corrin, however far, rather than the first 7 within 200 units
 *   (CreateBind's choice), so the castle around Corrin is peopled. The
 *   view holds 8 units (the cut-scene controller's casts), Corrin among
 *   them. A castle actor has a unit to show when castle::Actor::GetUnit
 *   says so; it is asked of each, chained. Corrin goes first in the list,
 *   which Load's first loop (Corrin's) has already passed, and its second
 *   loop (the others) then makes the rest.
 * - the walk and run animations are kept besides the subject ones. */
static int walk_index = -2, run_index = -2;
#define PEOPLE_MAX 64
static struct {
    uint32_t tls, sp, lr, resname, proc, step;
    int k, n;
    uint32_t actors[PEOPLE_MAX];
    uint32_t has_unit[PEOPLE_MAX];
} anims_step;

static void anims_restore(Context *ctx, const uint32_t *words) {
    uint32_t resname = words[0];
    if (walk_index >= 0 && walk_index < 256 && words[1]) mem_write32(ctx, resname + 0x310 + 4 * walk_index, words[1]);
    if (run_index >= 0 && run_index < 256 && words[2]) mem_write32(ctx, resname + 0x310 + 4 * run_index, words[2]);
    if (logging) fprintf(stderr, "[cameras] walk animations kept for Corrin: walk %d (%08X) run %d (%08X)\n", walk_index, words[1], run_index, words[2]);
}

static void anims_clear(Context *ctx, uint32_t resname, uint32_t lr) {
    uint32_t walk = walk_index >= 0 && walk_index < 256 ? mem_read32(ctx, resname + 0x310 + 4 * walk_index) : 0;
    uint32_t run = run_index >= 0 && run_index < 256 ? mem_read32(ctx, resname + 0x310 + 4 * run_index) : 0;
    ctx->r[0] = resname;
    ctx->r[14] = lr;
    ctx->r[15] = CLEAR_EXCEPT_SUBJECT;
    ctx->thumb = 0;
    CALL_THEN(RECOMP_ORIGINAL(0x001F9400), anims_restore, resname, walk, run);
}

/* the view's people: Corrin, then the 7 nearest with a unit */
static void choose_people(Context *ctx) {
    uint32_t proc = anims_step.proc;
    uint32_t player = player_actor(ctx);
    if (!proc || !player || mem_read32(ctx, proc + 0x38) != proc + 0x3C) return;
    V3 at = rv(ctx, player + 0x5C);
    uint32_t chosen[8];
    float dist[8];
    int n = 0;
    chosen[n] = player;
    dist[n++] = -1.0f;
    for (int i = 0; i < anims_step.n; i++) {
        uint32_t actor = anims_step.actors[i];
        if (actor == player || !anims_step.has_unit[i]) continue;
        V3 d = sub(rv(ctx, actor + 0x5C), at);
        float far = d.x * d.x + d.z * d.z;
        int place = n;
        while (place > 1 && dist[place - 1] > far) place--;
        if (place >= 8) continue;
        int last = n < 8 ? n : 7;
        for (int j = last; j > place; j--) {
            chosen[j] = chosen[j - 1];
            dist[j] = dist[j - 1];
        }
        chosen[place] = actor;
        dist[place] = far;
        if (n < 8) n++;
    }
    int before = (int)mem_read32(ctx, proc + 0x34);
    for (int i = 0; i < n; i++) mem_write32(ctx, proc + 0x3C + 4 * i, chosen[i]);
    mem_write32(ctx, proc + 0x34, (uint32_t)n);
    if (logging)
        fprintf(stderr, "[cameras] the view's people: %d nearest of %d castle actors (the game chose %d within 200), the furthest %.0f units away\n", n - 1,
                anims_step.n, before - 1, n > 1 ? sqrtf(dist[n - 1]) : 0.0f);
}

/* the next step of the player's call: GetUnit for the next actor, or the
 * animations' indices, or the clear */
static void anims_next(Context *ctx) {
    if (anims_step.step == 10) {
        if (anims_step.k < anims_step.n) {
            chain(ctx, CLEAR_EXCEPT_SUBJECT, GET_UNIT, anims_step.actors[anims_step.k], 0, 0);
            CALL(recomp_call);
            return;
        }
        choose_people(ctx);
        anims_step.step = 1;
    }
    /* FATES_CAMERAS_ANIMS=1: the animation names (AnimClipNameTable, as
     * AnimClip::IndexByName finds it through IdentHash::GetIdent) */
    static int listed;
    if (anims_step.step == 1 && !listed && getenv("FATES_CAMERAS_ANIMS")) {
        listed = 1;
        anims_step.step = 20;
        chain(ctx, CLEAR_EXCEPT_SUBJECT, 0x00127330u, mem_read32(ctx, mem_read32(ctx, 0x004E2E98u)), 0x004E2E84u, 0);
        CALL(recomp_call);
        return;
    }
    if (walk_index == -2) {
        if (anims_step.step == 1) {
            chain(ctx, CLEAR_EXCEPT_SUBJECT, INDEX_BY_NAME, NAME_WALK, 0, 0);
            CALL(recomp_call);
            return;
        }
    }
    anims_step.step = 0;
    anims_clear(ctx, anims_step.resname, anims_step.lr);
}

RECOMP_OVERRIDE(0x001F9400) {
    /* a return from a chained call: same thread and stack, and not Load
     * calling */
    if (anims_step.step && anims_step.tls == ctx->tls && anims_step.sp == ctx->r[13] && ctx->r[14] != LOAD_PLAYER_CLEAR_RET) {
        if (anims_step.step == 10) {
            anims_step.has_unit[anims_step.k++] = ctx->r[0];
        } else if (anims_step.step == 1) {
            walk_index = (int)(int32_t)ctx->r[0];
            anims_step.step = 2;
            chain(ctx, CLEAR_EXCEPT_SUBJECT, INDEX_BY_NAME, NAME_RUN, 0, 0);
            CALL(recomp_call);
            return;
        } else if (anims_step.step == 2) {
            run_index = (int)(int32_t)ctx->r[0];
            anims_step.step = 3;
        } else if (anims_step.step == 20) {
            uint32_t table = ctx->r[0] ? mem_read32(ctx, ctx->r[0] + 0xC) : 0;
            for (int i = 0; table && i < 256; i++) {
                uint32_t name = mem_read32(ctx, table + 4 * i);
                if (!name) continue;
                char text[48];
                int n = 0;
                for (; n < 47; n++) {
                    text[n] = (char)mem_read8(ctx, name + n);
                    if (!text[n]) break;
                }
                text[n] = 0;
                fprintf(stderr, "[cameras] animation %d: %s\n", i, text);
            }
            anims_step.step = 1;
        }
        anims_next(ctx);
        return;
    }
    anims_step.step = 0;
    /* Load is making a new view: the walk starts over */
    if (ctx->r[14] == LOAD_PLAYER_CLEAR_RET) memset(&wk, 0, sizeof wk);
    HaikuCamFrame frame;
    if (ctx->r[14] != LOAD_PLAYER_CLEAR_RET || !ask(&frame, CAM_QUERY_CASTLE)) {
        CALL(RECOMP_ORIGINAL(0x001F9400));
        return;
    }
    logging = frame.log;
    /* cameras3: the view's proc and that walking is on, from its loading:
     * the follow camera then places the camera already while the view
     * opens (SubjectCamera::UpdateParam, below) */
    wk.proc = ctx->r[4];
    wk.walk_on = 1;
    anims_step.tls = ctx->tls;
    anims_step.sp = ctx->r[13];
    anims_step.lr = ctx->r[14];
    anims_step.resname = ctx->r[0];
    anims_step.proc = ctx->r[4];
    /* the castle's actors, to be asked for their units */
    anims_step.n = 0;
    anims_step.k = 0;
    uint32_t seq = mem_read32(ctx, MAIN_SEQ_PTR);
    uint32_t actor = seq ? mem_read32(ctx, seq + 0x74) : 0;
    for (int guard = 0; actor && guard < 512 && anims_step.n < PEOPLE_MAX; guard++) {
        anims_step.actors[anims_step.n++] = actor;
        actor = mem_read32(ctx, actor + 0x10);
    }
    anims_step.step = env_float("FATES_CAMERAS_PEOPLE", 1.0f) > 0.0f ? 10 : 1;
    anims_next(ctx);
}

/* castle::cut::CreateEffect(const char*, const VEC3&): MovePlayer's dust
 * is made for the castle's small figures; in the view, under a full-size
 * Corrin, it is left out */
RECOMP_OVERRIDE(0x00464E00) {
    if (wk.walking_call) {
        ctx->r[0] = 0;
        ctx->budget -= 4;
        RETURN_TO(ctx->r[14]);
    }
    CALL(RECOMP_ORIGINAL(0x00464E00));
}

/* SubjectCamera::UpdateParam(void) builds the view's camera (eye +0x80,
 * target +0x8C) from its centre (+0xC8, where Corrin stood) and its turn
 * (CameraInput at +0xD4): a look-around from a point by Corrin's head, so
 * once Corrin walks and the camera turns, she can end up beside or behind
 * it. With walking on, the game's camera keeps its direction (the turn the
 * right stick and the mouse make) and is placed behind Corrin instead: it
 * looks at a point by her shoulders, followed damped (her drawn path is
 * smooth already, the damping takes out what is left), from 26 units back
 * (the 3DS's eye is 13.5 from its point), further or nearer with the
 * wheel. HitCheck (TransitionCamera::Tick, after) still keeps the eye out
 * of walls. */
static void castlecam_report(Context *ctx, uint32_t subject, uint32_t unit);
static void follow_tail(Context *ctx, const uint32_t *words) {
    uint32_t subject = words[0], unit = words[1];
    if (!unit || subject != wk.subject) return;
    V3 eye = rv(ctx, subject + 0x80), target = rv(ctx, subject + 0x8C);
    V3 v = norm(sub(target, eye));
    if (len(v) < 0.5f) return;
    V3 right = norm(cross(v, v3(0, 1, 0)));
    V3 at = rv(ctx, unit + 0x1A4);
    V3 want = add(add(at, v3(0, env_float("FATES_CAMERAS_HEIGHT", 13.0f), 0)), mul(right, env_float("FATES_CAMERAS_SIDE", 3.0f)));
    float back = env_float("FATES_CAMERAS_PULL", 26.0f) + wk.zoom;
    if (back < 8.0f) back = 8.0f;
    /* cameras3: while the view opens (the transition's +0x11 not yet set)
     * the camera the game's opening curve flies to is already the follow
     * camera, so it lands there: no jump from the 3DS's look-around point
     * to the follow camera, and no second zoom out after the landing */
    int opening = mem_read8(ctx, subject + 0x11) == 0;
    if (!wk.cam_set || opening) {
        wk.cam = want;
        wk.pull = back;
        wk.cam_set = 1;
    }
    wk.cam = add(wk.cam, mul(sub(want, wk.cam), env_float("FATES_CAMERAS_FOLLOW", 0.35f)));
    wk.pull += (back - wk.pull) * 0.12f;
    wv(ctx, subject + 0x8C, wk.cam);
    wv(ctx, subject + 0x80, sub(wk.cam, mul(v, wk.pull)));
    castlecam_report(ctx, subject, unit); /* castlecam: dev */
}

/* castlecam: dev, FATES_CASTLECAM_LOG (the host writes it): the view's
 * camera each tick, sent through the cameras hook in three parts of 13
 * floats (context 5, the part in `mode`): the SubjectCamera's eye and
 * target as placed now, the field camera's (the last tick's, after
 * HitCheck), the turn input's four numbers (+0xD4: pitch, yaw, roll,
 * distance), Corrin's drawn place, the followed point, the right stick,
 * how far back the camera is, and HitCheck's lift of the eye the tick
 * before (the game's, and as drawn). */
static void castlecam_report(Context *ctx, uint32_t subject, uint32_t unit) {
    static int on = -1;
    if (on < 0) on = getenv("FATES_CASTLECAM_LOG") != 0;
    if (!on || !fates_cameras_hook) return;
    uint32_t cam = field_camera(ctx);
    float v[39];
    memset(v, 0, sizeof v);
    V3 a = rv(ctx, subject + 0x80), b = rv(ctx, subject + 0x8C);
    V3 c = cam ? rv(ctx, cam + 0x04) : v3(0, 0, 0), d = cam ? rv(ctx, cam + 0x10) : v3(0, 0, 0);
    V3 at = rv(ctx, unit + 0x1A4);
    float head[12] = {a.x, a.y, a.z, b.x, b.y, b.z, c.x, c.y, c.z, d.x, d.y, d.z};
    memcpy(v, head, sizeof head);
    for (int i = 0; i < 4; i++) v[12 + i] = rf(ctx, subject + 0xD4 + 4 * i);
    float tail[11] = {at.x, at.y, at.z, wk.cam.x, wk.cam.y, wk.cam.z, wk.look[0], wk.look[1], wk.pull, wk.lift_raw, wk.lift};
    memcpy(v + 16, tail, sizeof tail);
    for (int part = 0; part < 3; part++) {
        HaikuCamFrame f;
        memset(&f, 0, sizeof f);
        f.size = sizeof f;
        f.context = 5;
        f.mode = (uint32_t)part;
        const float *p = v + 13 * part;
        f.orbit[0] = p[0]; f.orbit[1] = p[1]; f.pan[0] = p[2]; f.pan[1] = p[3]; f.zoom = p[4];
        f.look[0] = p[5]; f.look[1] = p[6]; f.turn[0] = p[7]; f.turn[1] = p[8];
        f.move[0] = p[9]; f.move[1] = p[10]; f.move[2] = p[11]; f.dolly = p[12];
        fates_cameras_hook(&f);
    }
}

/* castlecam: CameraState::HitCheck(CameraParam*), the view's camera.
 *
 * TransitionCamera::Tick runs it on the follow camera's eye and target
 * each tick: CamUtil::CalcHitPos lifts the eye above whatever is under it
 * (four short rays down round the eye through the field's collision), and
 * CalcHitUp turns the up vector to suit. The 3DS's look-around sits by
 * Corrin's head, where that never lifts it far; the follow camera is 26
 * units behind her, so turning near a building its eye goes over the
 * building's wall or roof edge, and in one tick the eye jumped up by the
 * wall's height (28 units measured: the view looking down on her from
 * above the roof), then down again when it came out: twice every turn past
 * a building, and the pictures in between of those frames were held (a
 * camera cut). The lift is now eased: a third of what is left each tick,
 * at most 2.5 units up and 2 down, so the camera cranes up over the wall
 * and back down in about 0.4 s instead of jumping. FATES_CASTLECAM_HIT=0: the game's lift as it
 * is. */
static int castlecam_hit_on(void) {
    static int on = -1;
    if (on < 0) {
        const char *v = getenv("FATES_CASTLECAM_HIT");
        on = !(v && v[0] == '0');
    }
    return on;
}

static void hit_tail(Context *ctx, const uint32_t *words) {
    uint32_t p = words[0];
    if (!p || p != wk.hit_param) return;
    V3 out = rv(ctx, p);
    float raw = out.y - wk.hit_eye.y;
    if (!(raw > -1000.0f && raw < 1000.0f)) return;
    wk.lift_raw = raw;
    if (!wk.lift_set) {
        wk.lift = raw;
        wk.lift_set = 1;
    }
    /* towards the game's lift, a third of the way a tick at most 2.5 units
     * up, a fifth at most 2 down: a wall's 28 units in about 0.4 s */
    float step = (raw - wk.lift) * (raw > wk.lift ? 0.33f : 0.2f);
    if (step > 2.5f) step = 2.5f;
    if (step < -2.0f) step = -2.0f;
    wk.lift += step;
    if (fabsf(raw - wk.lift) < 0.02f) wk.lift = raw;
    if (raw == wk.lift) return; /* as the game has it */
    /* the eye where it was, lifted by the eased amount, the up vector as
     * the follow camera had it (the look-at needs no other), and the view
     * angle as it was (HitCheck's third test widens it by how far the eye
     * was moved: that, unchanged, was the whole view jumping in one tick) */
    wv(ctx, p, v3(wk.hit_eye.x, wk.hit_eye.y + wk.lift, wk.hit_eye.z));
    wv(ctx, p + 24, wk.hit_up);
    mem_write32(ctx, p + 36, wk.hit_fov);
}

RECOMP_OVERRIDE(0x00194AEC) {
    uint32_t state = ctx->r[0], p = ctx->r[1];
    if (p && castlecam_hit_on() && wk.enabled && wk.waited && state == wk.subject && castle_view(ctx)) {
        wk.hit_param = p;
        wk.hit_eye = rv(ctx, p);
        wk.hit_up = rv(ctx, p + 24);
        wk.hit_fov = mem_read32(ctx, p + 36);
        CALL_THEN(RECOMP_ORIGINAL(0x00194AEC), hit_tail, p);
        return;
    }
    CALL(RECOMP_ORIGINAL(0x00194AEC));
}

RECOMP_OVERRIDE(0x001CAD0C) {
    uint32_t subject = ctx->r[0];
    /* cameras3: from the view's opening on (walking on when it loaded), not
     * only once WaitTick runs */
    if (castle_view(ctx) && mem_read32(ctx, subject) == SUBJECT_CAMERA_VT && (wk.waited ? wk.enabled : wk.walk_on)) {
        wk.subject = subject;
        uint32_t unit = mem_read32(ctx, wk.proc + 0x5C);
        CALL_THEN(RECOMP_ORIGINAL(0x001CAD0C), follow_tail, subject, unit);
        return;
    }
    CALL(RECOMP_ORIGINAL(0x001CAD0C));
}

/* ---- cameras3: steady shadows in the castle view ----
 *
 * In the castle view (FieldViewing 1) FieldShadow::Update fits the shadow
 * map's light camera round the shadow casters (ShadowActor) that are within
 * 250 units of the eye and in the camera's view (ICamera::IsReject on each
 * caster's box), the union of their boxes (AABB::Expand, then its centre
 * and radius). Walking or turning the follow camera, casters come into and
 * go out of the view every few steps: each time the shadow map is fitted
 * anew (another size and place, every shadow moves and changes its
 * sharpness), and a wall just out of the view stops shadowing the ground
 * in it. That is the buildings, trees and everything else flashing.
 *
 * While the castle view is open with walking on:
 * - a caster's shadow does not depend on the view (IsReject, called from
 *   FieldShadow::Update, says no): the 250 units round the eye decide;
 * - the box (the casters' union) is put on a grid of 32 units and kept
 *   while the casters stay inside it and it is not much larger than they
 *   need: the shadow map is fitted anew now and then (a caster coming
 *   within 250 units, or the casters' box shrinking well inside it), not at
 *   each step and turn. */
#define SHADOW_REJECT_RET 0x0019958Cu /* FieldShadow::Update's call of ICamera::IsReject */
#define SHADOW_EXPAND_RET 0x001995D8u /* its call of AABB::Expand(float, float, float) */

static int shadows_steady(Context *ctx) {
    static int off = -1;
    if (off < 0) off = env_float("FATES_CAMERAS_SHADOWS", 1.0f) <= 0.0f;
    if (off || !castle_view(ctx) || !(wk.waited ? wk.enabled : wk.walk_on)) return 0;
    uint32_t bc = battle_camera(ctx);
    uint32_t state = bc ? current_state(ctx, bc) : 0;
    return state && mem_read32(ctx, state) == SUBJECT_CAMERA_VT;
}

/* ICamera::IsReject(const AABB&) const */
RECOMP_OVERRIDE(0x00544528) {
    if (ctx->r[14] == SHADOW_REJECT_RET && shadows_steady(ctx)) {
        ctx->r[0] = 0;
        ctx->budget -= 8;
        RETURN_TO(ctx->r[14]);
    }
    CALL(RECOMP_ORIGINAL(0x00544528));
}

static struct { V3 lo, hi; int set; uint32_t view; } shadow_kept;

static float snap_down(float v, float g) { return floorf(v / g) * g; }
static float snap_up(float v, float g) { return ceilf(v / g) * g; }

static void shadow_box(Context *ctx, const uint32_t *words) {
    uint32_t box = words[0];
    V3 lo = rv(ctx, box), hi = rv(ctx, box + 12);
    if (!(lo.x <= hi.x && lo.y <= hi.y && lo.z <= hi.z)) return; /* no caster: the game's own fallback */
    /* the casters' box (all of them within 250 units of the eye, now that
     * the view does not choose), on a grid of 32 units */
    const float g = 32.0f;
    V3 want_lo = v3(snap_down(lo.x, g), snap_down(lo.y, g), snap_down(lo.z, g));
    V3 want_hi = v3(snap_up(hi.x, g), snap_up(hi.y, g), snap_up(hi.z, g));
    /* the box kept while the casters stay inside it and it is not much the
     * larger: fitted anew only now and then, not as each one comes and goes */
    if (shadow_kept.set && shadow_kept.view == wk.proc) {
        V3 k0 = shadow_kept.lo, k1 = shadow_kept.hi;
        int inside = want_lo.x >= k0.x && want_lo.y >= k0.y && want_lo.z >= k0.z && want_hi.x <= k1.x && want_hi.y <= k1.y && want_hi.z <= k1.z;
        float kept = len(sub(k1, k0)), wanted = len(sub(want_hi, want_lo));
        if (inside && kept <= wanted * 1.35f + 2.0f * g) {
            want_lo = k0;
            want_hi = k1;
        } else {
            /* refitted: a margin, so the next caster or two do not refit it again */
            want_lo = sub(want_lo, v3(g, 0, g));
            want_hi = add(want_hi, v3(g, 0, g));
        }
    } else {
        want_lo = sub(want_lo, v3(g, 0, g));
        want_hi = add(want_hi, v3(g, 0, g));
    }
    if (logging && (!shadow_kept.set || want_lo.x != shadow_kept.lo.x || want_lo.y != shadow_kept.lo.y || want_lo.z != shadow_kept.lo.z
                    || want_hi.x != shadow_kept.hi.x || want_hi.y != shadow_kept.hi.y || want_hi.z != shadow_kept.hi.z))
        fprintf(stderr, "[cameras] shadow map fitted to %.0f %.0f %.0f .. %.0f %.0f %.0f (casters %.0f %.0f %.0f .. %.0f %.0f %.0f)\n", want_lo.x, want_lo.y,
                want_lo.z, want_hi.x, want_hi.y, want_hi.z, lo.x, lo.y, lo.z, hi.x, hi.y, hi.z);
    shadow_kept.lo = want_lo;
    shadow_kept.hi = want_hi;
    shadow_kept.set = 1;
    shadow_kept.view = wk.proc;
    wv(ctx, box, want_lo);
    wv(ctx, box + 12, want_hi);
}

/* AABB::Expand(float, float, float) */
RECOMP_OVERRIDE(0x003CF12C) {
    if (ctx->r[14] == SHADOW_EXPAND_RET && shadows_steady(ctx)) {
        CALL_THEN(RECOMP_ORIGINAL(0x003CF12C), shadow_box, ctx->r[0]);
        return;
    }
    CALL(RECOMP_ORIGINAL(0x003CF12C));
}

/* ---- cameras3: the battle's comeback goes where the game sends it ----
 *
 * BattleUtil::CalcComebackCamera(CameraParam*) works out where the battle's
 * comeback flies to: it updates the map's camera (map::Camera, or My
 * Castle's FocusCamera), takes the field camera's CameraParam that gives,
 * and runs BattleCamera::Update again. The map's camera reads the field
 * camera as its last ICamera::Update left it (its matrices), and with the
 * free camera engaged that was the free camera's view: the comeback then
 * flew to a point about 1/50 of the way from the field's origin to the
 * map's camera, rolled on its side and low over the ground (the eye at
 * 19, 13, 20 instead of 950, 664, 1000 on st/f11000), the "bug out" when the
 * fight ends. (The map camera's own update goes through the field camera's
 * ICamera::Update, which the free camera took over.) So while the free
 * camera is engaged it stands aside (fc.bypass) for all of it: the field
 * camera is updated once with the game's own view, the game's
 * CalcComebackCamera runs, and then the field camera is updated once more
 * with the free camera's view for this frame's picture. */
static struct { uint32_t tls, sp, lr, param; int step; } calc_step;

RECOMP_OVERRIDE(0x00172FAC) {
    if (calc_step.step && calc_step.tls == ctx->tls && calc_step.sp == ctx->r[13]) {
        uint32_t cam = field_camera(ctx);
        if (calc_step.step == 1) {
            /* back from the field camera's update with the game's view: the
             * game's CalcComebackCamera, the free camera still aside */
            calc_step.step = 2;
            ctx->r[0] = calc_step.param;
            ctx->r[14] = 0x00172FACu;
            CALL(RECOMP_ORIGINAL(0x00172FAC));
            return;
        }
        if (calc_step.step == 2 && cam) {
            /* the comeback's way is worked out: the free camera's view again
             * for this frame's picture */
            calc_step.step = 3;
            fc.bypass = 0;
            if (logging) fprintf(stderr, "[cameras] the comeback's way is worked out from the game's own view\n");
            chain(ctx, 0x00172FACu, 0x004DBB50u, cam, 0, 0);
            CALL(recomp_call);
            return;
        }
        calc_step.step = 0;
        fc.bypass = 0;
        RETURN_TO(calc_step.lr);
    }
    uint32_t cam = field_camera(ctx);
    if (!(fc.engaged || fc.hand_n) || !cam) {
        CALL(RECOMP_ORIGINAL(0x00172FAC));
        return;
    }
    calc_step.tls = ctx->tls;
    calc_step.sp = ctx->r[13];
    calc_step.lr = ctx->r[14];
    calc_step.param = ctx->r[0];
    calc_step.step = 1;
    fc.bypass = 1;
    chain(ctx, 0x00172FACu, 0x004DBB50u, cam, 0, 0);
    CALL(recomp_call);
}
