/* audio: the native sound layer, see zakuro-core/src/pcos/snd/.
 *
 * nn::snd's DSP driver, nn::snd::CTR::Dspsnd, is the only code in the game
 * that talks to the audio DSP: it writes voice and mixer parameters into
 * the DSP's shared memory, signals the DSP, waits on its interrupt event
 * and reads the voice statuses back. Every Dspsnd method is replaced here
 * by a call into the native mixer with the method's own arguments, so the
 * DSP's memory, pipes, semaphore and interrupt are never used:
 *
 *   - the channel and master setters (SetChannelTimer, SetChannelMix,
 *     AssignPCM, AppendChannelNextBuffer, SetMasterVolume, ...) are leaves
 *     and go straight to the mixer;
 *   - WaitPipe sleeps the sound thread (nn::os::Thread::Sleep, a plain
 *     svcSleepThread) until the native mixer's next frame is due;
 *   - SyncFrameData renders the due frame(s) natively, then hands each
 *     voice's status to nn::snd's own VoiceManager::UpdateStatus;
 *   - SendParameter runs the game's aux effects and voice updates, the same
 *     calls the original makes, then commits.
 *
 * nn::aacdec (the voice clips are ADTS AAC) decodes on the host, and the
 * DSP component load and cache maintenance are answered without IPC.
 *
 * Guest calls made from inside an override come back into it: their
 * return address is the override's own address, and a small per-thread
 * record says how far it got. If a callee blocks (a contended critical
 * section switches threads, which unwinds the C stack), the host later
 * resumes the callee, which returns to the override's address, which the
 * entry table maps to the override, which carries on from its record.
 *
 * FATES_NATIVE_AUDIO=0 makes every override here run the original code
 * (the emulated DSP route), for comparisons.
 */
#include <string.h>
#include "overrides.h"

typedef int (*OsNativeHook)(Context *ctx, uint32_t op);
extern OsNativeHook os_native_hook;

/* call numbers, the same as pcos::snd::op (0x500-0x5FF; os uses up to 0x4FF) */
enum {
    SND_INITIALIZE = 0x500,
    SND_ENABLED = 0x501,
    SND_FINALIZE = 0x502,
    SND_WAIT = 0x503,
    SND_SYNC = 0x504,
    SND_SEND = 0x505,

    SND_CHANNEL_INITIALIZE = 0x510,
    SND_CHANNEL_PLAY_START,
    SND_CHANNEL_PLAY_STOP,
    SND_CHANNEL_SYNC_COUNT,
    SND_CHANNEL_TIMER,
    SND_CHANNEL_RIM,
    SND_CHANNEL_MIX,
    SND_CHANNEL_ADPCM_PARAM,
    SND_CHANNEL_FILTER_TYPE,
    SND_CHANNEL_FILTER_MONO,
    SND_CHANNEL_FILTER_BIQUAD,
    SND_CHANNEL_RESET_NEXT,
    SND_CHANNEL_APPEND_NEXT,
    SND_CHANNEL_UPDATE_NEXT,
    SND_CHANNEL_ASSIGN_PCM,

    SND_MASTER_VOLUME = 0x530,
    SND_AUX_RETURN_VOLUME,
    SND_ENABLE_AUX_BUS,
    SND_OUTPUT_MODE,
    SND_AUX_FRONT_BYPASS,
    SND_CLIPPING_MODE,
    SND_SURROUND_DEPTH,
    SND_SURROUND_POSITION,
    SND_REAR_RATIO,
    SND_HEADSET,
    SND_OUTPUT_BUFFER_COUNT,
    SND_SYNC_MODE,
    SND_DSP_DELAY,
    SND_DSP_REVERB,
    SND_DROPPED_FRAMES,

    SND_DSP_LOAD_COMPONENT = 0x540,
    SND_DSP_UNLOAD_COMPONENT,
    SND_DSP_HEADPHONE_OUT,
    SND_DSP_FLUSH,
    SND_DSP_INVALIDATE,
    SND_DSP_UNREACHED,

    SND_AAC_INITIALIZE = 0x550,
    SND_AAC_OPEN,
    SND_AAC_DECODE,
    SND_AAC_WAIT,
    SND_AAC_USED_BYTES,
    SND_AAC_RATE_INDEX,
    SND_AAC_CHANNELS,
    SND_AAC_FINALIZE,
    SND_AAC_STATE_BUFFER,
    SND_AAC_SLEEP,
    SND_AAC_WAKE_UP,
};

/* a leaf: the native layer answers it, or the original runs */
#define SND(op, address) do { \
    if (os_native_hook && os_native_hook(ctx, (op))) RETURN_TO(ctx->r[14]); \
    CALL(RECOMP_ORIGINAL(address)); \
} while (0)

/* whether the native layer is on (FATES_NATIVE_AUDIO), asked once */
static int snd_on(Context *ctx) {
    static int on = -1;
    if (on < 0) {
        uint32_t r0 = ctx->r[0];
        on = os_native_hook && os_native_hook(ctx, SND_ENABLED);
        ctx->r[0] = r0;
    }
    return on;
}

/* ---- guest calls that come back into the override ---- */

typedef struct {
    uint32_t tls, site, step, lr, self;
    uint32_t a[4];
} SndSeq;

static SndSeq snd_seqs[16];

static SndSeq *seq_find(Context *ctx, uint32_t site) {
    for (unsigned i = 0; i < sizeof snd_seqs / sizeof *snd_seqs; i++)
        if (snd_seqs[i].site == site && snd_seqs[i].tls == ctx->tls)
            return &snd_seqs[i];
    return 0;
}

static SndSeq *seq_new(Context *ctx, uint32_t site, uint32_t self) {
    for (unsigned i = 0; i < sizeof snd_seqs / sizeof *snd_seqs; i++)
        if (!snd_seqs[i].site) {
            SndSeq *s = &snd_seqs[i];
            memset(s, 0, sizeof *s);
            s->site = site;
            s->tls = ctx->tls;
            s->lr = ctx->r[14];
            s->self = self;
            return s;
        }
    return 0;
}

/* calls a guest function whose return comes back to the override */
#define SEQ_CALL(s, address, fn) do { \
    ctx->r[14] = (s)->self; \
    ctx->r[15] = (address); \
    ctx->thumb = 0; \
    CALL(fn); \
} while (0)

#define SEQ_RETURN(s) do { \
    uint32_t lr_ = (s)->lr; \
    (s)->site = 0; \
    RETURN_TO(lr_); \
} while (0)

enum { SITE_SYNC = 1, SITE_SEND, SITE_WAIT_TIMEOUT, SITE_SET_STATE, SITE_TEST };

/* ---- nn::snd::CTR::Dspsnd ---- */

/* the offsets of the driver's own fields this file keeps up (the USA
   build's; D() gives the running build's) */
#define DSPSND_INITIALIZED 0x1310
#define DSPSND_AUX 0x12E8          /* aux A/B buffers, two regions */
#define DSPSND_DSP_CYCLES 0x1318   /* the DSP time voices may use */
#define DSPSND_DSP_DEBUG 0x131C    /* the copy of the DSP's debug words */

/* jpn: the Japanese build's nn::snd is another SDK release, whose Dspsnd has
   12 more bytes before its output capture pointer (USA +0xC, JPN +0x18):
   every field from +0xC on is 0xC further (checked by disassembly of
   Initialize, InitializeVariables, SyncFrameData and SendParameter in both
   builds: +0x10 -> +0x1C, 0x1090 -> 0x109C, 0x12E8 -> 0x12F4, 0x1310 ->
   0x131C, 0x15A4 -> 0x15B0). Its methods also take a driver mutex, which
   none of these overrides need: they replace the whole method. */
int haiku_code_is_jpn(void); /* eur_region.c */
static uint32_t D(uint32_t offset) {
    static int jpn = -1;
    if (jpn < 0) jpn = haiku_code_is_jpn();
    return offset + (jpn && offset >= 0xC ? 0xCu : 0u);
}

/* Dspsnd::Initialize(bool waking) */
RECOMP_OVERRIDE(0x002F4454) {
    if (!snd_on(ctx)) { CALL(RECOMP_ORIGINAL(0x002F4454)); return; }
    uint32_t self = ctx->r[0], waking = ctx->r[1] & 0xFF, lr = ctx->r[14];
    uint32_t initialized = mem_read32(ctx, 0x002F45E8);
    if (!mem_read8(ctx, self + initialized)) {
        os_native_hook(ctx, SND_INITIALIZE); /* r0, r1: the native aux buffers */
        uint32_t aux0 = ctx->r[0], aux1 = ctx->r[1];
        mem_write8(ctx, self + D(DSPSND_INITIALIZED), 1);
        mem_write8(ctx, self + D(0x1311), 0);
        mem_write16(ctx, self + D(0x1312), 5);
        mem_write16(ctx, self + D(0x1314), 0);
        mem_write16(ctx, self + D(0x1316), 1);
        mem_write32(ctx, self + D(DSPSND_AUX) + 0x0, aux0);
        mem_write32(ctx, self + D(DSPSND_AUX) + 0x4, aux1);
        mem_write32(ctx, self + D(DSPSND_AUX) + 0x8, aux0);
        mem_write32(ctx, self + D(DSPSND_AUX) + 0xC, aux1);
        mem_write32(ctx, self + D(DSPSND_DSP_DEBUG), 0);
        if (!waking) {
            mem_write8(ctx, self + mem_read32(ctx, 0x002F45FC), 1);
            mem_write32(ctx, self + D(0xC), 0); /* no output capture */
        }
    }
    ctx->r[0] = 0;
    RETURN_TO(lr);
}

/* Dspsnd::Finalize(bool sleeping) */
RECOMP_OVERRIDE(0x002F5AE8) {
    if (!snd_on(ctx)) { CALL(RECOMP_ORIGINAL(0x002F5AE8)); return; }
    uint32_t self = ctx->r[0], lr = ctx->r[14];
    os_native_hook(ctx, SND_FINALIZE);
    mem_write8(ctx, self + D(DSPSND_INITIALIZED), 0);
    ctx->r[0] = 0;
    RETURN_TO(lr);
}

/* Dspsnd::WaitPipe(void): sleep until the next frame is due, then return
   straight to the caller (WaitForDspSync), which goes on to SyncFrameData */
RECOMP_OVERRIDE(0x002F5C80) {
    if (!snd_on(ctx) || !os_native_hook(ctx, SND_WAIT)) { CALL(RECOMP_ORIGINAL(0x002F5C80)); return; }
    if (!ctx->r[0] && !ctx->r[1]) RETURN_TO(ctx->r[14]);
    /* nn::os::Thread::SleepImpl(nanoseconds in r0:r1), a tail call */
    ctx->r[15] = 0x001334FCu;
    ctx->thumb = 0;
    CALL(f_001334FC);
}

/* Dspsnd::WaitPipe(TimeSpan) -> whether a frame came */
RECOMP_OVERRIDE(0x002F5C10) {
    if (!snd_on(ctx)) { CALL(RECOMP_ORIGINAL(0x002F5C10)); return; }
    SndSeq *s = seq_find(ctx, SITE_WAIT_TIMEOUT);
    if (s) { ctx->r[0] = 1; SEQ_RETURN(s); }
    os_native_hook(ctx, SND_WAIT);
    if ((!ctx->r[0] && !ctx->r[1]) || !(s = seq_new(ctx, SITE_WAIT_TIMEOUT, 0x002F5C10u))) {
        ctx->r[0] = 1;
        RETURN_TO(ctx->r[14]);
    }
    SEQ_CALL(s, 0x001334FCu, f_001334FC);
    ctx->r[0] = 1;
    SEQ_RETURN(s);
}

/* Dspsnd::SyncFrameData: render, then nn::snd's status updates */
RECOMP_OVERRIDE(0x002F49F0) {
    if (!snd_on(ctx)) { CALL(RECOMP_ORIGINAL(0x002F49F0)); return; }
    SndSeq *s = seq_find(ctx, SITE_SYNC);
    if (!s) {
        uint32_t self = ctx->r[0], lr = ctx->r[14];
        if (!os_native_hook(ctx, SND_SYNC) || !ctx->r[0]) RETURN_TO(lr);
        uint32_t statuses = ctx->r[1], final_samples = ctx->r[2];
        ctx->r[14] = lr;
        s = seq_new(ctx, SITE_SYNC, 0x002F49F0u);
        if (!s) RETURN_TO(lr);
        s->a[0] = self;
        s->a[1] = statuses;
        s->a[2] = final_samples;
    }
    for (;;) {
        uint32_t step = s->step++;
        if (step < 24) {
            /* VoiceManager::UpdateStatus(int, const DspsndChannelPlayVars*) */
            ctx->r[0] = mem_read32(ctx, 0x002F4AF4u);
            ctx->r[1] = step;
            ctx->r[2] = s->a[1] + step * 12;
            SEQ_CALL(s, 0x002F7300u, f_002F7300);
            continue;
        }
        if (step == 24) {
            /* OutputCapture::Write(short*, int), when the game captures */
            uint32_t capture = mem_read32(ctx, s->a[0] + D(0xC));
            if (capture && mem_read8(ctx, capture + 0x10)) {
                ctx->r[0] = capture;
                ctx->r[1] = s->a[2];
                ctx->r[2] = 0xA0;
                SEQ_CALL(s, 0x002F2B4Cu, f_002F2B4C);
            }
            continue;
        }
        if (step == 25) {
            /* MasterManager::UpdateDroppedSoundFrameCount */
            ctx->r[0] = mem_read32(ctx, 0x002F4AF8u);
            SEQ_CALL(s, 0x002F29DCu, f_002F29DC);
            continue;
        }
        SEQ_RETURN(s);
    }
}

/* Dspsnd::SendParameter: the game's effects and voice updates, then commit */
RECOMP_OVERRIDE(0x002F4748) {
    if (!snd_on(ctx)) { CALL(RECOMP_ORIGINAL(0x002F4748)); return; }
    SndSeq *s = seq_find(ctx, SITE_SEND);
    if (!s) {
        uint32_t self = ctx->r[0];
        s = seq_new(ctx, SITE_SEND, 0x002F4748u);
        if (!s) RETURN_TO(ctx->r[14]);
        s->a[0] = self;
    }
    uint32_t self = s->a[0];
    uint32_t master = mem_read32(ctx, 0x002F4888u), voices = mem_read32(ctx, 0x002F488Cu);
    uint32_t aux = mem_read32(ctx, 0x002F4890u);
    uint32_t region = (mem_read16(ctx, self + D(0x1314)) & 1) * 8;
    uint32_t aux0 = mem_read32(ctx, self + aux + region), aux1 = mem_read32(ctx, self + region + D(0x12EC));
    for (;;) {
        uint32_t step = s->step++;
        switch (step) {
        case 0:
        case 1:
            /* MasterManager::AuxUserCallback(AuxBusId, buffer) */
            if (mem_read8(ctx, self + aux + 700)) {
                ctx->r[0] = master;
                ctx->r[1] = step;
                ctx->r[2] = step ? aux1 : aux0;
                SEQ_CALL(s, 0x002F2900u, f_002F2900);
            }
            continue;
        case 2:
        case 3:
            /* MasterManager::ExecuteEffect(AuxBusId, buffer): FxReverb/FxDelay */
            ctx->r[0] = master;
            ctx->r[1] = step - 2;
            ctx->r[2] = step == 3 ? aux1 : aux0;
            SEQ_CALL(s, 0x002F287Cu, f_002F287C);
            continue;
        case 4:
            ctx->r[0] = voices; /* VoiceManager::UpdateParams */
            SEQ_CALL(s, 0x002F23F4u, f_002F23F4);
            continue;
        case 5:
            ctx->r[0] = voices; /* VoiceManager::UpdateWaveBufferList */
            SEQ_CALL(s, 0x002F25D4u, f_002F25D4);
            continue;
        case 6:
            ctx->r[0] = master; /* MasterManager::GetDspCycles */
            SEQ_CALL(s, 0x002F27FCu, f_002F27FC);
            continue;
        case 7:
            s->a[1] = ctx->r[0]; /* DspFxManager::GetInstance */
            SEQ_CALL(s, 0x002F20C4u, f_002F20C4);
            continue;
        case 8:
            /* DspFxManager::GetDspCycles, on the instance in r0 */
            SEQ_CALL(s, 0x002F20D0u, f_002F20D0);
            continue;
        case 9:
            s->a[2] = ctx->r[0];
            /* VoiceManager::AdjustVoicePlayState(cycles left, the DSP's) */
            ctx->r[0] = voices;
            ctx->r[1] = mem_read32(ctx, self + D(DSPSND_DSP_CYCLES)) - s->a[1] - s->a[2];
            ctx->r[2] = mem_read32(ctx, self + D(DSPSND_DSP_DEBUG));
            SEQ_CALL(s, 0x002F245Cu, f_002F245C);
            continue;
        default:
            os_native_hook(ctx, SND_SEND);
            mem_write8(ctx, self + D(0x1311), mem_read8(ctx, self + D(0x1311)) + 1);
            SEQ_RETURN(s);
        }
    }
}

/* Voice::SetState and VoiceImpl::SetState: Stop (1) stops the voice and
   resets it, Pause (2) stops it. The SDK reaches Dspsnd's PlayStop and
   InitializeChannelParameters by tail branches that the recompiler folded
   into these two functions, so they are replaced as a whole. */
static void snd_voice_state(Context *ctx, uint32_t impl, uint32_t state) {
    uint32_t driver = mem_read32(ctx, 0x002F7A68u), channel = mem_read32(ctx, impl) & 0xFF;
    mem_write8(ctx, impl + 0xD, state);
    if (state == 1 || state == 2) {
        ctx->r[0] = driver; ctx->r[1] = channel;
        os_native_hook(ctx, SND_CHANNEL_PLAY_STOP);
    }
    if (state == 1) {
        mem_write8(ctx, impl + 0xC, 0);
        ctx->r[0] = driver; ctx->r[1] = channel;
        os_native_hook(ctx, SND_CHANNEL_INITIALIZE);
    }
    ctx->r[0] = state == 1 || state == 2 ? 1 : impl;
}

/* VoiceImpl::SetState(State) */
RECOMP_OVERRIDE(0x002F7A00) {
    if (!snd_on(ctx)) { CALL(RECOMP_ORIGINAL(0x002F7A00)); return; }
    uint32_t lr = ctx->r[14];
    snd_voice_state(ctx, ctx->r[0], ctx->r[1] & 0xFF);
    RETURN_TO(lr);
}

/* Voice::SetState(State): Stop first releases the voice's wave buffers
   (VoiceImpl::ReleaseWaveBuffer, which takes the voice's lock) */
RECOMP_OVERRIDE(0x002F79D4) {
    if (!snd_on(ctx)) { CALL(RECOMP_ORIGINAL(0x002F79D4)); return; }
    SndSeq *s = seq_find(ctx, SITE_SET_STATE);
    if (!s) {
        uint32_t voice = ctx->r[0], state = ctx->r[1] & 0xFF;
        mem_write8(ctx, voice + 4, state);
        if (state != 1) {
            uint32_t lr = ctx->r[14];
            snd_voice_state(ctx, mem_read32(ctx, voice + 0x68), state);
            RETURN_TO(lr);
        }
        s = seq_new(ctx, SITE_SET_STATE, 0x002F79D4u);
        if (!s) RETURN_TO(ctx->r[14]);
        s->a[0] = voice;
        ctx->r[0] = mem_read32(ctx, voice + 0x68);
        s->step = 1;
        SEQ_CALL(s, 0x002F7844u, f_002F7844);
    }
    snd_voice_state(ctx, mem_read32(ctx, s->a[0] + 0x68), 1);
    SEQ_RETURN(s);
}

/* the channel setters */
RECOMP_OVERRIDE(0x002F5A80) { SND(SND_CHANNEL_INITIALIZE, 0x002F5A80); } /* InitializeChannelParameters */
RECOMP_OVERRIDE(0x002F538C) { SND(SND_CHANNEL_PLAY_START, 0x002F538C); }  /* SetChannelPlayStart */
RECOMP_OVERRIDE(0x002F4EAC) { SND(SND_CHANNEL_PLAY_STOP, 0x002F4EAC); }   /* SetChannelPlayStop */
RECOMP_OVERRIDE(0x002F53FC) { SND(SND_CHANNEL_SYNC_COUNT, 0x002F53FC); }  /* SetChannelSyncCount */
RECOMP_OVERRIDE(0x002F4AFC) { SND(SND_CHANNEL_TIMER, 0x002F4AFC); }       /* SetChannelTimer(float) */
RECOMP_OVERRIDE(0x002F4970) { SND(SND_CHANNEL_RIM, 0x002F4970); }         /* SetChannelRIM */
RECOMP_OVERRIDE(0x002F4894) { SND(SND_CHANNEL_MIX, 0x002F4894); }         /* SetChannelMix */
RECOMP_OVERRIDE(0x002F54BC) { SND(SND_CHANNEL_ADPCM_PARAM, 0x002F54BC); } /* SetChannelAdpcmParam */
RECOMP_OVERRIDE(0x002F57E0) { SND(SND_CHANNEL_FILTER_TYPE, 0x002F57E0); } /* SetChannelIirFilterType */
RECOMP_OVERRIDE(0x002F5910) { SND(SND_CHANNEL_FILTER_MONO, 0x002F5910); } /* SetChannelIIRFilter_Mono */
RECOMP_OVERRIDE(0x002F5988) { SND(SND_CHANNEL_FILTER_BIQUAD, 0x002F5988); } /* SetChannelIIRFilter_Biquad */
RECOMP_OVERRIDE(0x002F55FC) { SND(SND_CHANNEL_RESET_NEXT, 0x002F55FC); }  /* ResetChannelNextBuffer */
RECOMP_OVERRIDE(0x002F5664) { SND(SND_CHANNEL_APPEND_NEXT, 0x002F5664); } /* AppendChannelNextBuffer */
RECOMP_OVERRIDE(0x002F5850) { SND(SND_CHANNEL_UPDATE_NEXT, 0x002F5850); } /* UpdateChannelNextBuffer */
RECOMP_OVERRIDE(0x002F5CD8) { SND(SND_CHANNEL_ASSIGN_PCM, 0x002F5CD8); }  /* AssignPCM */

/* the master setters */
RECOMP_OVERRIDE(0x002F4BD8) { SND(SND_MASTER_VOLUME, 0x002F4BD8); }       /* SetMasterVolume(float) */
RECOMP_OVERRIDE(0x002F4E34) { SND(SND_AUX_RETURN_VOLUME, 0x002F4E34); }   /* SetAuxReturnVolume(float) */
RECOMP_OVERRIDE(0x002F4654) { SND(SND_ENABLE_AUX_BUS, 0x002F4654); }      /* EnableAuxBus */
RECOMP_OVERRIDE(0x002F508C) { SND(SND_OUTPUT_MODE, 0x002F508C); }         /* SetSoundOutputMode */
RECOMP_OVERRIDE(0x002F4CA4) { SND(SND_AUX_FRONT_BYPASS, 0x002F4CA4); }    /* SetAuxFrontBypass */
RECOMP_OVERRIDE(0x002F4B78) { SND(SND_CLIPPING_MODE, 0x002F4B78); }       /* SetClippingMode */
RECOMP_OVERRIDE(0x002F4C38) { SND(SND_SURROUND_DEPTH, 0x002F4C38); }      /* SetSurroundDepth */
RECOMP_OVERRIDE(0x002F5A14) { SND(SND_SURROUND_POSITION, 0x002F5A14); }   /* SetSurroundSpeakerPosition */
RECOMP_OVERRIDE(0x002F46E0) { SND(SND_REAR_RATIO, 0x002F46E0); }          /* SetRearRatio */
RECOMP_OVERRIDE(0x002F559C) { SND(SND_HEADSET, 0x002F559C); }             /* SetIsHeadsetConnected */
RECOMP_OVERRIDE(0x002F5548) { SND(SND_OUTPUT_BUFFER_COUNT, 0x002F5548); } /* SetOutputBufferCount */
RECOMP_OVERRIDE(0x002F4600) { SND(SND_SYNC_MODE, 0x002F4600); }           /* SetSyncMode */
RECOMP_OVERRIDE(0x002F4D30) { SND(SND_DSP_DELAY, 0x002F4D30); }           /* SetDspDelayEffect */
RECOMP_OVERRIDE(0x002F4F20) { SND(SND_DSP_REVERB, 0x002F4F20); }          /* SetDspReverbEffect */
RECOMP_OVERRIDE(0x002F546C) { SND(SND_DROPPED_FRAMES, 0x002F546C); }      /* GetDroppedFrameCount */

/* ---- nn::dsp: the firmware load and cache maintenance, without IPC ---- */
RECOMP_OVERRIDE(0x0012B7E8) { SND(SND_DSP_LOAD_COMPONENT, 0x0012B7E8); }   /* DSP::LoadComponent */
RECOMP_OVERRIDE(0x0012B848) { SND(SND_DSP_UNLOAD_COMPONENT, 0x0012B848); } /* DSP::UnloadComponent */
RECOMP_OVERRIDE(0x0012B870) { SND(SND_DSP_HEADPHONE_OUT, 0x0012B870); }    /* DSP::ForceHeadphoneOut */
RECOMP_OVERRIDE(0x0024292C) { SND(SND_DSP_FLUSH, 0x0024292C); }            /* nn::snd::FlushDataCache */
RECOMP_OVERRIDE(0x00242930) { SND(SND_DSP_FLUSH, 0x00242930); }            /* nn::dsp::FlushDataCache */
RECOMP_OVERRIDE(0x00242DAC) { SND(SND_DSP_FLUSH, 0x00242DAC); }            /* DSP::FlushDataCache */
RECOMP_OVERRIDE(0x00242AD4) { SND(SND_DSP_INVALIDATE, 0x00242AD4); }       /* nn::dsp::InvalidateDataCache */
RECOMP_OVERRIDE(0x00242F14) { SND(SND_DSP_INVALIDATE, 0x00242F14); }       /* DSP::InvalidateDataCache */

/* the DSP's pipe and semaphore: nothing reaches them with the native layer
   on; if something does, it is logged once and emulated */
#define SND_UNREACHED(address) do { \
    if (snd_on(ctx)) { uint32_t r12_ = ctx->r[12]; ctx->r[12] = (address); os_native_hook(ctx, SND_DSP_UNREACHED); ctx->r[12] = r12_; } \
    CALL(RECOMP_ORIGINAL(address)); \
} while (0)
RECOMP_OVERRIDE(0x00242D74) { SND_UNREACHED(0x00242D74); } /* DSP::SetSemaphore */
RECOMP_OVERRIDE(0x00242DE8) { SND_UNREACHED(0x00242DE8); } /* DSP::RecvDataIsReady */
RECOMP_OVERRIDE(0x00242E2C) { SND_UNREACHED(0x00242E2C); } /* DSP::SetSemaphoreMask */
RECOMP_OVERRIDE(0x00242E64) { SND_UNREACHED(0x00242E64); } /* DSP::WriteProcessPipe */
RECOMP_OVERRIDE(0x00242EAC) { SND_UNREACHED(0x00242EAC); } /* DSP::ReadPipeIfPossible */
RECOMP_OVERRIDE(0x00242F50) { SND_UNREACHED(0x00242F50); } /* DSP::GetSemaphoreEventHandle */
RECOMP_OVERRIDE(0x00242F84) { SND_UNREACHED(0x00242F84); } /* DSP::RegisterInterruptEvents */
RECOMP_OVERRIDE(0x00242FC0) { SND_UNREACHED(0x00242FC0); } /* DSP::ConvertProcessAddressFromDspDram */
RECOMP_OVERRIDE(0x00242FFC) { SND_UNREACHED(0x00242FFC); } /* DSP::RecvData */

/* ---- nn::aacdec: AAC voice clips decoded on the host ---- */
RECOMP_OVERRIDE(0x00303AE8) { SND(SND_AAC_INITIALIZE, 0x00303AE8); }  /* Initialize */
RECOMP_OVERRIDE(0x00303DAC) { SND(SND_AAC_OPEN, 0x00303DAC); }        /* Open */
RECOMP_OVERRIDE(0x00303FE4) { SND(SND_AAC_DECODE, 0x00303FE4); }      /* Decode */
RECOMP_OVERRIDE(0x00303E54) { SND(SND_AAC_WAIT, 0x00303E54); }        /* Wait */
RECOMP_OVERRIDE(0x00303C50) { SND(SND_AAC_USED_BYTES, 0x00303C50); }  /* GetUsedBytes */
RECOMP_OVERRIDE(0x00303D4C) { SND(SND_AAC_CHANNELS, 0x00303D4C); }    /* GetChannelConfig */
RECOMP_OVERRIDE(0x0030432C) { SND(SND_AAC_FINALIZE, 0x0030432C); }    /* Finalize */
RECOMP_OVERRIDE(0x00303C84) { SND(SND_AAC_STATE_BUFFER, 0x00303C84); } /* SetStateBuffer */
RECOMP_OVERRIDE(0x00303EF0) { SND(SND_AAC_SLEEP, 0x00303EF0); }       /* Sleep */
RECOMP_OVERRIDE(0x003041C4) { SND(SND_AAC_WAKE_UP, 0x003041C4); }     /* WakeUp */

/* GetSampleRate: the SDK's own table, at the index the decoder found */
RECOMP_OVERRIDE(0x00303C60) {
    if (!snd_on(ctx) || !os_native_hook(ctx, SND_AAC_RATE_INDEX)) { CALL(RECOMP_ORIGINAL(0x00303C60)); return; }
    int32_t index = (int32_t)ctx->r[0];
    ctx->r[0] = index < 0 ? 0xFFFFFFFFu : mem_read32(ctx, mem_read32(ctx, 0x00303C80u) + (uint32_t)index * 4);
    RETURN_TO(ctx->r[14]);
}

/* ---- a test hook: FATES_AUDIO_TEST=label@frame,... plays a sound by its
   archive label at a game frame (Sound::SEPlay from SoundPrivate::Tick, on
   the game's main thread), in either route, to reach sounds no replay
   reaches (the AAC voice clips) ---- */
enum { SND_TEST_LABEL = 0x506 };

/* SoundPrivate::Tick(int) */
RECOMP_OVERRIDE(0x00104CA8) {
    SndSeq *s = seq_find(ctx, SITE_TEST);
    if (!s) {
        uint32_t ticks = ctx->r[0];
        if (!os_native_hook || !os_native_hook(ctx, SND_TEST_LABEL) || !ctx->r[0]) {
            ctx->r[0] = ticks;
            CALL(RECOMP_ORIGINAL(0x00104CA8));
            return;
        }
        uint32_t label = ctx->r[0];
        if (!(s = seq_new(ctx, SITE_TEST, 0x00104CA8u))) { ctx->r[0] = ticks; CALL(RECOMP_ORIGINAL(0x00104CA8)); return; }
        s->a[0] = ticks;
        ctx->r[0] = label;
        SEQ_CALL(s, 0x004200FCu, f_004200FC); /* Sound::SEPlay(const char*) */
    }
    /* then the tick itself, returning to Tick's caller */
    ctx->r[0] = s->a[0];
    ctx->r[14] = s->lr;
    s->site = 0;
    CALL(RECOMP_ORIGINAL(0x00104CA8));
}

/* pulsefix: what of this file is the console's state, kept with Divine
   Pulse's moments and saved states (overrides/keep.c) */
void snd_native_keep(void (*add)(const char *, void *, uint32_t)) {
    add("snd_native.snd_seqs", &snd_seqs, sizeof snd_seqs);
}
