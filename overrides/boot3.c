/* boot3: the black screen at start and at Return to Title
 * (dev/reports/boot3.md).
 *
 * ProcThread is the game's proc that runs work on a thread of its own
 * (ProcExtRead, the extra data read at every start, is one; so are save,
 * load and content procs). Its Tick, state 1, creates the thread and only
 * then marks it running:
 *
 *     ThreadController::CreateThread(ProcThread::ThreadFunction, this, ...);
 *     this->running (+0x78) = 1;
 *     state++;
 *
 * and ProcThread::ThreadFunction, on the new thread, runs Exec and then
 * clears it (this->running = 0). State 2 waits, a frame at a time, for
 * running to be 0. If the worker runs Exec to its end before the creating
 * thread stores its 1, the 1 stays and state 2 waits forever. On the
 * console the worker's file reads wait on the FS service, which always lets
 * the creating thread store its 1 first. On PC the reads are native and
 * rarely wait, and in real time the worker can get the core first (it runs
 * at a higher priority, 31 for ProcExtRead, or gets a turn through the
 * scheduler's Aging rule). At every start that is ProcExtRead, inside
 * link::ProcInit: the game's main loop runs on, the screen stays black, and
 * the HOME button stays held off (Begin's SleepHomeBind), so Return to
 * Title (the game's soft reset) waits forever too, and a Return to Title
 * starts a boot that can hang the same way. Seen in the hung runs' proc
 * log: ProcExtRead in state 2, running 1, its timer 0, its worker gone.
 *
 * The fix, in real time (the host sets boot3_procthread_guard; the
 * deterministic mode of replays and the gate keeps the game's own order,
 * which its cycle timeline does not break): ProcThread::ThreadFunction
 * starts the work only once its proc is marked running. Until then the
 * worker sleeps 0.5 ms at a time (svcSleepThread, so the creating thread
 * gets the core and stores its 1), and then runs ThreadFunction as the
 * game wrote it. After 2000 sleeps (a second) it goes on regardless.
 * The sleep returns into ThreadFunction's entry (this override) with the
 * proc in r12 and the return address kept here.
 * FATES_BOOT3_PROCTHREAD=0 turns it off, =1 forces it in deterministic runs;
 * =test (2) holds every worker for three sleeps even when it is marked
 * (tests of the sleep and the way back).
 */
#include "overrides.h"

uint32_t boot3_procthread_guard = 0;   /* set by the host: 1 in real time */
uint32_t boot3_procthread_held = 0;    /* read by the host: workers that waited to be marked running */
uint32_t boot3_procthread_sleeps = 0;  /* read by the host: their sleeps */
uint32_t boot3_procthread_gave_up = 0; /* read by the host: workers that went on unmarked after a second */

#define PROCTHREAD_RUNNING 0x78u        /* u8: the worker runs */
#define THREAD_FUNCTION    0x0018D7FCu  /* ProcThread::ThreadFunction(void*) */
#define SVC_SLEEP_THREAD   0x00133500u  /* .nn.svc.SleepThread: svc 0x0A; bx lr */
#define BOOT3_SLEEP_NS     500000u
#define BOOT3_MAX_SLEEPS   2000u

/* the workers waiting: their proc, the return address ThreadFunction was
   called with, their sleeps. Only the thread holding the core touches it. */
#define BOOT3_SLOTS 16
static struct { uint32_t proc; uint32_t lr; uint32_t sleeps; } boot3_waiting[BOOT3_SLOTS];

static int boot3_slot(uint32_t proc) {
    for (int i = 0; i < BOOT3_SLOTS; i++)
        if (boot3_waiting[i].proc == proc)
            return i;
    return -1;
}

/* ProcThread::ThreadFunction(void* proc), on the worker */
RECOMP_OVERRIDE(0x0018D7FC) {
    if (!*(volatile uint32_t *)&boot3_procthread_guard) {
        CALL(RECOMP_ORIGINAL(0x0018D7FC));
        return;
    }
    /* back from our sleep: r0 is its result (0), r12 the proc */
    uint32_t proc = ctx->r[0];
    int slot = -1;
    if (proc == 0) {
        proc = ctx->r[12];
        slot = proc ? boot3_slot(proc) : -1;
        if (slot < 0) {
            /* not ours: as the game would (it never calls this with 0) */
            CALL(RECOMP_ORIGINAL(0x0018D7FC));
            return;
        }
    } else {
        int stale = boot3_slot(proc);
        if (stale >= 0)
            boot3_waiting[stale].proc = 0; /* a note left from an earlier run of the same proc */
    }
    uint32_t guard = *(volatile uint32_t *)&boot3_procthread_guard;
    if (mem_read8(ctx, proc + PROCTHREAD_RUNNING) == 0
        || (guard == 2 && (slot < 0 || boot3_waiting[slot].sleeps < 3))) {
        if (slot < 0) {
            slot = boot3_slot(0);
            if (slot >= 0) {
                boot3_waiting[slot].proc = proc;
                boot3_waiting[slot].lr = ctx->r[14];
                boot3_waiting[slot].sleeps = 0;
                boot3_procthread_held++;
            }
        }
        if (slot >= 0 && boot3_waiting[slot].sleeps < BOOT3_MAX_SLEEPS) {
            boot3_waiting[slot].sleeps++;
            boot3_procthread_sleeps++;
            ctx->r[12] = proc;
            ctx->r[0] = BOOT3_SLEEP_NS;
            ctx->r[1] = 0;
            ctx->r[14] = THREAD_FUNCTION;
            ctx->r[15] = SVC_SLEEP_THREAD;
            ctx->thumb = 0;
            ctx->budget -= 4;
            CALL(recomp_call);
            return;
        }
        if (slot >= 0)
            boot3_procthread_gave_up++;
    }
    /* marked running (or waited long enough): the game's ThreadFunction,
       with the proc and the return address it was called with */
    if (slot >= 0) {
        ctx->r[14] = boot3_waiting[slot].lr;
        boot3_waiting[slot].proc = 0;
    }
    ctx->r[0] = proc;
    CALL(RECOMP_ORIGINAL(0x0018D7FC));
}

/* pulsefix: what of this file is the console's state, kept with Divine
   Pulse's moments and saved states (overrides/keep.c) */
void boot3_keep(void (*add)(const char *, void *, uint32_t)) {
    add("boot3.boot3_waiting", &boot3_waiting, sizeof boot3_waiting);
}
