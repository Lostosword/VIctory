//! A background run (`VICTORY_BACKGROUND=1`, `victory_gpu::background`) is
//! silent: every audio session of our process, on every output device, is
//! muted before the title starts and whenever a new one appears. A stream
//! takes its session's mute when it is made, so nothing is heard first as
//! long as the title plays in the default session (Grand Theft Auto VI and
//! XAudio2 do); a stream in a session of its own is muted within
//! [`EVERY`]. Only our process's sessions are touched, never a device.
//!
//! shortcut: on Windows the mixer remembers the mute for this executable;
//! it is unmuted there (Volume mixer) by hand. Wine forgets it.

use std::time::Duration;

use windows::Win32::Media::Audio::{
    DEVICE_STATE_ACTIVE, IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume,
    MMDeviceEnumerator, eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows::core::Interface;

/// How often new sessions are looked for.
const EVERY: Duration = Duration::from_millis(100);

/// Mutes our sessions on every active output; returns how many it muted.
fn mute_all(devices: &IMMDeviceEnumerator) -> windows::core::Result<u32> {
    let mut muted = 0;
    // SAFETY: COM calls on live interfaces, with parameters built here.
    unsafe {
        let outputs = devices.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        for i in 0..outputs.GetCount()? {
            let manager: IAudioSessionManager2 = outputs.Item(i)?.Activate(CLSCTX_ALL, None)?;
            // The default session, made here if no stream has yet.
            let default = manager.GetSimpleAudioVolume(None, 0)?;
            if !default.GetMute()?.as_bool() {
                default.SetMute(true, std::ptr::null())?;
                muted += 1;
            }
            // Sessions streams made under identifiers of their own.
            let Ok(sessions) = manager.GetSessionEnumerator() else {
                continue;
            };
            let count = sessions.GetCount().unwrap_or(0);
            let mut unreachable = 0;
            for k in 0..count {
                // Wine's session controls do not give out their volume.
                let volume = sessions
                    .GetSession(k)
                    .and_then(|s| s.cast::<ISimpleAudioVolume>());
                match volume {
                    Ok(volume) => {
                        if !volume.GetMute()?.as_bool() {
                            volume.SetMute(true, std::ptr::null())?;
                            muted += 1;
                        }
                    }
                    Err(_) => unreachable += 1,
                }
            }
            // More than the default session, and no way to mute the rest:
            // say so, once per count.
            static SEEN: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(1);
            if unreachable > 1
                && SEEN.fetch_max(unreachable, std::sync::atomic::Ordering::Relaxed) < unreachable
            {
                tracing::warn!(
                    sessions = unreachable,
                    "background run: audio sessions besides the default one cannot be muted here"
                );
            }
        }
    }
    Ok(muted)
}

/// Our own `IAudioRenderClient::ReleaseBuffer` in the place of the host's:
/// the frames go out as silence ([`silence`]).
static RELEASE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

extern "system" fn release_silent(this: *mut std::ffi::c_void, frames: u32, flags: u32) -> i32 {
    const SILENT: u32 = 2;
    let host = RELEASE.load(std::sync::atomic::Ordering::Relaxed);
    // SAFETY: the host's own method, kept when its slot was replaced.
    unsafe {
        std::mem::transmute::<usize, extern "system" fn(*mut std::ffi::c_void, u32, u32) -> i32>(host)(
            this,
            frames,
            flags | SILENT,
        )
    }
}

/// `VICTORY_MUTE=1`: every stream of the process plays silence and goes on
/// as before (Wine's sessions ignore the mute, and a title without a working
/// output gives up). All of the host's render clients share one method
/// table; one is made here to find it.
fn silence() -> windows::core::Result<()> {
    use windows::Win32::Media::Audio::{
        AUDCLNT_SHAREMODE_SHARED, IAudioClient, IAudioRenderClient, eConsole,
    };
    // SAFETY: COM calls on live interfaces; the table's one slot is made
    // writable, replaced by a method of the same signature, and put back.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let client: IAudioClient = devices
            .GetDefaultAudioEndpoint(eRender, eConsole)?
            .Activate(CLSCTX_ALL, None)?;
        let format = client.GetMixFormat()?;
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 1_000_000, 0, format, None)?;
        let render: IAudioRenderClient = client.GetService()?;
        let table = *render.as_raw().cast::<*mut usize>();
        let slot = table.add(4);
        let mut old = 0;
        let protect = |to: u32, old: &mut u32| {
            windows_sys::Win32::System::Memory::VirtualProtect(slot.cast(), 8, to, old)
        };
        if protect(4, &mut old) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        RELEASE.store(*slot, std::sync::atomic::Ordering::Relaxed);
        slot.write(release_silent as *const () as usize);
        let mut back = 0;
        protect(old, &mut back);
        // The client stays: its table must.
        std::mem::forget((client, render));
    }
    tracing::info!("sound: every stream plays silence (VICTORY_MUTE)");
    Ok(())
}

/// In a background run: mutes our audio sessions, returning once the
/// first pass is done (call before the title starts), and keeps muting
/// new ones.
pub fn start() {
    if std::env::var_os("VICTORY_MUTE").is_some()
        && let Err(e) = silence()
    {
        tracing::warn!(%e, "sound not silenced");
    }
    if !victory_gpu::background() {
        return;
    }
    let (first, done) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("victory-mute".into())
        .spawn(move || {
            // SAFETY: this thread's own apartment, kept for its lifetime.
            let devices: windows::core::Result<IMMDeviceEnumerator> = unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
            };
            let mut first = Some(first);
            let mut failed = false;
            loop {
                let result = devices.as_ref().map_err(Clone::clone).and_then(mute_all);
                match &result {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(sessions = n, "background run: sound muted"),
                    Err(e) if !failed => {
                        failed = true;
                        tracing::warn!(%e, "background run: muting sound failed (no output device?)");
                    }
                    Err(_) => {}
                }
                if let Some(first) = first.take() {
                    let _ = first.send(());
                }
                std::thread::sleep(EVERY);
            }
        });
    if spawned.is_ok() {
        let _ = done.recv_timeout(Duration::from_secs(5));
    }
}
