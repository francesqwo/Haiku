# Overrides

An override is a function written by hand in C that runs instead of the
function 3dsrecomp would generate for an address. Overrides are for fixes,
mods, and faster native versions of hot functions. Titles that need none
still build without them.

## Writing one

Put the functions in one or more C files and mark each with the address it
replaces:

```c
#include "overrides.h"

/* the title's strlen, done natively */
RECOMP_OVERRIDE(0x0012A4F0) {
    uint32_t start = ctx->r[0], end = start;
    while (mem_read8(ctx, end))
        end++;
    ctx->r[0] = end - start;
    RETURN_TO(ctx->r[14]);
}
```

Then pass the file, or a directory of them, to `build` or `port`:

```
3dsrecomp build <rom> <dir> --overrides overrides/
```

The files are compiled with the generated code, so everything in
[`recomp.h`](../abi/recomp.h) is available: the context, the memory
helpers such as `mem_read32` and `mem_write32`, and the `CALL` and
`RETURN_TO` macros. [The interface](interface.md) describes them.

An override is entered the way the guest enters the function it replaces:

- the arguments are in `r0` to `r3` and on the guest stack;
- `r14` holds the return address.

It returns the way a guest function does:

- the result goes in `r0`;
- it ends with `RETURN_TO(ctx->r[14])`, which also switches between ARM and
  Thumb when the caller needs it.

## Addresses

- **Thumb.** An odd address means a Thumb function, as in
  `RECOMP_OVERRIDE(0x0012A4F1)`.
- **Modules.** A function in a CRO module is named by the module and its
  offset from the start of the module's file:

  ```c
  RECOMP_OVERRIDE_IN(DllField, 0x1A40) {
      ...
  }
  ```

  The offset stays the same wherever the title loads the module.

## Calling the original

The generated function is still there. `RECOMP_ORIGINAL` names it, so an
override can run it before or after doing something of its own:

```c
#include <stdio.h>
#include "overrides.h"

RECOMP_OVERRIDE(0x00112120) {
    fprintf(stderr, "0x00112120 called with r0 = %08X\n", ctx->r[0]);
    CALL(RECOMP_ORIGINAL(0x00112120));
}
```

When the original returns, `r15` already holds the address it returned to,
so the override can simply return too. `build` warns about an override
whose address has no generated code. There is no original to call in that
case.

## Work after the original: CALL_THEN

`CALL` returns at once when the code leaves for the host on the way: an
svc the host stops the code after, the budget running out, a thread that
blocks, calls nested too deep. The host goes on from `r15` later, deep
inside the original, and by the time the original returns the override's C
frame is gone. Anything written after `CALL(RECOMP_ORIGINAL(...))` then
never runs: an end hook is lost, a restore is missed.

Put such work in a tail and call the original with `CALL_THEN`:

```c
/* words[0] is what the override saved for it */
static void effect_end(Context *ctx, const uint32_t *words) {
    my_end_hook(words[0], ctx->r[0]);
}

RECOMP_OVERRIDE(0x00112120) {
    my_begin_hook(ctx->r[0]);
    CALL_THEN(RECOMP_ORIGINAL(0x00112120), effect_end, ctx->r[0]);
}
```

- The tail runs once the original has returned: right after the call when
  it came straight back, and otherwise when the guest comes back to the
  override's return address (`r14` on entry) with the stack pointer it had
  on entry, in the same thread (`tls`).
- It runs with the context as the original left it: the result in `r0` to
  `r3`, which it may change, and `r15` at the return address.
- It gets up to `RECOMP_TAIL_WORDS` (6) words the override saved; give at
  least one.
- It must not make guest calls.
- A return into a frame that is gone always comes back into generated code
  midway, or through `recomp_call`; both run a tail due there first
  (`TAILS_CHECK`). The registry is in `entries.c`; a host needs nothing for
  it, but should call `recomp_tails_forget(tls)` when a thread ends and
  `recomp_tails_forget(0)` when it loads a saved state, so a tail is never
  run for a frame that is not its own.
- A tail whose thread's stack goes back above its frame without returning
  is dropped, with a message. Setting `recomp_tails_counting` to 1 counts
  how each tail ran (inline, deferred, resumed, expired, forgotten, dropped, replaced), printed
  at exit.

Work done before the original, and overrides that only call it last, need
none of this.

## Natives

`build` writes overrides of its own for the integer divisions of ARM's C
library, which every title built with armcc links and which the ARM11 does
with a loop, having no divide instruction. It finds them by their bytes,
RVCT 4.1's, and puts their overrides in `natives.c`:

- `__aeabi_uidivmod` and `__aeabi_idivmod`, 32-bit unsigned and signed;
- `__aeabi_uldivmod` and `__aeabi_ldivmod`, 64-bit.

Each divides on the host and leaves the quotient and the remainder where the
library does. A divisor of zero, and the one quotient that overflows, go to
the library's own code, which is still there. Each charges the budget what
that code would have: exactly for the 32-bit ones, and for the 64-bit ones
once the quotient has 6 bits or more.

An override written by hand for the same address takes the place of the
native. A host can turn the natives off by setting the library's
`recomp_natives` to 0, and Zakuro does that when `ZAKURO_NATIVES` is 0.
`verify` compares the generated code with the natives off, and then each
native against the title's own code in the interpreter, on numbers that go
every way through it.

## What changes

- **Calls.** Every call the recompiler can follow to the address goes to the
  override instead.
- **Entries.** The address's entry in the table points at the override, so
  the host also runs the override for indirect calls and for anything it
  looks up there.
- **Resuming.** Code that resumes partway through the replaced function
  still runs the generated code. For example, a return into it after one of
  its own calls. Only entering at the address goes to the override.
- **`verify`.** It compares the generated code against Zakuro's interpreter.
  An override that does something different from the original shows up
  there as a mismatch, by design.
