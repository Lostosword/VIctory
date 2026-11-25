//! A temporal upscaler of ours in the place of the one the title ships
//! (AMD FidelityFX, on Series S): its passes come as console programs
//! without the DXIL part our translator starts from, so none of them runs
//! and everything after them reads an image nobody wrote. The runtime hands
//! us the title's `ffxDispatch` calls (`victory-hle`, `ffx`); this records
//! one compute pass into the title's list with the resources the call names.
//!
//! The pass: the current frame's samples gathered around each output pixel
//! (jitter taken out), the previous output fetched along the motion of the
//! nearest surface, held to the spread of the current neighbourhood, and
//! the two blended by how well the pixel is covered.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::s;

use crate::host::{HostList, host, transition};
use crate::object::This;

const SOURCE: &str = r#"
#define RS "RootConstants(num32BitConstants = 24, b0), DescriptorTable(SRV(t0, numDescriptors = 4), UAV(u0, numDescriptors = 2)), StaticSampler(s0, filter = FILTER_MIN_MAG_LINEAR_MIP_POINT, addressU = TEXTURE_ADDRESS_CLAMP, addressV = TEXTURE_ADDRESS_CLAMP, addressW = TEXTURE_ADDRESS_CLAMP)"
cbuffer Constants : register(b0) {
    float2 render; float2 size;
    float2 jitter; float2 motion_scale;
    float reset; float exposure_ratio; float far_is_one; float has_motion;
    float2 history_size; float most; float pad0;
    float4 pad1; float4 pad2;
};
Texture2D<float4> colour : register(t0);
Texture2D<float> depth : register(t1);
Texture2D<float2> motion : register(t2);
Texture2D<float4> history : register(t3);
RWTexture2D<float4> result : register(u0);
RWTexture2D<float4> kept : register(u1);
SamplerState linear_clamp : register(s0);

float3 finite(float3 c) { return min(max(c, 0.0), 60000.0); }
float3 squash(float3 c) { return c / (1.0 + max(c.r, max(c.g, c.b))); }
float3 stretch(float3 c) { return c / max(1.0 - max(c.r, max(c.g, c.b)), 0.0001); }
float3 to_ycc(float3 c) { return float3(dot(c, float3(0.25, 0.5, 0.25)), dot(c, float3(0.5, 0.0, -0.5)), dot(c, float3(-0.25, 0.5, -0.25))); }
float3 to_rgb(float3 c) { return float3(c.x + c.y - c.z, c.x + c.z, c.x - c.y - c.z); }
float lanczos(float d2) {
    d2 = min(d2, 4.0);
    float a = 0.4 * d2 - 1.0;
    float b = 0.25 * d2 - 1.0;
    return (1.5625 * a * a - 0.5625) * (b * b);
}

float4 history_at(float2 uv) {
    // Catmull-Rom in five bilinear fetches.
    float2 at = uv * history_size;
    float2 centre = floor(at - 0.5) + 0.5;
    float2 f = at - centre;
    float2 w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    float2 w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    float2 w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    float2 w3 = f * f * (-0.5 + 0.5 * f);
    float2 w12 = w1 + w2;
    float2 t0 = (centre - 1.0) / history_size;
    float2 t3 = (centre + 2.0) / history_size;
    float2 t12 = (centre + w2 / w12) / history_size;
    float4 c = 0.0; float w = 0.0; float k;
    k = w12.x * w0.y;  c += history.SampleLevel(linear_clamp, float2(t12.x, t0.y), 0) * k; w += k;
    k = w0.x * w12.y;  c += history.SampleLevel(linear_clamp, float2(t0.x, t12.y), 0) * k; w += k;
    k = w12.x * w12.y; c += history.SampleLevel(linear_clamp, t12, 0) * k; w += k;
    k = w3.x * w12.y;  c += history.SampleLevel(linear_clamp, float2(t3.x, t12.y), 0) * k; w += k;
    k = w12.x * w3.y;  c += history.SampleLevel(linear_clamp, float2(t12.x, t3.y), 0) * k; w += k;
    return c / w;
}

[RootSignature(RS)]
[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (any(float2(id.xy) >= size)) return;
    float2 uv = (float2(id.xy) + 0.5) / size;
    float2 at = uv * render;
    int2 base = int2(floor(at));
    int2 last = int2(render) - 1;
    float3 sum = 0.0, low = 1e9, high = -1e9, m1 = 0.0, m2 = 0.0;
    float total = 0.0, covered = 0.0, near = -1.0;
    int2 nearest = clamp(base, 0, last);
    for (int y = -1; y <= 1; y++) {
        for (int x = -1; x <= 1; x++) {
            int2 p = clamp(base + int2(x, y), 0, last);
            float3 c = to_ycc(squash(finite(colour.Load(int3(p, 0)).rgb)));
            float2 off = (float2(base + int2(x, y)) + 0.5 - jitter) - at;
            float d2 = dot(off, off);
            float w = lanczos(d2);
            sum += c * w; total += w;
            covered = max(covered, exp(-2.0 * d2 * dot(size / render, float2(0.5, 0.5))));
            low = min(low, c); high = max(high, c);
            m1 += c; m2 += c * c;
            float z = depth.Load(int3(p, 0));
            z = far_is_one > 0.5 ? 1.0 - z : z;
            if (z > near) { near = z; nearest = p; }
        }
    }
    float3 now = clamp(sum / max(total, 0.001), low, high);
    m1 /= 9.0; m2 /= 9.0;
    float3 spread = sqrt(max(m2 - m1 * m1, 0.0)) * 1.5;
    float2 move = has_motion > 0.5 ? motion.Load(int3(nearest, 0)) * motion_scale / render : 0.0;
    float2 from = uv + move;
    bool usable = reset < 0.5 && all(from >= 0.0) && all(from <= 1.0);
    float count = 0.0;
    float3 blended = now;
    if (usable) {
        float4 then = history_at(from);
        float3 old = to_ycc(squash(finite(then.rgb * exposure_ratio)));
        old = clamp(old, max(low, m1 - spread), min(high, m1 + spread));
        float moved = length(move * size);
        float longest = min(lerp(16.0, 6.0, saturate(moved / 20.0)), most);
        count = clamp(then.a, 0.0, longest);
        float share = covered / (covered + count);
        blended = lerp(old, now, saturate(share));
    }
    float3 shown = finite(stretch(saturate(to_rgb(blended))));
    result[id.xy] = float4(shown, 1.0);
    kept[id.xy] = float4(shown, min(count + covered, 16.0));
}
"#;

/// The light the title's ray tracing would have brought (it is off, and
/// what neither the sun nor a lamp reaches was black): the colour of the
/// surface under the light of the sky, less where its surroundings stand
/// close. The picture with it, at the size the title rendered.
const FILL: &str = r#"
#define RS "RootConstants(num32BitConstants = 8, b0), DescriptorTable(SRV(t0, numDescriptors = 3), UAV(u0, numDescriptors = 1))"
cbuffer Constants : register(b0) { float3 light; float far_is_one; uint2 size; uint frame; uint pad; };
Texture2D<float4> colour : register(t0);
Texture2D<float> depth : register(t1);
Texture2D<float4> surface : register(t2);
RWTexture2D<float4> result : register(u0);
float nearness(int2 p) {
    float z = depth.Load(int3(p, 0));
    return far_is_one > 0.5 ? 1.0 - z : z;
}
[RootSignature(RS)]
[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (any(id.xy >= size)) return;
    int2 p = int2(id.xy), last = int2(size) - 1;
    float4 lit = colour.Load(int3(p, 0));
    float z = nearness(p);
    if (z > 0.0) {
        float turn = 6.2831853 * frac(52.9829189 * frac(dot(float2(p) + 5.588238 * (frame & 7), float2(0.06711056, 0.00583715))));
        float shut = 0.0;
        for (int i = 0; i < 6; i++) {
            float a = turn + i * 2.39996;
            float2 off = float2(cos(a), sin(a)) * (2.5 + 4.5 * i);
            // How much nearer than this point, as a share of the distance.
            float nearer = 1.0 - z / max(nearness(clamp(p + int2(off), 0, last)), 1e-7);
            shut += saturate(nearer * 40.0) * saturate(1.0 - nearer * 6.0);
        }
        float3 s = surface.Load(int3(p, 0)).rgb;
        lit.rgb += s * s * light * (1.0 - 0.7 * saturate(shut / 4.0));
    }
    result[id.xy] = lit;
}
"#;

/// What the title's call names: resources (null if none) and numbers.
#[derive(Debug, Clone, Copy)]
pub struct Dispatch {
    pub list: usize,
    pub colour: usize,
    pub depth: usize,
    pub motion: usize,
    pub output: usize,
    pub jitter: [f32; 2],
    pub motion_scale: [f32; 2],
    pub render: [u32; 2],
    pub size: [u32; 2],
    pub pre_exposure: f32,
    pub reset: bool,
    /// Depth grows towards the far plane.
    pub far_is_one: bool,
}

const SETS: u32 = 64;
const VIEWS: u32 = 6;
const READ: u32 = 0x40 | 0x80;
const WRITE: u32 = 0x8;

struct Pass {
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
    /// [`FILL`]'s, if it was made.
    fill: Option<(ID3D12RootSignature, ID3D12PipelineState)>,
    views: ID3D12DescriptorHeap,
    cpu: usize,
    gpu: u64,
    step: u32,
    next: AtomicU32,
}

// SAFETY: D3D12 objects are free-threaded.
unsafe impl Send for Pass {}
// SAFETY: as above.
unsafe impl Sync for Pass {}

struct History {
    size: [u32; 2],
    textures: [ID3D12Resource; 2],
    /// The one written last.
    last: usize,
    pre_exposure: f32,
    fresh: bool,
}

// SAFETY: D3D12 objects are free-threaded.
unsafe impl Send for History {}

/// How many frames before it a frame is blended with, at most
/// (`VICTORY_UPSCALE_HISTORY`, 16; 0: each is upscaled on its own). At ten
/// frames a second things move far between two of them, and what the frames
/// before held smears behind them; with none of them the title's dithering
/// shows as dots and edges are rough.
fn most() -> f32 {
    static MOST: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *MOST.get_or_init(|| {
        let asked = std::env::var("VICTORY_UPSCALE_HISTORY").ok().and_then(|v| v.trim().parse::<f32>().ok());
        asked.filter(|n| n.is_finite()).map_or(16.0, |n| n.clamp(0.0, 16.0))
    })
}

/// Where the title's shaders write the light of the sky, by direction (its
/// "ambient cube", in its own buffer): noted by `crate::draw` at the
/// dispatch that writes it.
pub(crate) static SKY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How much of the sky's light is put on surfaces in the place of the
/// title's ray-traced light: `VICTORY_FILL` (0, the default: none).
pub(crate) fn filling() -> f32 {
    static SHARE: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *SHARE.get_or_init(|| {
        let asked = std::env::var("VICTORY_FILL").ok().and_then(|v| v.trim().parse::<f32>().ok());
        asked.filter(|n| n.is_finite()).map_or(0.0, |n| n.clamp(0.0, 8.0))
    })
}

/// The light to fill with: the mean of the sky's over the directions, times
/// [`filling`]. `None` while it is not known.
fn sky_light() -> Option<[f32; 3]> {
    let share = filling();
    #[cfg(feature = "diagnostics")]
    if crate::capture::knob("nofill") {
        return None;
    }
    let at = SKY.load(Ordering::Relaxed);
    if share == 0.0 || at == 0 {
        return None;
    }
    let mut bytes = [0u8; 384];
    if !peek(at as usize, &mut bytes) {
        return None;
    }
    let words: Vec<f32> = bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
    static LOGGED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let now = crate::host::now_ms();
    if now > LOGGED.load(Ordering::Relaxed) + 10_000 {
        LOGGED.store(now, Ordering::Relaxed);
        tracing::info!(?words, "the light of the sky");
    }
    let mut mean = [0f32; 3];
    for direction in words[..18].as_chunks::<3>().0 {
        for (m, d) in mean.iter_mut().zip(direction) {
            *m += d / 6.0;
        }
    }
    // Half of its blue: what rooms and streets give back is not the sky's.
    let grey = mean[0] * 0.2126 + mean[1] * 0.7152 + mean[2] * 0.0722;
    let mean = mean.map(|m| (m + grey) / 2.0);
    #[cfg(feature = "diagnostics")]
    let share = ["e0", "e1", "e2", "e3", "e4", "e5", "e6"]
        .iter()
        .position(|name| crate::capture::knob(name))
        .map_or(share, |n| 10f32.powi(-(n as i32)) * if crate::capture::knob("x3") { 3.0 } else { 1.0 });
    mean.iter().all(|m| m.is_finite() && *m >= 0.0).then(|| mean.map(|m| m * share))
}

fn pass() -> Option<&'static Pass> {
    static PASS: std::sync::OnceLock<Option<Pass>> = std::sync::OnceLock::new();
    PASS.get_or_init(|| match make() {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!(error = %e, "no upscaler of ours");
            None
        }
    })
    .as_ref()
}

fn pipeline(device: &ID3D12Device, source: &'static str) -> windows::core::Result<(ID3D12RootSignature, ID3D12PipelineState)> {
    let code = crate::host::compile(source, s!("main"), s!("cs_5_1"))?;
    // SAFETY: plain object creation; the root signature is in the shader.
    unsafe {
        let root: ID3D12RootSignature = device.CreateRootSignature(0, &code[..])?;
        let pso = device.CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(&root),
            CS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: code.as_ptr().cast(),
                BytecodeLength: code.len(),
            },
            ..Default::default()
        })?;
        Ok((root, pso))
    }
}

fn make() -> windows::core::Result<Pass> {
    let device = &host().ok_or_else(windows::core::Error::empty)?.device;
    let (root, pso) = pipeline(device, SOURCE)?;
    let fill = (filling() > 0.0).then(|| pipeline(device, FILL)).and_then(|made| {
        made.inspect_err(|e| tracing::warn!(error = %e, "no pass of ours for the light of the sky")).ok()
    });
    let kind = D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV;
    // SAFETY: plain object creation.
    unsafe {
        let views: ID3D12DescriptorHeap =
            device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: kind,
                NumDescriptors: SETS * VIEWS,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                NodeMask: 0,
            })?;
        Ok(Pass {
            root,
            pso,
            fill,
            cpu: views.GetCPUDescriptorHandleForHeapStart().ptr,
            gpu: views.GetGPUDescriptorHandleForHeapStart().ptr,
            step: device.GetDescriptorHandleIncrementSize(kind),
            views,
            next: AtomicU32::new(0),
        })
    }
}

fn history_texture(device: &ID3D12Device, size: [u32; 2]) -> Option<ID3D12Resource> {
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        ..Default::default()
    };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: u64::from(size[0]),
        Height: size[1],
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
        ..Default::default()
    };
    let mut made: Option<ID3D12Resource> = None;
    // SAFETY: plain creation; out pointer ours.
    unsafe {
        device
            .CreateCommittedResource(
                &heap,
                D3D12_HEAP_FLAG_NONE,
                &desc,
                D3D12_RESOURCE_STATES(READ as i32),
                None,
                &mut made,
            )
            .ok()?;
    }
    made
}

/// The format a texture is read as: depth and typeless families by name.
fn read_as(format: DXGI_FORMAT) -> DXGI_FORMAT {
    DXGI_FORMAT(match format.0 {
        9 => 10,
        19 | 20 => 21,
        23 => 24,
        27 => 28,
        33 => 34,
        39 | 40 => 41,
        44 | 45 => 46,
        53 | 55 => 56,
        other => other,
    })
}

/// What the title's last call named (colour, depth, size rendered, whether
/// depth grows away): the scene [`fill_scene`] lights.
static VIEW: Mutex<Option<(usize, usize, [u32; 2], bool, f32)>> = Mutex::new(None);

/// The size of the scene the title last had upscaled.
pub(crate) fn scene_size() -> Option<[u32; 2]> {
    VIEW.lock().unwrap_or_else(|e| e.into_inner()).map(|v| v.2)
}

/// When [`fill_scene`] last lit the scene before the title metered it (ms,
/// 0: never): the upscale does not light it again then.
static METERED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Records, before the title's draw that meters the scene, the light the
/// title did not trace, into the scene itself: the exposure is taken with
/// it, and what is drawn afterwards (glass, smoke) lies over it.
pub(crate) fn fill_scene(list: &HostList) -> bool {
    let Some((colour, depth, render, far_is_one, pre_exposure)) = *VIEW.lock().unwrap_or_else(|e| e.into_inner()) else {
        return false;
    };
    let done = || -> Option<()> {
        let (host, pass) = (host()?, pass()?);
        if !crate::resource::is_texture(colour) || !crate::resource::is_texture(depth) {
            return None;
        }
        let scene = crate::resource::host_resource(colour as This)?;
        // SAFETY: a live resource.
        let desc = unsafe { scene.GetDesc() };
        let size = [desc.Width as u32, desc.Height];
        if desc.Format != DXGI_FORMAT_R16G16B16A16_FLOAT || desc.DepthOrArraySize != 1 {
            return None;
        }
        let to = fill(list, host, pass, (colour, depth), size, (render, far_is_one, pre_exposure))?;
        const FROM: u32 = 0x800;
        const TO: u32 = 0x400;
        crate::list::ensure_state(list, colour as This, TO);
        let (read, copy) = (D3D12_RESOURCE_STATES(READ as i32), D3D12_RESOURCE_STATES(FROM as i32));
        let at = |r: &ID3D12Resource| D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: borrowed for the call below only.
            pResource: unsafe { std::mem::transmute_copy(r) },
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
        };
        // SAFETY: an open list and live textures of one size and format.
        unsafe {
            list.list.ResourceBarrier(&[transition(&to, read, copy)]);
            list.list.CopyTextureRegion(&at(scene), 0, 0, 0, &at(&to), None);
            list.list.ResourceBarrier(&[transition(&to, copy, read)]);
        }
        Some(())
    }()
    .is_some();
    if done {
        METERED.store(crate::host::now_ms().max(1), Ordering::Relaxed);
    }
    #[cfg(feature = "diagnostics")]
    {
        static WAS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
        static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let now = crate::host::now_ms();
        if WAS.swap(done, Ordering::Relaxed) != done || now > LAST.load(Ordering::Relaxed) + 5000 {
            LAST.store(now, Ordering::Relaxed);
            let size = |this: This| {
                crate::resource::host_resource(this).map(|t| {
                    // SAFETY: a live resource.
                    let d = unsafe { t.GetDesc() };
                    (d.Width, d.Height, d.Format.0)
                })
            };
            let surface = crate::draw::surface_colours();
            tracing::info!(
                done,
                light = ?sky_light(),
                colour = ?size(colour as This),
                depth = ?size(depth as This),
                surface = ?surface.map(|s| (s, size(s))),
                ?render,
                "the scene lit before it is metered"
            );
        }
    }
    done
}

/// Records [`FILL`] over the scene `colour` with `depth`: the texture with
/// the result, left to be read. `None` while what it needs is not known.
fn fill(
    list: &HostList,
    host: &crate::host::Host,
    pass: &Pass,
    (colour, depth): (usize, usize),
    size: [u32; 2],
    (render, far_is_one, pre_exposure): ([u32; 2], bool, f32),
) -> Option<ID3D12Resource> {
    static FILLED: Mutex<Option<([u32; 2], ID3D12Resource)>> = Mutex::new(None);
    let light = sky_light()?;
    let (root, pso) = pass.fill.as_ref()?;
    let surface = crate::draw::surface_colours()?;
    let textures = [colour as This, depth as This, surface];
    let mut hosts = Vec::with_capacity(3);
    for this in textures {
        let texture = crate::resource::host_resource(this)?;
        // SAFETY: a live resource.
        let desc = unsafe { texture.GetDesc() };
        if [desc.Width as u32, desc.Height] != size {
            return None;
        }
        hosts.push(texture);
    }
    let mut kept = FILLED.lock().unwrap_or_else(|e| e.into_inner());
    if kept.as_ref().is_none_or(|k| k.0 != size) {
        // (the one before may still be in a flight: never released)
        std::mem::forget(kept.replace((size, history_texture(&host.device, size)?)));
    }
    let to = kept.as_ref()?.1.clone();
    for this in textures {
        list.twin_use(this, false, false);
        crate::list::ensure_state(list, this, READ);
    }
    let render = [render[0].min(size[0]), render[1].min(size[1])];
    let mut words = [0u32; 8];
    for (w, l) in words.iter_mut().zip(light) {
        *w = (l * pre_exposure.max(0.0)).to_bits();
    }
    words[3] = f32::from(u8::from(far_is_one)).to_bits();
    words[4..7].copy_from_slice(&[render[0], render[1], crate::frame::current() as u32]);
    let slot = pass.next.fetch_add(1, Ordering::Relaxed) % SETS * VIEWS;
    let cpu = |i: u32| D3D12_CPU_DESCRIPTOR_HANDLE {
        ptr: pass.cpu + ((slot + i) * pass.step) as usize,
    };
    for (i, texture) in hosts.iter().enumerate() {
        // SAFETY: a live resource and a slot of our heap.
        unsafe {
            let view = D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: read_as(texture.GetDesc().Format),
                ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D12_TEX2D_SRV {
                        MipLevels: 1,
                        ..Default::default()
                    },
                },
            };
            host.device.CreateShaderResourceView(*texture, Some(&view), cpu(i as u32));
        }
    }
    let view = D3D12_UNORDERED_ACCESS_VIEW_DESC {
        Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
        ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
        ..Default::default()
    };
    let (read, write) = (D3D12_RESOURCE_STATES(READ as i32), D3D12_RESOURCE_STATES(WRITE as i32));
    let l = &list.list;
    // SAFETY: an open list; live resources; objects that live as long as
    // the process.
    unsafe {
        host.device.CreateUnorderedAccessView(&to, None, Some(&view), cpu(3));
        l.ResourceBarrier(&[transition(&to, read, write)]);
        l.SetDescriptorHeaps(&[Some(pass.views.clone())]);
        l.SetComputeRootSignature(root);
        l.SetPipelineState(pso);
        l.SetComputeRoot32BitConstants(0, 8, words.as_ptr().cast(), 0);
        let ptr = pass.gpu + u64::from(slot * pass.step);
        l.SetComputeRootDescriptorTable(1, D3D12_GPU_DESCRIPTOR_HANDLE { ptr });
        l.Dispatch(render[0].div_ceil(8), render[1].div_ceil(8), 1);
        l.ResourceBarrier(&[transition(&to, write, read)]);
    }
    Some(to)
}

/// Records the upscale into the title's list: whether it was.
/// shortcut: one context (one view) at a time, as the title uses it.
pub fn record(d: &Dispatch) -> bool {
    let list = d.list as This;
    let mut done = false;
    crate::list::with_host_list(list, |host_list, state| {
        done = record_into(host_list, d).is_some();
        if done {
            state.host_state_lost();
        }
    });
    done
}

fn record_into(list: &HostList, d: &Dispatch) -> Option<()> {
    static HISTORY: Mutex<Option<History>> = Mutex::new(None);
    if d.size[0] == 0 || d.size[1] == 0 || d.render[0] == 0 || d.render[1] == 0 {
        return None;
    }
    let (host, pass) = (host()?, pass()?);
    let device = &host.device;
    let colour = crate::resource::host_resource(d.colour as This)?;
    let output = crate::resource::host_resource(d.output as This)?;
    let depth = crate::resource::host_resource(d.depth as This)?;
    let motion = (d.motion != 0)
        .then(|| crate::resource::host_resource(d.motion as This))
        .flatten();
    let mut history = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    if history.as_ref().is_none_or(|h| h.size != d.size) {
        let textures = [history_texture(device, d.size)?, history_texture(device, d.size)?];
        tracing::info!(render = ?d.render, size = ?d.size, "upscaler of ours in the title's place");
        // The ones before may still be in a flight: never released.
        std::mem::forget(history.replace(History {
            size: d.size,
            textures,
            last: 0,
            pre_exposure: d.pre_exposure,
            fresh: true,
        }));
    }
    let h = history.as_mut()?;
    let (before, after) = (h.last, 1 - h.last);
    let reset = d.reset || h.fresh || most() == 0.0;
    let ratio = if h.pre_exposure > 0.0 && d.pre_exposure > 0.0 {
        d.pre_exposure / h.pre_exposure
    } else {
        1.0
    };
    // SAFETY: live resources.
    let (output_desc, colour_desc) = unsafe { (output.GetDesc(), colour.GetDesc()) };
    let render = [
        d.render[0].min(colour_desc.Width as u32),
        d.render[1].min(colour_desc.Height),
    ];
    let mut constants = [0f32; 24];
    constants[0] = render[0] as f32;
    constants[1] = render[1] as f32;
    constants[2] = d.size[0] as f32;
    constants[3] = d.size[1] as f32;
    constants[4..6].copy_from_slice(&d.jitter);
    constants[6..8].copy_from_slice(&d.motion_scale);
    constants[8] = f32::from(u8::from(reset));
    constants[14] = most();
    constants[9] = ratio;
    constants[10] = f32::from(u8::from(d.far_is_one));
    constants[11] = f32::from(u8::from(motion.is_some()));
    constants[12] = d.size[0] as f32;
    constants[13] = d.size[1] as f32;
    *VIEW.lock().unwrap_or_else(|e| e.into_inner()) = Some((d.colour, d.depth, render, d.far_is_one, d.pre_exposure));
    // The picture with the light the title did not trace, if the scene was
    // not lit before the title metered it.
    let metered = METERED.load(Ordering::Relaxed);
    let filled = (metered == 0 || crate::host::now_ms() > metered + 3000)
        .then(|| {
            let size = [colour_desc.Width as u32, colour_desc.Height];
            fill(list, host, pass, (d.colour, d.depth), size, (render, d.far_is_one, d.pre_exposure))
        })
        .flatten();
    // The title's textures to the states the pass uses them in.
    for texture in [d.colour, d.depth].into_iter().chain(motion.map(|_| d.motion)) {
        list.twin_use(texture as This, false, false);
        crate::list::ensure_state(list, texture as This, READ);
    }
    crate::list::ensure_state(list, d.output as This, WRITE);
    let l = &list.list;
    let slot = pass.next.fetch_add(1, Ordering::Relaxed) % SETS * VIEWS;
    let cpu = |i: u32| D3D12_CPU_DESCRIPTOR_HANDLE {
        ptr: pass.cpu + ((slot + i) * pass.step) as usize,
    };
    let srv = |texture: &ID3D12Resource, i: u32| {
        // SAFETY: a live resource and a slot of our heap.
        unsafe {
            let view = D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: read_as(texture.GetDesc().Format),
                ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D12_TEX2D_SRV {
                        MipLevels: 1,
                        ..Default::default()
                    },
                },
            };
            device.CreateShaderResourceView(texture, Some(&view), cpu(i));
        }
    };
    let uav = |texture: &ID3D12Resource, format: DXGI_FORMAT, i: u32| {
        let view = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: format,
            ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
            ..Default::default()
        };
        // SAFETY: a live resource and a slot of our heap.
        unsafe { device.CreateUnorderedAccessView(texture, None, Some(&view), cpu(i)) };
    };
    srv(filled.as_ref().unwrap_or(colour), 0);
    srv(depth, 1);
    srv(motion.unwrap_or(colour), 2);
    srv(&h.textures[before], 3);
    uav(output, read_as(output_desc.Format), 4);
    uav(&h.textures[after], DXGI_FORMAT_R16G16B16A16_FLOAT, 5);
    let (read, write) = (D3D12_RESOURCE_STATES(READ as i32), D3D12_RESOURCE_STATES(WRITE as i32));
    // SAFETY: an open list; live resources; objects that live as long as
    // the process.
    unsafe {
        l.ResourceBarrier(&[transition(&h.textures[after], read, write)]);
        l.SetDescriptorHeaps(&[Some(pass.views.clone())]);
        l.SetComputeRootSignature(&pass.root);
        l.SetPipelineState(&pass.pso);
        l.SetComputeRoot32BitConstants(0, 24, constants.as_ptr().cast(), 0);
        l.SetComputeRootDescriptorTable(
            1,
            D3D12_GPU_DESCRIPTOR_HANDLE {
                ptr: pass.gpu + u64::from(slot * pass.step),
            },
        );
        l.Dispatch(d.size[0].div_ceil(8), d.size[1].div_ceil(8), 1);
        l.ResourceBarrier(&[
            transition(&h.textures[after], write, read),
            // The title reads what the pass wrote, whatever barriers it has.
            D3D12_RESOURCE_BARRIER {
                Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
                ..Default::default()
            },
        ]);
    }
    h.last = after;
    h.pre_exposure = d.pre_exposure;
    h.fresh = false;
    Some(())
}

/// Whether `at` is one of our lists, read through the kernel (a pointer the
/// title passes in a structure whose layout we only believe we know).
pub fn is_list(at: usize) -> bool {
    table_of(at).is_some_and(crate::list::is_table)
}

/// Whether `at` is one of our resources, likewise.
pub fn is_resource(at: usize) -> bool {
    table_of(at).is_some_and(crate::resource::is_table)
}

fn table_of(at: usize) -> Option<usize> {
    let mut bytes = [0u8; 8];
    (at > 0xffff && at.is_multiple_of(8) && crate::host::read_into(at as *const u8, &mut bytes))
        .then(|| usize::from_le_bytes(bytes))
}

/// Reads `out.len()` bytes of the title's at `at`, through the kernel.
pub fn peek(at: usize, out: &mut [u8]) -> bool {
    at > 0xffff && crate::host::read_into(at as *const u8, out)
}
