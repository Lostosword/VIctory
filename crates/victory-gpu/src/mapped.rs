//! Which title memory holds physical pages, as the runtime says.
//!
//! A title reserves address space for 64 KiB pages and maps physical pages
//! into it and out of it as it streams (`victory-hle` xmem: a placeholder
//! reservation on the host, a view of a section per run of pages). Buffers
//! in such a reservation are mapped in part, and a submission's copies of
//! them must leave the holes out. Asking the host for the state of every
//! region costs a query per view: under Wine the query waits for the lock
//! the title's mapping calls hold while it streams (11-270 us), and a busy
//! world frame asked 4,000-38,000 times, 0.3-4 s (measured on a Mac).
//!
//! The runtime makes every one of those mappings itself, so it tells us:
//! the reservations that take pages, and each view mapped or unmapped.
//! Inside a reservation it told us of, what no view covers is unmapped and
//! what a view covers is mapped; of memory outside them we know nothing
//! and the copy asks the host as before. A copy that goes by an answer a
//! mapping call has just overtaken is the copy made a moment earlier: a
//! view gone meanwhile fails its read, which then asks the host.
//!
//! The runtime says a view is going before it unmaps it, and that call
//! waits for whoever holds the views ([`hold`]): memory held as mapped
//! stays readable and writable for as long, with no call into the host.

use std::collections::BTreeMap;
use std::sync::RwLock;

#[derive(Default)]
struct Map {
    /// Reservations that take physical pages: start to end.
    reservations: BTreeMap<usize, usize>,
    /// The views in them: start to end. They do not overlap.
    views: BTreeMap<usize, usize>,
}

static MAP: RwLock<Map> = RwLock::new(Map {
    reservations: BTreeMap::new(),
    views: BTreeMap::new(),
});

fn map() -> std::sync::RwLockWriteGuard<'static, Map> {
    MAP.write().unwrap_or_else(|e| e.into_inner())
}

/// Removes from `ranges` what lies in `[at, end)`, keeping the parts of
/// ranges that reach out of it.
fn cut(ranges: &mut BTreeMap<usize, usize>, at: usize, end: usize) {
    let hit: Vec<(usize, usize)> = ranges
        .range(..end)
        .rev()
        .take_while(|&(_, &stop)| stop > at)
        .map(|(&start, &stop)| (start, stop))
        .collect();
    for (start, stop) in hit {
        ranges.remove(&start);
        if start < at {
            ranges.insert(start, at);
        }
        if stop > end {
            ranges.insert(end, stop);
        }
    }
}

/// The runtime reserved `[at, at + len)` for physical pages (nothing is
/// mapped there yet).
pub fn title_pages_reserved(at: usize, len: usize) {
    if len == 0 {
        return;
    }
    let mut m = map();
    cut(&mut m.reservations, at, at + len);
    cut(&mut m.views, at, at + len);
    m.reservations.insert(at, at + len);
}

/// The title released the reservation at `at` (or one holding it): what it
/// held is no longer known.
pub fn title_pages_released(at: usize) {
    let mut m = map();
    let Some((&start, &end)) = m.reservations.range(..=at).next_back().filter(|&(_, &e)| e > at)
    else {
        return;
    };
    m.reservations.remove(&start);
    cut(&mut m.views, start, end);
}

/// The runtime mapped physical pages at `[at, at + len)`.
pub fn title_pages_mapped(at: usize, len: usize) {
    if len == 0 {
        return;
    }
    let mut m = map();
    cut(&mut m.views, at, at + len);
    m.views.insert(at, at + len);
}

/// The runtime is about to unmap `[at, at + len)`.
pub fn title_pages_unmapped(at: usize, len: usize) {
    if len == 0 {
        return;
    }
    cut(&mut map().views, at, at + len);
}

/// The ranges of `ranges` that reach into `[a, b)`, in order.
fn within(ranges: &BTreeMap<usize, usize>, a: usize, b: usize) -> impl Iterator<Item = (usize, usize)> + '_ {
    let before = ranges
        .range(..=a)
        .next_back()
        .filter(|&(_, &end)| end > a)
        .map(|(&s, &e)| (s, e));
    before
        .into_iter()
        .chain(ranges.range(a + 1..b.max(a + 1)).map(|(&s, &e)| (s, e)))
}

/// Views that stay mapped while this lives: the runtime's unmapping waits
/// for it. Not to be kept over anything that waits for the title.
pub(crate) struct Held(#[allow(dead_code)] std::sync::RwLockReadGuard<'static, Map>);

/// What [`hold`] found.
pub(crate) enum Hold {
    /// All of it is views, read-write, until the hold is dropped.
    Mapped(Held),
    /// All of it is reserved for pages and has none.
    Unmapped,
    /// Something else, or we were told nothing of it: ask the host.
    Unknown,
}

/// `[from, to)` held if it is all views.
pub(crate) fn hold(from: usize, to: usize) -> Hold {
    if from >= to {
        return Hold::Unknown;
    }
    let m = MAP.read().unwrap_or_else(|e| e.into_inner());
    let mut at = from;
    let mut any = false;
    for (start, end) in within(&m.views, from, to) {
        any = true;
        if start > at {
            return Hold::Unknown;
        }
        at = at.max(end);
    }
    if at >= to {
        return Hold::Mapped(Held(m));
    }
    let reserved = m
        .reservations
        .range(..=from)
        .next_back()
        .is_some_and(|(_, &end)| end >= to);
    if !any && reserved { Hold::Unmapped } else { Hold::Unknown }
}

/// What is known of a piece of title memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// A view of physical pages: readable.
    Mapped,
    /// Reserved for pages, none there.
    Unmapped,
    /// Not in a reservation we were told of.
    Unknown,
}

/// `[from, to)` in pieces `(start, end, state)`, in order and each as long
/// as its state lasts (neighbouring views are one piece). Empty if nothing
/// of the range is known.
pub(crate) fn pieces(from: usize, to: usize) -> Vec<(usize, usize, State)> {
    let mut out: Vec<(usize, usize, State)> = Vec::new();
    if from >= to {
        return out;
    }
    let m = MAP.read().unwrap_or_else(|e| e.into_inner());
    if m.reservations.is_empty() && m.views.is_empty() {
        return out;
    }
    let mut push = |start: usize, end: usize, state: State| {
        if start >= end {
            return;
        }
        match out.last_mut() {
            Some(last) if last.2 == state && last.1 == start => last.1 = end,
            _ => out.push((start, end, state)),
        }
    };
    // Views are mapped wherever they are; between them, what a reservation
    // holds is unmapped and the rest unknown.
    let gaps = |a: usize, b: usize, push: &mut dyn FnMut(usize, usize, State)| {
        if a >= b {
            return;
        }
        let mut at = a;
        for (start, end) in within(&m.reservations, a, b) {
            let (start, end) = (start.max(a), end.min(b));
            push(at, start, State::Unknown);
            push(start, end, State::Unmapped);
            at = at.max(end);
        }
        push(at, b, State::Unknown);
    };
    let mut at = from;
    for (start, end) in within(&m.views, from, to) {
        let (start, end) = (start.max(from), end.min(to));
        gaps(at, start, &mut push);
        push(start, end, State::Mapped);
        at = at.max(end);
    }
    gaps(at, to, &mut push);
    drop(m);
    if out.iter().all(|p| p.2 == State::Unknown) {
        out.clear();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::State::{Mapped, Unknown, Unmapped};
    use super::*;

    const P: usize = 64 << 10;
    /// Tests share the process's map: each takes addresses of its own.
    fn base(n: usize) -> usize {
        0x70_0000_0000 + n * 0x1_0000_0000
    }

    #[test]
    fn a_reservation_is_unmapped_until_views_come() {
        let b = base(1);
        assert_eq!(pieces(b, b + 8 * P), []);
        title_pages_reserved(b, 8 * P);
        assert_eq!(pieces(b, b + 8 * P), [(b, b + 8 * P, Unmapped)]);
        // Around it nothing is known.
        assert_eq!(
            pieces(b - P, b + 9 * P),
            [(b - P, b, Unknown), (b, b + 8 * P, Unmapped), (b + 8 * P, b + 9 * P, Unknown)]
        );
        title_pages_mapped(b + P, P);
        title_pages_mapped(b + 2 * P, P);
        title_pages_mapped(b + 5 * P, 2 * P);
        assert_eq!(
            pieces(b, b + 8 * P),
            [
                (b, b + P, Unmapped),
                (b + P, b + 3 * P, Mapped),
                (b + 3 * P, b + 5 * P, Unmapped),
                (b + 5 * P, b + 7 * P, Mapped),
                (b + 7 * P, b + 8 * P, Unmapped),
            ]
        );
        // A range cut inside views and holes.
        assert_eq!(
            pieces(b + P + 100, b + 6 * P - 4),
            [
                (b + P + 100, b + 3 * P, Mapped),
                (b + 3 * P, b + 5 * P, Unmapped),
                (b + 5 * P, b + 6 * P - 4, Mapped),
            ]
        );
        assert_eq!(pieces(b + P, b + 3 * P), [(b + P, b + 3 * P, Mapped)]);
    }

    #[test]
    fn unmapping_cuts_views_and_releasing_forgets() {
        let b = base(2);
        title_pages_reserved(b, 16 * P);
        title_pages_mapped(b, 8 * P);
        // The middle of a view goes: its ends stay (the runtime maps them
        // again and says so; saying it twice changes nothing).
        title_pages_unmapped(b + 2 * P, 3 * P);
        title_pages_mapped(b, 2 * P);
        title_pages_mapped(b + 5 * P, 3 * P);
        assert_eq!(
            pieces(b, b + 16 * P),
            [
                (b, b + 2 * P, Mapped),
                (b + 2 * P, b + 5 * P, Unmapped),
                (b + 5 * P, b + 8 * P, Mapped),
                (b + 8 * P, b + 16 * P, Unmapped),
            ]
        );
        // A page mapped over part of a view replaces that part.
        title_pages_mapped(b + 6 * P, P);
        assert_eq!(pieces(b + 5 * P, b + 8 * P), [(b + 5 * P, b + 8 * P, Mapped)]);
        // Released (by any address in it): nothing known any more.
        title_pages_released(b + 3 * P);
        assert_eq!(pieces(b, b + 16 * P), []);
        title_pages_released(b);
    }

    #[test]
    fn a_view_outside_any_reservation_is_still_mapped() {
        let b = base(3);
        title_pages_mapped(b + P, P);
        assert_eq!(
            pieces(b, b + 3 * P),
            [(b, b + P, Unknown), (b + P, b + 2 * P, Mapped), (b + 2 * P, b + 3 * P, Unknown)]
        );
        title_pages_unmapped(b + P, P);
        assert_eq!(pieces(b, b + 3 * P), []);
    }

    #[test]
    fn views_are_held_and_unmapping_waits_for_the_hold() {
        let b = base(5);
        assert!(matches!(hold(b, b + P), Hold::Unknown));
        title_pages_reserved(b, 8 * P);
        assert!(matches!(hold(b, b + P), Hold::Unmapped));
        title_pages_mapped(b + P, P);
        title_pages_mapped(b + 2 * P, P);
        // Two views side by side, and parts of them.
        assert!(matches!(hold(b + P, b + 3 * P), Hold::Mapped(_)));
        assert!(matches!(hold(b + P + 5, b + 2 * P + 9), Hold::Mapped(_)));
        // Views and a hole, either way round: neither one nor the other.
        assert!(matches!(hold(b, b + 2 * P), Hold::Unknown));
        assert!(matches!(hold(b + 2 * P, b + 4 * P), Hold::Unknown));
        // Past the reservation's end nothing is known.
        assert!(matches!(hold(b + 7 * P, b + 9 * P), Hold::Unknown));
        assert!(matches!(hold(b + 7 * P, b + 8 * P), Hold::Unmapped));

        let held = hold(b + P, b + 2 * P);
        let gone = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let unmapping = {
            let gone = gone.clone();
            std::thread::spawn(move || {
                title_pages_unmapped(b + P, P);
                gone.store(true, std::sync::atomic::Ordering::SeqCst);
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(!gone.load(std::sync::atomic::Ordering::SeqCst), "unmapped under a hold");
        drop(held);
        unmapping.join().unwrap();
        assert!(matches!(hold(b + P, b + 2 * P), Hold::Unmapped));
        title_pages_released(b);
    }

    #[test]
    fn a_new_reservation_over_an_old_one_starts_empty() {
        let b = base(4);
        title_pages_reserved(b, 4 * P);
        title_pages_mapped(b, 4 * P);
        title_pages_reserved(b + 2 * P, 4 * P);
        assert_eq!(
            pieces(b, b + 6 * P),
            [(b, b + 2 * P, Mapped), (b + 2 * P, b + 6 * P, Unmapped)]
        );
    }
}
