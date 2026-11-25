//! Where every thread is: `VICTORY_STACKS=<seconds>[,<seconds>...]` prints
//! each thread's unwound stack as `module+rva` that many seconds after
//! [`dump_stacks_on_schedule`], to find what a title waits on.

use std::ffi::c_void;

use windows_sys::Wdk::System::Threading::{
    NtQueryInformationThread, ThreadBasicInformation, ThreadQuerySetWin32StartAddress,
};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, LocalFree};
use windows_sys::Win32::System::Diagnostics::Debug::{
    CONTEXT, CONTEXT_FULL_AMD64, GetThreadContext,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThreadId, GetThreadDescription, OpenThread, ResumeThread,
    SuspendThread, THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION, THREAD_SUSPEND_RESUME,
};

use crate::crash::{MAX_FRAMES, locate, unwind_into};

/// Reads `VICTORY_STACKS` and, if set, starts a thread that prints every
/// thread's stack at each listed time (seconds from now).
pub fn dump_stacks_on_schedule() {
    let Ok(list) = std::env::var("VICTORY_STACKS") else {
        return;
    };
    let mut times: Vec<f64> = list
        .split(',')
        .filter_map(|t| t.trim().parse().ok())
        .collect();
    times.sort_by(f64::total_cmp);
    let start = std::time::Instant::now();
    std::thread::spawn(move || {
        for t in times {
            let at = std::time::Duration::from_secs_f64(t.max(0.0));
            std::thread::sleep(at.saturating_sub(start.elapsed()));
            dump_stacks();
        }
    });
}

/// `THREAD_BASIC_INFORMATION` (ntddk).
#[repr(C)]
struct BasicInfo {
    exit_status: i32,
    teb: *const usize,
    client_id: [usize; 2],
    affinity: usize,
    priority: i32,
    base_priority: i32,
}

/// `GetThreadContext` wants a 16-byte aligned context.
#[repr(C, align(16))]
struct Aligned(CONTEXT);

/// Prints every other thread's stack to stderr. Each thread is suspended
/// only while its context is read and unwound into a buffer allocated
/// beforehand (it may hold the heap lock).
pub fn dump_stacks() {
    // SAFETY: plain queries and a snapshot of this process's threads.
    let (me, process, snapshot) = unsafe {
        (
            GetCurrentThreadId(),
            GetCurrentProcessId(),
            CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0),
        )
    };
    if snapshot == INVALID_HANDLE_VALUE {
        return;
    }
    let mut ids = Vec::new();
    // SAFETY: zeroed is a valid THREADENTRY32; its size is set before use.
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<THREADENTRY32>() as u32;
    // SAFETY: `snapshot` is open; `entry` is sized.
    let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == process && entry.th32ThreadID != me {
            ids.push(entry.th32ThreadID);
        }
        // SAFETY: as above.
        more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    // SAFETY: we opened it.
    unsafe { CloseHandle(snapshot) };
    eprintln!("VIctory: stacks of {} threads", ids.len());
    for id in ids {
        // SAFETY: opens a thread of our own process.
        let thread = unsafe {
            OpenThread(
                THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION,
                0,
                id,
            )
        };
        if thread.is_null() {
            continue;
        }
        let (frames, (priority, base)) = frames_of(thread);
        eprintln!(
            "  thread {id} \"{}\" start {} priority {priority} (base {base})",
            name_of(thread),
            locate(start_of(thread))
        );
        for (i, pc) in frames.into_iter().enumerate() {
            eprintln!("    #{i:<2} {}", locate(pc as usize));
        }
        // SAFETY: we opened it.
        unsafe { CloseHandle(thread) };
    }
}

/// `VICTORY_PROFILE=<file>`: whenever that file appears, holding a number
/// of seconds, it is removed, the threads that run most are sampled for
/// that long, and what they were doing is written to `<file>.out`: a line
/// per distinct stack, `count thread leaf;caller;...` as `module+rva`.
pub fn profile_when_asked() {
    let Some(file) = std::env::var_os("VICTORY_PROFILE") else {
        return;
    };
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            let _ = std::fs::remove_file(&file);
            // "stacks": every thread's, unwound, on the log.
            if text.trim() == "stacks" {
                dump_stacks();
                continue;
            }
            let seconds: f64 = text.trim().parse().unwrap_or(5.0);
            let found = sample_threads(std::time::Duration::from_secs_f64(seconds));
            let mut out = file.clone();
            out.push(".out");
            let _ = std::fs::write(out, found);
        }
    });
}

/// The ids of this process's other threads.
fn thread_ids() -> Vec<u32> {
    // SAFETY: plain queries and a snapshot of this process's threads.
    let (me, process, snapshot) = unsafe {
        (
            GetCurrentThreadId(),
            GetCurrentProcessId(),
            CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0),
        )
    };
    let mut ids = Vec::new();
    if snapshot == INVALID_HANDLE_VALUE {
        return ids;
    }
    // SAFETY: zeroed is a valid THREADENTRY32; its size is set before use.
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<THREADENTRY32>() as u32;
    // SAFETY: `snapshot` is open; `entry` is sized.
    let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == process && entry.th32ThreadID != me {
            ids.push(entry.th32ThreadID);
        }
        // SAFETY: as above.
        more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    // SAFETY: we opened it.
    unsafe { CloseHandle(snapshot) };
    ids
}

/// Where a thread is, and the return addresses into our program's code
/// found on the top of its stack: no unwinding, which a thread stopped in
/// any place does not always survive (some may be of calls long done).
fn places_of(thread: HANDLE, code: (u64, u64)) -> Vec<u64> {
    let mut frames = Vec::with_capacity(16);
    let mut info: BasicInfo = BasicInfo {
        exit_status: 0,
        teb: std::ptr::null(),
        client_id: [0; 2],
        affinity: 0,
        priority: 0,
        base_priority: 0,
    };
    // SAFETY: as in `frames_of`; the stack is read between the pointer and
    // its base while the thread stands still, and code inside our image.
    unsafe {
        let status = NtQueryInformationThread(
            thread,
            ThreadBasicInformation,
            (&raw mut info).cast(),
            size_of::<BasicInfo>() as u32,
            std::ptr::null_mut(),
        );
        if status != 0 || info.teb.is_null() {
            return frames;
        }
        let (high, low) = (info.teb.add(1).read() as u64, info.teb.add(2).read() as u64);
        let mut context = Aligned(std::mem::zeroed());
        context.0.ContextFlags = CONTEXT_FULL_AMD64;
        if SuspendThread(thread) == u32::MAX {
            return frames;
        }
        if GetThreadContext(thread, &mut context.0) != 0 {
            frames.push(context.0.Rip);
            let top = context.0.Rsp;
            if top >= low && top < high && top % 8 == 0 {
                for i in 0..((high - top) / 8).min(600) {
                    let v = ((top + i * 8) as *const u64).read_volatile();
                    if v < code.0 + 16 || v >= code.1 {
                        continue;
                    }
                    let at = |n: u64| ((v - n) as *const u8).read();
                    let called = at(5) == 0xe8
                        || (at(6) == 0xff && at(5) & 0x38 == 0x10)
                        || (at(3) == 0xff && at(2) & 0x38 == 0x10)
                        || (at(2) == 0xff && at(1) & 0x38 == 0x10)
                        || (at(7) == 0xff && at(6) & 0x38 == 0x10);
                    if called {
                        frames.push(v);
                        if frames.len() >= 10 {
                            break;
                        }
                    }
                }
            }
        }
        ResumeThread(thread);
    }
    frames
}

/// The code of our own program.
fn own_code() -> (u64, u64) {
    // SAFETY: the headers of our own loaded image.
    unsafe {
        let base = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) as u64;
        let pe = base + u64::from(((base + 0x3c) as *const u32).read());
        let code = ((pe + 0x2c) as *const u32).read();
        let size = ((pe + 0x1c) as *const u32).read();
        (base + u64::from(code), base + u64::from(code) + u64::from(size))
    }
}

fn sample_threads(span: std::time::Duration) -> String {
    use std::collections::HashMap;
    let code = own_code();
    let mut threads: Vec<(u32, usize, u64)> = thread_ids()
        .into_iter()
        .filter_map(|id| {
            // SAFETY: opens a thread of our own process.
            let h = unsafe { OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION, 0, id) };
            (!h.is_null()).then_some((id, h as usize, 0))
        })
        .collect();
    // (The processor time of another thread is not told by Wine.) How many
    // places each was seen at: one that waits is at the same one.
    let mut places: HashMap<u32, std::collections::HashSet<Vec<u64>>> = HashMap::new();
    for _ in 0..40 {
        for t in &threads {
            let mut frames = places_of(t.1 as HANDLE, code);
            frames.truncate(6);
            places.entry(t.0).or_default().insert(frames);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    for t in &mut threads {
        t.2 = places.get(&t.0).map_or(0, |p| p.len() as u64);
    }
    threads.sort_by_key(|t| std::cmp::Reverse(t.2));
    let mut text = String::new();
    for t in threads.iter().take(24) {
        text.push_str(&format!("# thread {} \"{}\" seen at {} places of 40\n", t.0, name_of(t.1 as HANDLE), t.2));
    }
    let mut seen: HashMap<(u32, Vec<u64>), u32> = HashMap::new();
    let began = std::time::Instant::now();
    while began.elapsed() < span {
        for t in threads.iter().take(16) {
            let frames = places_of(t.1 as HANDLE, code);
            *seen.entry((t.0, frames)).or_default() += 1;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    for t in &threads {
        // SAFETY: we opened it.
        unsafe { CloseHandle(t.1 as HANDLE) };
    }
    let mut names: HashMap<u64, String> = HashMap::new();
    for ((id, frames), n) in &seen {
        let line: Vec<&str> = {
            for f in frames {
                names.entry(*f).or_insert_with(|| locate(*f as usize));
            }
            frames.iter().map(|f| names[f].as_str()).collect()
        };
        text.push_str(&format!("{n} {id} {}\n", line.join(";")));
    }
    text
}

/// The thread's frames and its current and base scheduling priority (0-31).
fn frames_of(thread: HANDLE) -> (Vec<u64>, (i32, i32)) {
    let mut frames = Vec::with_capacity(MAX_FRAMES);
    let mut info: BasicInfo = BasicInfo {
        exit_status: 0,
        teb: std::ptr::null(),
        client_id: [0; 2],
        affinity: 0,
        priority: 0,
        base_priority: 0,
    };
    // SAFETY: writes `info`, whose layout and size are the system's.
    let status = unsafe {
        NtQueryInformationThread(
            thread,
            ThreadBasicInformation,
            (&raw mut info).cast(),
            size_of::<BasicInfo>() as u32,
            std::ptr::null_mut(),
        )
    };
    if status != 0 || info.teb.is_null() {
        return (frames, (0, 0));
    }
    // SAFETY: the TEB of a live thread of this process: its NT_TIB holds
    // the stack base at +8 and the stack limit at +0x10.
    let (high, low) = unsafe { (info.teb.add(1).read(), info.teb.add(2).read()) };
    // SAFETY: zeroed is a valid CONTEXT.
    let mut context = Aligned(unsafe { std::mem::zeroed() });
    context.0.ContextFlags = CONTEXT_FULL_AMD64;
    // SAFETY: suspends, reads and resumes a thread that is not this one;
    // nothing between suspend and resume allocates or takes a lock the
    // thread could hold besides the unwinder's own.
    unsafe {
        if SuspendThread(thread) == u32::MAX {
            return (frames, (info.priority, info.base_priority));
        }
        if GetThreadContext(thread, &mut context.0) != 0 {
            unwind_into(context.0, low, high, &mut frames);
        }
        ResumeThread(thread);
    }
    (frames, (info.priority, info.base_priority))
}

fn start_of(thread: HANDLE) -> usize {
    let mut start = 0usize;
    // SAFETY: writes one pointer-sized start address.
    unsafe {
        NtQueryInformationThread(
            thread,
            ThreadQuerySetWin32StartAddress,
            (&raw mut start).cast(),
            size_of::<usize>() as u32,
            std::ptr::null_mut(),
        )
    };
    start
}

fn name_of(thread: HANDLE) -> String {
    let mut name = std::ptr::null_mut();
    // SAFETY: the system allocates the string; we free it with LocalFree.
    unsafe {
        if GetThreadDescription(thread, &mut name) < 0 || name.is_null() {
            return String::new();
        }
        let len = (0..).take_while(|&i| *name.add(i) != 0).count();
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(name, len));
        LocalFree(name.cast::<c_void>());
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn another_threads_stack_unwinds_to_its_start() {
        let (tx, rx) = std::sync::mpsc::channel();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let parked = std::thread::spawn(move || {
            // SAFETY: plain query.
            tx.send(unsafe { GetCurrentThreadId() }).unwrap();
            stop_rx.recv().ok();
        });
        let id = rx.recv().unwrap();
        // SAFETY: a thread of this process.
        let thread = unsafe {
            OpenThread(
                THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION,
                0,
                id,
            )
        };
        assert!(!thread.is_null());
        let (frames, _) = frames_of(thread);
        // Waiting in the kernel, several frames down from the thread start.
        assert!(frames.len() >= 3, "{frames:?}");
        assert!(start_of(thread) != 0);
        // SAFETY: opened above.
        unsafe { CloseHandle(thread) };
        dump_stacks();
        stop_tx.send(()).unwrap();
        parked.join().unwrap();
    }
}
