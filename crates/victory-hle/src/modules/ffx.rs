//! The FidelityFX API the title ships (`ffxDispatch` and its four siblings,
//! AMD's public `ffx_api.h`): the upscale dispatches are run by an upscaler
//! of ours (`victory_gpu::upscale`), everything else by the title's own DLL.
//! `VICTORY_FFX=title` leaves the dispatches to it too.
#![allow(non_snake_case)]
// Title code passes pointers the real library would trust.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::{CStr, c_void};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::{HleModule, hle_export, runtime};

const DLL: &str = "amd_fidelityfx_loader_xbox.dll";
const CREATE_UPSCALE: u64 = 0x0001_0000;
const DISPATCH_UPSCALE: u64 = 0x0001_0001;
const DEPTH_INVERTED: u32 = 1 << 3;
/// `FFX_API_RETURN_OK` and `FFX_API_RETURN_ERROR`.
const OK: u32 = 0;
const ERROR: u32 = 1;

/// The flags the upscale context was made with (depth inverted until told).
static FLAGS: AtomicU32 = AtomicU32::new(DEPTH_INVERTED);

fn title(name: &CStr) -> Option<usize> {
    let base = runtime::image_named(DLL).or_else(|| runtime::load_title_dll(DLL).ok().flatten())?;
    super::kernel::own_export(base, name)
}

fn words(at: *const u8, count: usize) -> Vec<u32> {
    let mut bytes = vec![0u8; count * 4];
    if victory_gpu::upscale::peek(at as usize, &mut bytes) {
        bytes.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect()
    } else {
        Vec::new()
    }
}

fn wide(words: &[u32], at: usize) -> u64 {
    match words.get(at..at + 2) {
        Some(w) => u64::from(w[0]) | u64::from(w[1]) << 32,
        None => 0,
    }
}

/// `ffxCreateContext(context, description, memory callbacks)`.
pub extern "system" fn ffxCreateContext(
    context: *mut *mut c_void,
    desc: *mut u8,
    callbacks: *const c_void,
) -> u32 {
    let w = words(desc, 16);
    if wide(&w, 0) == CREATE_UPSCALE {
        FLAGS.store(w[4], Ordering::Relaxed);
    }
    let result = title(c"ffxCreateContext").map_or(ERROR, |f| {
        // SAFETY: the title's own export, called as it calls it.
        unsafe {
            std::mem::transmute::<usize, extern "system" fn(*mut *mut c_void, *mut u8, *const c_void) -> u32>(f)(
                context, desc, callbacks,
            )
        }
    });
    tracing::info!(words = format_args!("{w:x?}"), result, "ffxCreateContext");
    result
}

/// `ffxDispatch(context, description)`.
pub extern "system" fn ffxDispatch(context: *mut *mut c_void, desc: *const u8) -> u32 {
    static CALLS: AtomicU32 = AtomicU32::new(0);
    static OWN: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let own = *OWN.get_or_init(|| std::env::var("VICTORY_FFX").as_deref() != Ok("title"));
    let n = CALLS.fetch_add(1, Ordering::Relaxed) + 1;
    let w = words(desc, 128);
    let kind = wide(&w, 0);
    if n <= 3 || n.is_power_of_two() {
        tracing::info!(n, kind = format_args!("{kind:#x}"), words = format_args!("{w:x?}"), "ffxDispatch");
    }
    if own && kind == DISPATCH_UPSCALE && upscale(&w, n) {
        return OK;
    }
    title(c"ffxDispatch").map_or(ERROR, |f| {
        // SAFETY: the title's own export, called as it calls it.
        unsafe {
            std::mem::transmute::<usize, extern "system" fn(*mut *mut c_void, *const u8) -> u32>(f)(context, desc)
        }
    })
}

/// The public layout of `ffxDispatchDescUpscale`: header (16 bytes), list,
/// seven resources of 48 bytes (pointer, 32 bytes of description, state),
/// then the numbers.
fn upscale(w: &[u32], n: u32) -> bool {
    const FIRST: usize = 6;
    const RESOURCE: usize = 12;
    const NUMBERS: usize = FIRST + 7 * RESOURCE;
    if w.len() < NUMBERS + 14 {
        return false;
    }
    let resource = |i: usize| wide(w, FIRST + i * RESOURCE) as usize;
    let float = |i: usize| f32::from_bits(w[NUMBERS + i]);
    let dispatch = victory_gpu::upscale::Dispatch {
        list: wide(w, 4) as usize,
        colour: resource(0),
        depth: resource(1),
        motion: resource(2),
        output: resource(6),
        jitter: [float(0), float(1)],
        motion_scale: [float(2), float(3)],
        render: [w[NUMBERS + 4], w[NUMBERS + 5]],
        size: [w[NUMBERS + 6], w[NUMBERS + 7]],
        // Sharpening (a byte) and its strength, then the frame time.
        pre_exposure: float(11),
        reset: w[NUMBERS + 12] & 0xff != 0,
        far_is_one: FLAGS.load(Ordering::Relaxed) & DEPTH_INVERTED == 0,
    };
    let known = |at: usize| victory_gpu::upscale::is_resource(at);
    let sane = victory_gpu::upscale::is_list(dispatch.list)
        && known(dispatch.colour)
        && known(dispatch.depth)
        && known(dispatch.output)
        && (dispatch.motion == 0 || known(dispatch.motion))
        && dispatch.size.iter().chain(&dispatch.render).all(|v| (1..=8192).contains(v));
    let done = sane && victory_gpu::upscale::record(&dispatch);
    if n <= 3 || n.is_power_of_two() || (!done && n < 16) {
        tracing::info!(n, sane, done, ?dispatch, "an upscale of the title's");
    }
    done
}

pub(crate) fn module() -> HleModule {
    HleModule {
        name: "ffx",
        dlls: &[DLL],
        exports: vec![hle_export!(ffxCreateContext), hle_export!(ffxDispatch)],
    }
}
