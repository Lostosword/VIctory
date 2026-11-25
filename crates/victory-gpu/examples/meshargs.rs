//! Dessins de meshes indirects : le décalage des arguments et leur attente, sur la carte.
#[cfg(not(windows))]
fn main() {}
#[cfg(windows)]
fn main() {
    use windows::Win32::Graphics::Direct3D::*;
    use windows::Win32::Graphics::Direct3D12::*;
    use windows::Win32::Graphics::Dxgi::Common::*;
    use windows::core::Interface;
    let dir = std::env::args().nth(1).expect("dossier");
    let mode: u32 = std::env::args().nth(2).and_then(|m| m.parse().ok()).unwrap_or(0);
    let read = |n: &str| std::fs::read(format!("{dir}/{n}.dxil")).unwrap();
    let (ms, ps, cs) = (read("ms"), read("ps"), read("cs"));
    unsafe {
        let mut device: Option<ID3D12Device> = None;
        D3D12CreateDevice(None, D3D_FEATURE_LEVEL_12_0, &mut device).unwrap();
        let device = device.unwrap();
        let queue: ID3D12CommandQueue = device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC::default()).unwrap();
        let alloc: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).unwrap();
        let list: ID3D12GraphicsCommandList6 = device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &alloc, None).unwrap();
        let params = [
            D3D12_ROOT_PARAMETER { ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS, Anonymous: D3D12_ROOT_PARAMETER_0 { Constants: D3D12_ROOT_CONSTANTS { ShaderRegister: 0, RegisterSpace: 0, Num32BitValues: 2 } }, ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL },
            D3D12_ROOT_PARAMETER { ParameterType: D3D12_ROOT_PARAMETER_TYPE_UAV, Anonymous: D3D12_ROOT_PARAMETER_0 { Descriptor: D3D12_ROOT_DESCRIPTOR { ShaderRegister: 0, RegisterSpace: 0 } }, ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL },
        ];
        let desc = D3D12_ROOT_SIGNATURE_DESC { NumParameters: 2, pParameters: params.as_ptr(), ..Default::default() };
        let mut blob = None;
        D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &mut blob, None).unwrap();
        let blob: ID3DBlob = blob.unwrap();
        let root: ID3D12RootSignature = device.CreateRootSignature(0, std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())).unwrap();
        let code = |c: &Vec<u8>| D3D12_SHADER_BYTECODE { pShaderBytecode: c.as_ptr().cast(), BytecodeLength: c.len() };
        let compute: ID3D12PipelineState = device.CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC { pRootSignature: std::mem::transmute_copy(&root), CS: code(&cs), ..Default::default() }).expect("cs");
        #[repr(C, align(8))]
        struct Sub<T>(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE, T);
        #[repr(C)]
        struct Stream { root: Sub<*mut std::ffi::c_void>, ms: Sub<D3D12_SHADER_BYTECODE>, ps: Sub<D3D12_SHADER_BYTECODE>, blend: Sub<D3D12_BLEND_DESC>, raster: Sub<D3D12_RASTERIZER_DESC>, depth: Sub<D3D12_DEPTH_STENCIL_DESC>, targets: Sub<D3D12_RT_FORMAT_ARRAY>, samples: Sub<DXGI_SAMPLE_DESC>, topo: Sub<D3D12_PRIMITIVE_TOPOLOGY_TYPE> }
        let mut blend = D3D12_BLEND_DESC::default();
        blend.RenderTarget[0].RenderTargetWriteMask = 15;
        let mut targets = D3D12_RT_FORMAT_ARRAY { NumRenderTargets: 1, ..Default::default() };
        targets.RTFormats[0] = DXGI_FORMAT_R8G8B8A8_UNORM;
        let mut stream = Stream {
            root: Sub(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_ROOT_SIGNATURE, root.as_raw()),
            ms: Sub(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_MS, code(&ms)),
            ps: Sub(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_PS, code(&ps)),
            blend: Sub(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_BLEND, blend),
            raster: Sub(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_RASTERIZER, D3D12_RASTERIZER_DESC { FillMode: D3D12_FILL_MODE_SOLID, CullMode: D3D12_CULL_MODE_NONE, DepthClipEnable: true.into(), ..Default::default() }),
            depth: Sub(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_DEPTH_STENCIL, D3D12_DEPTH_STENCIL_DESC::default()),
            targets: Sub(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_RENDER_TARGET_FORMATS, targets),
            samples: Sub(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_SAMPLE_DESC, DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }),
            topo: Sub(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_PRIMITIVE_TOPOLOGY, D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE),
        };
        let pso: ID3D12PipelineState = device.cast::<ID3D12Device2>().unwrap().CreatePipelineState(&D3D12_PIPELINE_STATE_STREAM_DESC { SizeInBytes: size_of::<Stream>(), pPipelineStateSubobjectStream: std::ptr::from_mut(&mut stream).cast() }).expect("mesh");
        let heap = |t| D3D12_HEAP_PROPERTIES { Type: t, ..Default::default() };
        let buffer = |size: u64, t, flags, state| -> ID3D12Resource {
            let d = D3D12_RESOURCE_DESC { Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: size, Height: 1, DepthOrArraySize: 1, MipLevels: 1, SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR, Flags: flags, ..Default::default() };
            let mut r: Option<ID3D12Resource> = None;
            device.CreateCommittedResource(&heap(t), D3D12_HEAP_FLAG_NONE, &d, state, None, &mut r).unwrap(); r.unwrap()
        };
        let (none, uavf) = (D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
        let (common, uav, args, rt, source, dest) = (D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
        let barrier = |r: &ID3D12Resource, a, b| D3D12_RESOURCE_BARRIER { Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION, Anonymous: D3D12_RESOURCE_BARRIER_0 { Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER { pResource: std::mem::transmute_copy(r), Subresource: u32::MAX, StateBefore: a, StateAfter: b }) }, ..Default::default() };
        let uavall = || D3D12_RESOURCE_BARRIER { Type: D3D12_RESOURCE_BARRIER_TYPE_UAV, ..Default::default() };
        let mut rts = Vec::new();
        for _ in 0..2 {
            let d = D3D12_RESOURCE_DESC { Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D, Width: 64, Height: 64, DepthOrArraySize: 1, MipLevels: 1, Format: DXGI_FORMAT_R8G8B8A8_UNORM, SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET, ..Default::default() };
            let mut r: Option<ID3D12Resource> = None;
            device.CreateCommittedResource(&heap(D3D12_HEAP_TYPE_DEFAULT), D3D12_HEAP_FLAG_NONE, &d, rt, None, &mut r).unwrap(); rts.push(r.unwrap());
        }
        let rh: ID3D12DescriptorHeap = device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV, NumDescriptors: 2, ..Default::default() }).unwrap();
        let step = device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV) as usize;
        let rtv = |i: usize| D3D12_CPU_DESCRIPTOR_HANDLE { ptr: rh.GetCPUDescriptorHandleForHeapStart().ptr + i * step };
        for i in 0..2 { device.CreateRenderTargetView(&rts[i], None, rtv(i)); }
        let sig = |stride: u32| -> ID3D12CommandSignature {
            let a = D3D12_INDIRECT_ARGUMENT_DESC { Type: D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH_MESH, ..Default::default() };
            let mut s: Option<ID3D12CommandSignature> = None;
            device.CreateCommandSignature(&D3D12_COMMAND_SIGNATURE_DESC { ByteStride: stride, NumArgumentDescs: 1, pArgumentDescs: &a, NodeMask: 0 }, None, &mut s).unwrap();
            s.unwrap()
        };
        let signature = sig(12);
        let arena = buffer(1 << 20, D3D12_HEAP_TYPE_DEFAULT, uavf, common);
        let clamp = |base: u32, before, spin: u32| {
            list.ResourceBarrier(&[uavall(), barrier(&arena, before, uav)]);
            list.SetComputeRootSignature(&root);
            list.SetPipelineState(&compute);
            list.SetComputeRoot32BitConstants(0, 2, [base, spin].as_ptr().cast(), 0);
            list.SetComputeRootUnorderedAccessView(1, arena.GetGPUVirtualAddress());
            list.Dispatch(1, 1, 1);
            list.ResourceBarrier(&[uavall(), barrier(&arena, uav, args)]);
        };
        let spin = 200000;
        let vp = D3D12_VIEWPORT { Width: 64.0, Height: 64.0, MaxDepth: 1.0, ..Default::default() };
        let rect = windows::Win32::Foundation::RECT { left: 0, top: 0, right: 64, bottom: 64 };
        list.RSSetViewports(&[vp]); list.RSSetScissorRects(&[rect]);
        for i in 0..2 { list.ClearRenderTargetView(rtv(i), &[0.0; 4], None); }
        let pass = |i: usize| { list.OMSetRenderTargets(1, Some(&rtv(i)), false, None); };
        let ready = || { list.SetGraphicsRootSignature(&root); list.SetPipelineState(&pso); };
        let draw = |k: u32, base: u32| {
            list.SetGraphicsRoot32BitConstants(0, 2, [k, 0].as_ptr().cast(), 0);
            list.ExecuteIndirect(&signature, 1, &arena, u64::from(base + k * 16), None, 0);
        };
        clamp(0, common, spin);
        match mode {
            // Une passe.
            0 => { pass(0); ready(); for k in 0..64 { draw(k, 0); } }
            // Deux passes, une attente.
            1 => { pass(0); ready(); for k in 0..32 { draw(k, 0); } pass(1); ready(); for k in 32..64 { draw(k, 0); } }
            // Deux passes, la seconde dessinée d'un bloc calculé entre elles.
            2 => { pass(0); ready(); for k in 0..32 { draw(k, 0); } clamp(4096, args, spin); ready(); for k in 32..64 { draw(k, 4096); } }
            // Pareil, et encore du premier bloc après.
            _ => { pass(0); ready(); for k in 0..16 { draw(k, 0); } clamp(4096, args, spin); ready(); for k in 16..32 { draw(k, 4096); } for k in 32..48 { draw(k, 0); } pass(1); ready(); for k in 48..64 { draw(k, 4096); } }
        }
        let mut backs = Vec::new();
        for i in 0..2 {
            list.ResourceBarrier(&[barrier(&rts[i], rt, source)]);
            let ra = buffer(256 * 64, D3D12_HEAP_TYPE_READBACK, none, dest);
            let src = D3D12_TEXTURE_COPY_LOCATION { pResource: std::mem::transmute_copy(&rts[i]), Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX, Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 } };
            let dst = D3D12_TEXTURE_COPY_LOCATION { pResource: std::mem::transmute_copy(&ra), Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT { Offset: 0, Footprint: D3D12_SUBRESOURCE_FOOTPRINT { Format: DXGI_FORMAT_R8G8B8A8_UNORM, Width: 64, Height: 64, Depth: 1, RowPitch: 256 } } } };
            list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
            backs.push(ra);
        }
        list.Close().unwrap();
        queue.ExecuteCommandLists(&[Some(list.cast().unwrap())]);
        let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
        queue.Signal(&fence, 1).unwrap();
        for _ in 0..500 { if fence.GetCompletedValue() >= 1 { break } std::thread::sleep(std::time::Duration::from_millis(10)); }
        let mut lit = [0u32; 64];
        for ra in &backs {
            let mut p = std::ptr::null_mut(); ra.Map(0, None, Some(&mut p)).unwrap();
            for k in 0..64 { for y in 0..64 { if p.cast::<u8>().add(y * 256 + k * 4).read() > 0 { lit[k] += 1; } } }
        }
        let bad: Vec<usize> = (0..64).filter(|&k| lit[k] != k as u32 % 8 + 1).collect();
        println!("mode {mode} fence {} : {} colonnes fausses sur 64 {:?} au lieu de 1..8 : {:?}", fence.GetCompletedValue(), bad.len(), bad, lit);
    }
}
