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
    float2 history_size; float2 pad0;
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
        float longest = lerp(16.0, 6.0, saturate(moved / 20.0));
        count = clamp(then.a, 0.0, longest);
        float share = covered / (covered + count);
        blended = lerp(old, now, saturate(share));
    }
    float3 shown = finite(stretch(saturate(to_rgb(blended))));
    result[id.xy] = float4(shown, 1.0);
    kept[id.xy] = float4(shown, min(count + covered, 16.0));
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

fn make() -> windows::core::Result<Pass> {
    let device = &host().ok_or_else(windows::core::Error::empty)?.device;
    let code = crate::host::compile(SOURCE, s!("main"), s!("cs_5_1"))?;
    let kind = D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV;
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
    let reset = d.reset || h.fresh;
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
    constants[9] = ratio;
    constants[10] = f32::from(u8::from(d.far_is_one));
    constants[11] = f32::from(u8::from(motion.is_some()));
    constants[12] = d.size[0] as f32;
    constants[13] = d.size[1] as f32;
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
    srv(colour, 0);
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
