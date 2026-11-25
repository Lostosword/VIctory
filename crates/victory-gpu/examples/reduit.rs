//! Rejoue un shader de calcul (DXIL) hors du jeu : heap direct, entrées connues, sorties relues.
#[cfg(not(windows))]
fn main() {}
#[cfg(windows)]
fn main() {
    use windows::Win32::Graphics::Direct3D::*;
    use windows::Win32::Graphics::Direct3D12::*;
    use windows::Win32::Graphics::Dxgi::Common::*;
    use windows::core::Interface;
    let path = std::env::args().nth(1).expect("dxil");
    let code = std::fs::read(&path).unwrap();
    unsafe {
        let mut device: Option<ID3D12Device> = None;
        D3D12CreateDevice(None, D3D_FEATURE_LEVEL_12_0, &mut device).unwrap();
        let device = device.unwrap();
        let queue: ID3D12CommandQueue = device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC::default()).unwrap();
        let alloc: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).unwrap();
        let list: ID3D12GraphicsCommandList = device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &alloc, None).unwrap();
        let params = [D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_CBV,
            Anonymous: D3D12_ROOT_PARAMETER_0 { Descriptor: D3D12_ROOT_DESCRIPTOR { ShaderRegister: 0, RegisterSpace: 0 } },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        }];
        let samp = D3D12_STATIC_SAMPLER_DESC { Filter: D3D12_FILTER_MIN_MAG_MIP_POINT, AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP, AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP, AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP, MaxLOD: 100.0, ..Default::default() };
        let desc = D3D12_ROOT_SIGNATURE_DESC {
            NumParameters: 1, pParameters: params.as_ptr(), NumStaticSamplers: 1, pStaticSamplers: &samp,
            Flags: D3D12_ROOT_SIGNATURE_FLAGS(0x400 | 0x800), ..Default::default()
        };
        let mut blob = None; let mut err = None;
        D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &mut blob, Some(&mut err)).unwrap();
        let blob: ID3DBlob = blob.unwrap();
        let root: ID3D12RootSignature = device.CreateRootSignature(0, std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())).unwrap();
        let pso: ID3D12PipelineState = device.CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(&root),
            CS: D3D12_SHADER_BYTECODE { pShaderBytecode: code.as_ptr().cast(), BytecodeLength: code.len() }, ..Default::default()
        }).expect("pso");
        let heap = |t| D3D12_HEAP_PROPERTIES { Type: t, ..Default::default() };
        let buffer = |size: u64, t, flags, state| -> ID3D12Resource {
            let d = D3D12_RESOURCE_DESC { Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: size, Height: 1, DepthOrArraySize: 1, MipLevels: 1, SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR, Flags: flags, ..Default::default() };
            let mut r: Option<ID3D12Resource> = None;
            device.CreateCommittedResource(&heap(t), D3D12_HEAP_FLAG_NONE, &d, state, None, &mut r).unwrap(); r.unwrap()
        };
        let tex = |w: u64, h: u32, f: DXGI_FORMAT, levels: u16, state| -> ID3D12Resource {
            let d = D3D12_RESOURCE_DESC { Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D, Width: w, Height: h, DepthOrArraySize: 1, MipLevels: levels, Format: f, SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS, ..Default::default() };
            let mut r: Option<ID3D12Resource> = None;
            device.CreateCommittedResource(&heap(D3D12_HEAP_TYPE_DEFAULT), D3D12_HEAP_FLAG_NONE, &d, state, None, &mut r).unwrap(); r.unwrap()
        };
        let barrier = |r: &ID3D12Resource, a, b| D3D12_RESOURCE_BARRIER { Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION, Anonymous: D3D12_RESOURCE_BARRIER_0 { Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER { pResource: std::mem::transmute_copy(r), Subresource: u32::MAX, StateBefore: a, StateAfter: b }) }, ..Default::default() };
        let up = D3D12_HEAP_TYPE_UPLOAD; let none = D3D12_RESOURCE_FLAG_NONE;
        #[allow(unused_variables)] let (cd, cs, uav, srv) = (D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        let t0 = tex(1280, 5120, DXGI_FORMAT_R32_FLOAT, 1, cd);
        let value = |x: u32, y: u32| -> f32 { (x as f32 / 1280.0) * 0.5 + (y as f32 / 5120.0) * 0.5 };
        let pitch = 1280usize * 4;
        let b = buffer((pitch * 5120) as u64, up, none, D3D12_RESOURCE_STATE_GENERIC_READ);
        let mut p = std::ptr::null_mut(); b.Map(0, None, Some(&mut p)).unwrap();
        for y in 0..5120u32 { for x in 0..1280u32 { p.cast::<f32>().add((y * 1280 + x) as usize).write(value(x, y)) } }
        b.Unmap(0, None);
        let dst = D3D12_TEXTURE_COPY_LOCATION { pResource: std::mem::transmute_copy(&t0), Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX, Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 } };
        let src = D3D12_TEXTURE_COPY_LOCATION { pResource: std::mem::transmute_copy(&b), Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT { Offset: 0, Footprint: D3D12_SUBRESOURCE_FOOTPRINT { Format: DXGI_FORMAT_R32_FLOAT, Width: 1280, Height: 5120, Depth: 1, RowPitch: pitch as u32 } } } };
        list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
        list.ResourceBarrier(&[barrier(&t0, cd, srv)]);
        let o0 = tex(160, 640, DXGI_FORMAT_R16G16_UNORM, 1, uav);
        let o1 = tex(640, 2560, DXGI_FORMAT_R16_TYPELESS, 3, uav);
        let rootcb = buffer(256, up, none, D3D12_RESOURCE_STATE_GENERIC_READ);
        let mut q = std::ptr::null_mut(); rootcb.Map(0, None, Some(&mut q)).unwrap();
        std::ptr::write_bytes(q.cast::<u8>(), 0, 256);
        let base = 200u32;
        q.cast::<u32>().add(4).write(base * 32); q.cast::<u32>().add(5).write(base * 32);
        let kind = D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV;
        let dh: ID3D12DescriptorHeap = device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC { Type: kind, NumDescriptors: 1024, Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, NodeMask: 0 }).unwrap();
        let step = device.GetDescriptorHandleIncrementSize(kind) as usize;
        let at = |i: u32| D3D12_CPU_DESCRIPTOR_HANDLE { ptr: dh.GetCPUDescriptorHandleForHeapStart().ptr + i as usize * step };
        device.CreateShaderResourceView(&t0, None, at(base + 29));
        device.CreateUnorderedAccessView(&o0, None, None, at(base));
        let vue = std::env::args().nth(2).is_some();
        for (n, level) in [(1u32, 2u32), (2, 1), (3, 0)] {
            let d = D3D12_UNORDERED_ACCESS_VIEW_DESC { Format: if vue { DXGI_FORMAT_R16_FLOAT } else { DXGI_FORMAT_R16_UNORM }, ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D, Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_UAV { MipSlice: level, PlaneSlice: 0 } } };
            device.CreateUnorderedAccessView(&o1, None, Some(&d), at(base + n));
        }
        list.SetDescriptorHeaps(&[Some(dh.clone())]);
        list.SetComputeRootSignature(&root); list.SetPipelineState(&pso);
        list.SetComputeRootConstantBufferView(0, rootcb.GetGPUVirtualAddress());
        list.Dispatch(40, 160, 1);
        list.ResourceBarrier(&[barrier(&o0, uav, cs)]);
        let pitch2 = (160usize * 4).next_multiple_of(256);
        let ra = buffer((pitch2 * 640) as u64, D3D12_HEAP_TYPE_READBACK, none, cd);
        let src = D3D12_TEXTURE_COPY_LOCATION { pResource: std::mem::transmute_copy(&o0), Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX, Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 } };
        let dst = D3D12_TEXTURE_COPY_LOCATION { pResource: std::mem::transmute_copy(&ra), Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT { Offset: 0, Footprint: D3D12_SUBRESOURCE_FOOTPRINT { Format: DXGI_FORMAT_R16G16_UNORM, Width: 160, Height: 640, Depth: 1, RowPitch: pitch2 as u32 } } } };
        list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
        list.Close().unwrap();
        queue.ExecuteCommandLists(&[Some(list.cast().unwrap())]);
        let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
        queue.Signal(&fence, 1).unwrap();
        for _ in 0..500 { if fence.GetCompletedValue() >= 1 { break } std::thread::sleep(std::time::Duration::from_millis(10)); }
        let mut p = std::ptr::null_mut(); ra.Map(0, None, Some(&mut p)).unwrap();
        let mut bad = 0;
        for ty in 0..640u32 { for tx in 0..160u32 {
            let g = p.cast::<u8>().add(ty as usize * pitch2 + tx as usize * 4).cast::<[u16; 2]>().read();
            let lo = value(tx * 8, ty * 8) * 65535.0; let hi = value(tx * 8 + 7, ty * 8 + 7) * 65535.0;
            if (g[0] as f32 - lo).abs() > 2.0 || (g[1] as f32 - hi).abs() > 2.0 { bad += 1; if bad < 6 { println!("{tx},{ty} : {g:?} au lieu de {lo},{hi}"); } }
        } }
        println!("fence {} : {bad} fausses sur {}", fence.GetCompletedValue(), 160 * 640);
    }
}
