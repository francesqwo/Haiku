//! os: native fast paths for nn::os synchronisation (step 2 of the plan in
//! dev/reports/os.md, as far as it is safe with threads on the guest
//! scheduler).
//!
//! nn::os::CriticalSection, LightEvent and LightSemaphore are user-mode
//! atomics that fall back to svcArbitrateAddress; nn::os::Event and the
//! SDK's threads use svcClearEvent and svcGetThreadPriority. The calls
//! answered here are the ones that cannot block and wake nobody, so no
//! thread changes state and the guest scheduler has nothing to decide:
//!
//! - ArbitrateAddress Signal on an address no thread is parked on: nothing
//!   to wake. (The kernel path also asked for a reschedule, which picks the
//!   running thread again: a thread keeps the core unless something more
//!   urgent is ready, and nothing became ready.)
//! - ArbitrateAddress WaitIfLessThan / DecrementAndWaitIfLessThan (with or
//!   without timeout) when the word is not less than the value: the kernel
//!   returns at once without touching it.
//! - ClearEvent on an event: clears its flag.
//! - GetThreadPriority: reads it.
//!
//! Everything else (a wake-up, a wait that sleeps, timers, mutexes) goes to
//! the kernel as before. The answers are exactly the kernel's, so runs stay
//! deterministic.

use recomp_abi::Context;
use zakuro_cpu::Bus;

use super::bridge::Call;
use crate::kernel::object::KObject;
use crate::System;

/// the pseudo-handle of the current thread.
const CURRENT_THREAD: u32 = 0xFFFF_8000;

/// runs a native synchronisation call made by an override, false when it
/// is not one (none are defined yet: the SVC fast paths cover the SDK's
/// inlined calls).
pub fn call(_system: &mut System, _call: &mut Call, _number: u32) -> bool {
    false
}

/// answers an svc the code made, when it is one of the fast paths above.
/// the name to count it under, None to leave it to the kernel.
pub fn svc(system: &mut System, ctx: &mut Context, number: u32) -> Option<&'static str> {
    match number {
        // ArbitrateAddress(arbiter, address, type, value, timeout)
        0x22 => {
            let (handle, address, kind, value) = (ctx.r[0], ctx.r[1], ctx.r[2], ctx.r[3] as i32);
            let object = system.kernel.resolve(handle)?;
            let Some(KObject::AddressArbiter(arbiter)) = system.kernel.objects.get(object) else { return None };
            match kind {
                0 => {
                    if arbiter.waiters.iter().any(|&(_, parked)| parked == address) {
                        return None;
                    }
                    ctx.r[0] = 0;
                    Some("svc.ArbitrateAddress.signal-nobody")
                }
                1..=4 => {
                    if system.memory.read32(address) as i32 >= value {
                        ctx.r[0] = 0;
                        Some("svc.ArbitrateAddress.wait-not-needed")
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
        // ClearEvent(event)
        0x19 => {
            let object = system.kernel.resolve(ctx.r[0])?;
            match system.kernel.objects.get_mut(object) {
                Some(KObject::Event(event)) => {
                    event.signaled = false;
                    ctx.r[0] = 0;
                    Some("svc.ClearEvent")
                }
                _ => None,
            }
        }
        // GetThreadPriority(out, thread) -> r1
        0x0B => {
            let handle = ctx.r[1];
            let id = if handle == CURRENT_THREAD {
                system.kernel.current_thread?
            } else {
                match system.kernel.objects.get(system.kernel.handles.resolve(handle)?) {
                    Some(KObject::Thread(id)) => *id,
                    _ => return None,
                }
            };
            ctx.r[1] = system.kernel.thread(id).priority;
            ctx.r[0] = 0;
            Some("svc.GetThreadPriority")
        }
        _ => None,
    }
}
