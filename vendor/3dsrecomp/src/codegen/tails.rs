//! the registry of overrides' tails waiting for their original to return,
//! which entries.c carries beside the tables. recomp.h describes CALL_THEN,
//! and docs/overrides.md when to use it.

/// the tails waiting, up to how many, and what each kept.
pub const REGISTRY: &str = r#"
/* ---- tails waiting for their return (recomp.h, CALL_THEN) ---- */
#include <stdio.h>
#include <stdlib.h>

#define RECOMP_TAILS 64

typedef struct {
    RecompTail tail;
    const char *name;
    uint32_t tls, ret, sp;
    uint32_t words[RECOMP_TAIL_WORDS];
} RecompWaiting;

static RecompWaiting recomp_waiting[RECOMP_TAILS];
uint32_t recomp_tails_pending;
RECOMP_EXPORT uint8_t recomp_tails_counting;

/* the counts by tail, for recomp_tails_counting */
typedef struct {
    RecompTail tail;
    const char *name;
    uint64_t n[TAIL_KINDS];
} RecompTailCount;

static RecompTailCount recomp_tail_counts[128];
static uint32_t recomp_tail_counted, recomp_tails_most;

static void recomp_tails_print(void) {
    static const char *const kinds[TAIL_KINDS] = {"inline", "deferred", "resumed", "expired", "forgotten", "dropped", "replaced"};
    fprintf(stderr, "[tails] %u waiting at exit, at most %u at once\n", recomp_tails_pending, recomp_tails_most);
    for (uint32_t i = 0; i < recomp_tail_counted; i++) {
        const RecompTailCount *c = &recomp_tail_counts[i];
        fprintf(stderr, "[tails] %-40s", c->name);
        for (int k = 0; k < TAIL_KINDS; k++) fprintf(stderr, " %s %llu", kinds[k], (unsigned long long)c->n[k]);
        fprintf(stderr, "\n");
    }
}

void recomp_tail_count(RecompTail tail, const char *name, int how) {
    uint32_t i = 0;
    while (i < recomp_tail_counted && recomp_tail_counts[i].tail != tail) i++;
    if (i == recomp_tail_counted) {
        if (i == sizeof recomp_tail_counts / sizeof *recomp_tail_counts) return;
        if (i == 0) atexit(recomp_tails_print);
        recomp_tail_counts[i].tail = tail;
        recomp_tail_counts[i].name = name;
        recomp_tail_counted++;
    }
    recomp_tail_counts[i].n[how]++;
}

/* the frame a tail waits in, entered with sp at sp, is gone once the
   thread's stack is back above it without having come back through ret */
void recomp_tail_defer(Context *ctx, uint32_t ret, uint32_t sp, RecompTail tail, const char *name, const uint32_t *words) {
    RecompWaiting *free_slot = 0;
    for (uint32_t i = 0; i < RECOMP_TAILS; i++) {
        RecompWaiting *w = &recomp_waiting[i];
        if (!w->tail) {
            if (!free_slot) free_slot = w;
            continue;
        }
        /* the same frame again (the host entered the override once more
           after calls nested too deep): the later one stands */
        if (w->tls == ctx->tls && w->ret == ret && w->sp == sp) {
            if (recomp_tails_counting) recomp_tail_count(w->tail, w->name, TAIL_REPLACED);
            free_slot = w;
            w->tail = 0;
            recomp_tails_pending--;
            break;
        }
    }
    if (!free_slot) {
        if (recomp_tails_counting) recomp_tail_count(tail, name, TAIL_DROPPED);
        fprintf(stderr, "[tails] no room for %s's tail, it is lost\n", name);
        return;
    }
    free_slot->tail = tail;
    free_slot->name = name;
    free_slot->tls = ctx->tls;
    free_slot->ret = ret;
    free_slot->sp = sp;
    memcpy(free_slot->words, words, sizeof free_slot->words);
    recomp_tails_pending++;
    if (recomp_tails_pending > recomp_tails_most) recomp_tails_most = recomp_tails_pending;
    if (recomp_tails_counting) recomp_tail_count(tail, name, TAIL_DEFERRED);
}

void recomp_tails_due(Context *ctx) {
    uint32_t at = ctx->r[15] | ctx->thumb, sp = ctx->r[13], tls = ctx->tls;
    for (uint32_t i = 0; i < RECOMP_TAILS && recomp_tails_pending; i++) {
        RecompWaiting *w = &recomp_waiting[i];
        if (!w->tail || w->tls != tls) continue;
        if (w->ret == at && w->sp == sp) {
            RecompTail tail = w->tail;
            uint32_t words[RECOMP_TAIL_WORDS];
            memcpy(words, w->words, sizeof words);
            w->tail = 0;
            recomp_tails_pending--;
            if (recomp_tails_counting) recomp_tail_count(tail, w->name, TAIL_RESUMED);
            tail(ctx, words);
        } else if (sp > w->sp) {
            if (recomp_tails_counting) recomp_tail_count(w->tail, w->name, TAIL_EXPIRED);
            static unsigned told;
            if (told++ < 20) fprintf(stderr, "[tails] %s's frame (sp %08X) went away without returning to %08X\n", w->name, w->sp, w->ret);
            w->tail = 0;
            recomp_tails_pending--;
        }
    }
}

/* for the host: the tails of the thread whose tls is given, or of every
   thread for 0, are dropped, when the thread ends or a saved state replaces
   every thread */
RECOMP_EXPORT void recomp_tails_forget(uint32_t tls) {
    for (uint32_t i = 0; i < RECOMP_TAILS; i++) {
        RecompWaiting *w = &recomp_waiting[i];
        if (!w->tail || (tls && w->tls != tls)) continue;
        if (recomp_tails_counting) recomp_tail_count(w->tail, w->name, TAIL_FORGOTTEN);
        w->tail = 0;
        recomp_tails_pending--;
    }
}

/* pulsefix: the tails waiting are part of the console's state, as the
   threads' registers they wait on are: a host that keeps the console to go
   back to (a rewind's moment, a saved state) keeps them with it, and puts
   them back in place of those waiting now. A record is the waiting slot as
   it is; tail and name are this program's addresses (a host writing them
   into a file gives them relative to something of its own). keep returns
   how many waited, at most max of them written. */
typedef struct {
    uint64_t tail, name;
    uint32_t tls, ret, sp;
    uint32_t words[RECOMP_TAIL_WORDS];
} RecompKeptTail;

RECOMP_EXPORT uint32_t recomp_tails_keep(RecompKeptTail *out, uint32_t max) {
    uint32_t n = 0;
    for (uint32_t i = 0; i < RECOMP_TAILS; i++) {
        const RecompWaiting *w = &recomp_waiting[i];
        if (!w->tail) continue;
        if (n < max) {
            RecompKeptTail *k = &out[n];
            k->tail = (uint64_t)(uintptr_t)w->tail;
            k->name = (uint64_t)(uintptr_t)w->name;
            k->tls = w->tls;
            k->ret = w->ret;
            k->sp = w->sp;
            memcpy(k->words, w->words, sizeof k->words);
        }
        n++;
    }
    return n;
}

/* every tail waiting now is dropped (as recomp_tails_forget(0) does), and
   the kept ones wait again */
RECOMP_EXPORT void recomp_tails_put(const RecompKeptTail *in, uint32_t count) {
    recomp_tails_forget(0);
    for (uint32_t i = 0; i < count && i < RECOMP_TAILS; i++) {
        RecompWaiting *w = &recomp_waiting[i];
        if (!in[i].tail) continue;
        w->tail = (RecompTail)(uintptr_t)in[i].tail;
        w->name = (const char *)(uintptr_t)in[i].name;
        w->tls = in[i].tls;
        w->ret = in[i].ret;
        w->sp = in[i].sp;
        memcpy(w->words, in[i].words, sizeof w->words);
        recomp_tails_pending++;
        if (recomp_tails_pending > recomp_tails_most) recomp_tails_most = recomp_tails_pending;
    }
}
"#;
