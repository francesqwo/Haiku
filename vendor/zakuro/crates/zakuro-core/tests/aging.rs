//! savehang: real time's Aging rule (pcos::sched): a thread kept from the
//! core gets a turn. The lib's own unit tests do not build in the pure
//! configuration, so these are an integration test.

use zakuro_core::kernel::thread::{Thread, ThreadId, ThreadStatus};
use zakuro_core::pcos::sched::{Aging, AGE_TICKS, TURN_TICKS};

fn thread(id: ThreadId, priority: u32, status: ThreadStatus) -> Thread {
    let mut thread = Thread::new(id, format!("t{id}"), 0x100000, 0x10000000, 0, priority, 0x1FF82000, 0);
    thread.status = status;
    thread
}

/// savehang: the starvation the save list hung on. A main thread (48)
/// that works the first half of every refresh and waits for the next, a
/// busy thread (50) for the rest, and the Backup controller (63) with a
/// job: priority alone (and the starvation rule, which never looks at a
/// thread that waits each refresh) never picks the controller. With
/// Aging it gets a turn within AGE_TICKS, and every AGE_TICKS after.
#[test]
fn a_thread_kept_from_the_core_gets_a_turn() {
    let mut threads = vec![thread(0, 48, ThreadStatus::Running), thread(1, 50, ThreadStatus::Ready), thread(2, 63, ThreadStatus::Ready)];
    let mut aging = Aging::default();
    let step = TURN_TICKS / 4; // a scheduling pass every quarter of a turn
    let frames = 40;
    let mut ran = [0u64; 3];
    let mut first_turn = None;
    let mut tick = 0;
    while tick < frames * zakuro_core::CYCLES_PER_FRAME {
        let in_frame = tick % zakuro_core::CYCLES_PER_FRAME < zakuro_core::CYCLES_PER_FRAME / 2;
        threads[0].status = if in_frame { ThreadStatus::Ready } else { ThreadStatus::WaitSync };
        let picked = Some(if in_frame { 0 } else { 1 });
        let next = aging.pass(&threads, tick, picked).expect("a thread runs");
        ran[next as usize] += step;
        if next == 2 && first_turn.is_none() {
            first_turn = Some(tick);
        }
        tick += step;
    }
    let first = first_turn.expect("the lowest thread got a turn");
    assert!(first <= AGE_TICKS + step, "first turn at {first}");
    let turns = frames * zakuro_core::CYCLES_PER_FRAME / (AGE_TICKS + TURN_TICKS);
    assert!(ran[2] >= (turns - 1) * TURN_TICKS, "the lowest thread ran {} ticks", ran[2]);
    // the others keep nearly all the time
    assert!(ran[2] * 20 < ran[0] + ran[1], "{ran:?}");
}

/// savehang: threads that get the core in time are left as priority
/// picks them.
#[test]
fn aging_leaves_a_fair_schedule_alone() {
    let threads = vec![thread(0, 48, ThreadStatus::Running), thread(1, 63, ThreadStatus::Ready)];
    let mut aging = Aging::default();
    let mut tick = 0;
    while tick < 20 * zakuro_core::CYCLES_PER_FRAME {
        // the low thread runs every other refresh
        let picked = if (tick / zakuro_core::CYCLES_PER_FRAME) % 2 == 0 { 0 } else { 1 };
        assert_eq!(aging.pass(&threads, tick, Some(picked)), Some(picked));
        tick += TURN_TICKS;
    }
    assert_eq!(aging.turns, 0);
}
