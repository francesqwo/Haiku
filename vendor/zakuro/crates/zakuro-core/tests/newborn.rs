//! savemods (GitHub issue #10): a new thread no more urgent than the thread
//! that made it isn't picked by the starvation rule or Aging before its
//! maker lets the core go (pcos::sched::Newborn). The game's Backup
//! controller thread (63), started by main (48) before its stop flag is
//! cleared, ran first on a slow boot, saw a stale flag and ended, and the
//! save lists then spun forever.

use zakuro_core::kernel::thread::{Thread, ThreadId, ThreadStatus};
use zakuro_core::pcos::sched::{Aging, Newborn, AGE_TICKS, TURN_TICKS};

fn thread(id: ThreadId, priority: u32, status: ThreadStatus) -> Thread {
    let mut thread = Thread::new(id, format!("t{id}"), 0x100000, 0x10000000, 0, priority, 0x1FF82000, 0);
    thread.status = status;
    thread
}

/// the issue's start-up: main (48) makes the controller (63) while a busy
/// thread above them both (35) takes the core. However long that lasts,
/// Aging never gives the controller a turn before main has run again and
/// waited; main, which has aged too, gets its turn first. Once main waits,
/// the controller is let go and ages as any other thread.
#[test]
fn a_new_thread_waits_for_its_maker_to_let_the_core_go() {
    let mut threads = vec![thread(0, 48, ThreadStatus::Running), thread(1, 35, ThreadStatus::Ready)];
    threads.push(thread(2, 63, ThreadStatus::Ready));
    let mut newborn = Newborn::default();
    newborn.made(&threads, 2, Some(0));
    threads[0].status = ThreadStatus::Ready; // main, cut off right after svcCreateThread
    let mut aging = Aging::default();
    let step = TURN_TICKS / 4;
    let mut tick = 0;
    let mut main_turn = None;
    while tick < 3 * AGE_TICKS {
        let held = newborn.held(&threads);
        assert_eq!(held, vec![2], "held while main is runnable");
        // priority picks the busy thread every time
        let next = aging.pass_holding(&threads, tick, Some(1), &held);
        assert_ne!(next, Some(2), "the controller ran before main at tick {tick}");
        if next == Some(0) && main_turn.is_none() {
            main_turn = Some(tick);
        }
        tick += step;
    }
    assert!(main_turn.is_some_and(|at| at <= AGE_TICKS + step), "main aged and got its turn: {main_turn:?}");
    // main waits (the game's next VBlank wait): the controller is let go
    threads[0].status = ThreadStatus::WaitSync;
    assert!(newborn.held(&threads).is_empty());
    let mut controller_turn = None;
    let start = tick;
    while tick < start + 2 * AGE_TICKS + TURN_TICKS {
        let held = newborn.held(&threads);
        if aging.pass_holding(&threads, tick, Some(1), &held) == Some(2) && controller_turn.is_none() {
            controller_turn = Some(tick - start);
        }
        tick += step;
    }
    assert!(controller_turn.is_some_and(|after| after <= AGE_TICKS + TURN_TICKS + step), "the controller got a turn once let go: {controller_turn:?}");
}

/// a new thread more urgent than its maker runs at once by priority, as on
/// the console, so it is never held; a held one is let go when its maker
/// ends, when it ends itself, or when priority picks it.
#[test]
fn only_less_urgent_new_threads_are_held_and_only_until_let_go() {
    let mut threads = vec![thread(0, 48, ThreadStatus::Running), thread(1, 20, ThreadStatus::Ready), thread(2, 63, ThreadStatus::Ready), thread(3, 48, ThreadStatus::Ready)];
    let mut newborn = Newborn::default();
    newborn.made(&threads, 1, Some(0));
    assert!(newborn.held(&threads).is_empty(), "more urgent: not held");
    newborn.made(&threads, 2, Some(0));
    newborn.made(&threads, 3, Some(0));
    assert_eq!(newborn.held(&threads), vec![2, 3], "lower and the same priority: held");
    // a thread made with nothing running (the first) isn't held
    newborn.made(&threads, 2, None);
    assert_eq!(newborn.held(&threads), vec![3]);
    newborn.made(&threads, 2, Some(0));
    newborn.ran(3);
    assert_eq!(newborn.held(&threads), vec![2], "priority picked 3");
    threads[2].status = ThreadStatus::Dead;
    assert!(newborn.held(&threads).is_empty(), "it ended");
    threads[2].status = ThreadStatus::Ready;
    newborn.made(&threads, 2, Some(0));
    threads[0].status = ThreadStatus::Dead;
    assert!(newborn.held(&threads).is_empty(), "its maker ended");
    // a slot used again by another thread is not the one held
    threads[0].status = ThreadStatus::Running;
    newborn.made(&threads, 2, Some(0));
    threads[2].guest_id += 100;
    assert!(newborn.held(&threads).is_empty(), "another thread in its slot");
}

/// without held threads, pass_holding is Aging as before.
#[test]
fn nothing_held_is_aging_as_before() {
    let threads = vec![thread(0, 48, ThreadStatus::Running), thread(1, 50, ThreadStatus::Ready), thread(2, 63, ThreadStatus::Ready)];
    let (mut a, mut b) = (Aging::default(), Aging::default());
    let mut tick = 0;
    while tick < 10 * AGE_TICKS {
        assert_eq!(a.pass(&threads, tick, Some(0)), b.pass_holding(&threads, tick, Some(0), &[]));
        tick += TURN_TICKS / 3;
    }
    assert!(a.turns > 0 && a.turns == b.turns);
}
