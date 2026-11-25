//! Where the recording threads' time goes (diagnostics builds): named
//! stretches of code, timed with the processor's counter, each without the
//! stretches inside it. `span!("name")` at the start of a scope times the
//! rest of it; the sums are logged with the frame line.

use std::cell::Cell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

pub(crate) struct Site {
    name: &'static str,
    own: AtomicU64,
    calls: AtomicU64,
    known: AtomicBool,
}

static SITES: Mutex<Vec<&'static Site>> = Mutex::new(Vec::new());

thread_local! {
    static INSIDE: Cell<u64> = const { Cell::new(0) };
}

fn ticks() -> u64 {
    // SAFETY: reads the time-stamp counter.
    unsafe { core::arch::x86_64::_rdtsc() }
}

impl Site {
    pub(crate) const fn new(name: &'static str) -> Site {
        Site { name, own: AtomicU64::new(0), calls: AtomicU64::new(0), known: AtomicBool::new(false) }
    }
}

pub(crate) struct Span {
    site: &'static Site,
    began: u64,
    outside: u64,
}

impl Span {
    pub(crate) fn new(site: &'static Site) -> Span {
        if !site.known.load(Relaxed) && !site.known.swap(true, Relaxed) {
            SITES.lock().unwrap_or_else(|e| e.into_inner()).push(site);
        }
        let outside = INSIDE.replace(0);
        Span { site, began: ticks(), outside }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        let took = ticks().wrapping_sub(self.began);
        let inside = INSIDE.replace(self.outside.wrapping_add(took));
        self.site.own.fetch_add(took.saturating_sub(inside), Relaxed);
        self.site.calls.fetch_add(1, Relaxed);
    }
}

/// The sums since the last call, per frame of `frames`.
pub(crate) fn log(frames: u64) {
    static LAST: Mutex<Option<(std::time::Instant, u64)>> = Mutex::new(None);
    let now = (std::time::Instant::now(), ticks());
    let Some(last) = LAST.lock().unwrap_or_else(|e| e.into_inner()).replace(now) else {
        return;
    };
    let rate = now.1.wrapping_sub(last.1) as f64 / (now.0 - last.0).as_secs_f64() / 1000.0;
    let frames = frames.max(1) as f64;
    let mut sums: Vec<(f64, f64, &str)> = SITES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|s| (s.own.swap(0, Relaxed) as f64 / rate / frames, s.calls.swap(0, Relaxed) as f64 / frames, s.name))
        .collect();
    sums.sort_by(|a, b| b.0.total_cmp(&a.0));
    let text: Vec<String> = sums.iter().map(|(ms, n, name)| format!("{name} {ms:.1} ms x{n:.0}")).collect();
    tracing::info!("spans per frame, all threads: {}", text.join("; "));
}

macro_rules! span {
    ($name:literal) => {
        let _span = {
            static SITE: crate::span::Site = crate::span::Site::new($name);
            crate::span::Span::new(&SITE)
        };
    };
}
pub(crate) use span;
