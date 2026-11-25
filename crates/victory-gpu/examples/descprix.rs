//! What making and copying descriptors costs on this host.
#[cfg(not(windows))]
fn main() {}
#[cfg(windows)]
fn main() {
    use windows::Win32::Graphics::Direct3D::*;
    use windows::Win32::Graphics::Direct3D12::*;
    use windows::Win32::Graphics::Dxgi::Common::*;
    unsafe {
        let mut device: Option<ID3D12Device> = None;
        D3D12CreateDevice(None, D3D_FEATURE_LEVEL_12_0, &mut device).unwrap();
        let device = device.unwrap();
        let d = D3D12_RESOURCE_DESC { Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: 1 << 24, Height: 1, DepthOrArraySize: 1, MipLevels: 1, SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR, Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS, ..Default::default() };
        let mut b: Option<ID3D12Resource> = None;
        let props = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
        device.CreateCommittedResource(&props, D3D12_HEAP_FLAG_NONE, &d, D3D12_RESOURCE_STATE_COMMON, None, &mut b).unwrap();
        let b = b.unwrap();
        let t = D3D12_RESOURCE_DESC { Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D, Width: 512, Height: 512, DepthOrArraySize: 1, MipLevels: 9, Format: DXGI_FORMAT_BC7_TYPELESS, SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, ..Default::default() };
        let mut tx: Option<ID3D12Resource> = None;
        device.CreateCommittedResource(&props, D3D12_HEAP_FLAG_NONE, &t, D3D12_RESOURCE_STATE_COMMON, None, &mut tx).unwrap();
        let tx = tx.unwrap();
        let heap = |n, f| -> ID3D12DescriptorHeap { device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, NumDescriptors: n, Flags: f, NodeMask: 0 }).unwrap() };
        let seen = heap(65536, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE);
        let kept = heap(4096, D3D12_DESCRIPTOR_HEAP_FLAG_NONE);
        let step = device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV) as usize;
        let at = |h: &ID3D12DescriptorHeap, i: usize| D3D12_CPU_DESCRIPTOR_HANDLE { ptr: h.GetCPUDescriptorHandleForHeapStart().ptr + i * step };
        let (s0, k0) = (at(&seen, 0).ptr, at(&kept, 0).ptr);
        println!("step {step}, visible heap at {s0:#x}, cpu heap at {k0:#x}");
        let srv = |i: u64| D3D12_SHADER_RESOURCE_VIEW_DESC { Format: DXGI_FORMAT_R32_TYPELESS, ViewDimension: D3D12_SRV_DIMENSION_BUFFER, Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING, Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 { Buffer: D3D12_BUFFER_SRV { FirstElement: i * 16, NumElements: 64, StructureByteStride: 0, Flags: D3D12_BUFFER_SRV_FLAG_RAW } } };
        let tsrv = |_i: u64| D3D12_SHADER_RESOURCE_VIEW_DESC { Format: DXGI_FORMAT_BC7_UNORM, ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D, Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING, Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 9, PlaneSlice: 0, ResourceMinLODClamp: 0.0 } } };
        const N: usize = 40000;
        let time = |name: &str, f: &dyn Fn(usize)| {
            for round in 0..3 {
                let t = std::time::Instant::now();
                for i in 0..N {
                    f(i);
                }
                if round == 2 {
                    println!("{name}: {:.0} ns each", t.elapsed().as_nanos() as f64 / N as f64);
                }
            }
        };
        let h = |p: usize| D3D12_CPU_DESCRIPTOR_HANDLE { ptr: p };
        time("nothing", &|i| {
            std::hint::black_box(i);
        });
        time("Instant::now", &|_| {
            std::hint::black_box(std::time::Instant::now());
        });
        time("Vec of 64 bytes", &|_| {
            std::hint::black_box(Vec::<u8>::with_capacity(64));
        });
        std::thread::scope(|scope| {
            let threads: Vec<_> = (0..5)
                .map(|_| {
                    scope.spawn(|| {
                        let t = std::time::Instant::now();
                        for _ in 0..200000 {
                            std::hint::black_box(Vec::<u8>::with_capacity(64));
                        }
                        t.elapsed().as_nanos() / 200000
                    })
                })
                .collect();
            let each: Vec<u128> = threads.into_iter().map(|t| t.join().unwrap()).collect();
            println!("Vec of 64 bytes, five threads at once: {each:?} ns each");
        });
        let m = std::sync::Mutex::new(0u64);
        time("mutex", &|_| {
            *m.lock().unwrap() += 1;
        });
        let a = std::sync::Arc::new(1u32);
        time("Arc clone", &|_| {
            std::hint::black_box(a.clone());
        });
        let src = vec![1u8; 512];
        let mut dst = vec![0u8; 512];
        let process = windows::Win32::System::Threading::GetCurrentProcess();
        let _ = (&mut dst, process);
        time("GetDesc", &|_| {
            std::hint::black_box(b.GetDesc());
        });
        time("AddRef+Release", &|_| {
            std::hint::black_box(b.clone());
        });
        time("buffer SRV, visible heap", &|i| device.CreateShaderResourceView(&b, Some(&srv(i as u64 % 1000)), h(s0 + i % 65536 * step)));
        time("buffer SRV, cpu heap", &|i| device.CreateShaderResourceView(&b, Some(&srv(i as u64 % 1000)), h(k0 + i % 4096 * step)));
        time("texture SRV, visible heap", &|i| device.CreateShaderResourceView(&tx, Some(&tsrv(i as u64)), h(s0 + i % 65536 * step)));
        time("CBV, visible heap", &|i| device.CreateConstantBufferView(Some(&D3D12_CONSTANT_BUFFER_VIEW_DESC { BufferLocation: b.GetGPUVirtualAddress() + (i as u64 % 1000) * 256, SizeInBytes: 256 }), h(s0 + i % 65536 * step)));
        time("copy 1", &|i| device.CopyDescriptorsSimple(1, h(s0 + i % 65536 * step), h(k0 + i % 4096 * step), D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV));
        time("copy 8 at once (per call)", &|i| device.CopyDescriptorsSimple(8, h(s0 + i % 60000 * step), h(k0 + i % 4000 * step), D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV));
        // Are the bytes at the handle the descriptor?
        let bytes = std::slice::from_raw_parts(k0 as *const u64, step / 8 * 2);
        println!("cpu heap words {bytes:x?}");
        let bytes = std::slice::from_raw_parts(s0 as *const u64, step / 8 * 2);
        println!("visible heap words {bytes:x?}");
    }
}
