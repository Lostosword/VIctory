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
        let desc = D3D12_ROOT_SIGNATURE_DESC {
            NumParameters: 1, pParameters: params.as_ptr(),
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
        let tex = |w: u64, h: u32, f: DXGI_FORMAT, state| -> ID3D12Resource {
            let d = D3D12_RESOURCE_DESC { Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D, Width: w, Height: h, DepthOrArraySize: 1, MipLevels: 1, Format: f, SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS, ..Default::default() };
            let mut r: Option<ID3D12Resource> = None;
            device.CreateCommittedResource(&heap(D3D12_HEAP_TYPE_DEFAULT), D3D12_HEAP_FLAG_NONE, &d, state, None, &mut r).unwrap(); r.unwrap()
        };
        let barrier = |r: &ID3D12Resource, a, b| D3D12_RESOURCE_BARRIER { Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION, Anonymous: D3D12_RESOURCE_BARRIER_0 { Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER { pResource: std::mem::transmute_copy(r), Subresource: u32::MAX, StateBefore: a, StateAfter: b }) }, ..Default::default() };
        let up = D3D12_HEAP_TYPE_UPLOAD; let none = D3D12_RESOURCE_FLAG_NONE;
        let (cd, cs, uav, srv) = (D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        // t0 : drapeaux R8_UINT, t1 : bits R32_UINT
        let t0 = tex(80, 45, DXGI_FORMAT_R8_UINT, cd); let t1 = tex(80, 45, DXGI_FORMAT_R32_UINT, cd);
        let (v0, v1) = (3u32, 0x22u32);
        for (t, size, v) in [(&t0, 1usize, v0), (&t1, 4, v1)] {
            let pitch = (80 * size).next_multiple_of(256);
            let b = buffer((pitch * 45) as u64, up, none, D3D12_RESOURCE_STATE_GENERIC_READ);
            let mut p = std::ptr::null_mut(); b.Map(0, None, Some(&mut p)).unwrap();
            for y in 0..45 { for x in 0..80 { std::ptr::copy_nonoverlapping(v.to_le_bytes().as_ptr(), p.cast::<u8>().add(y * pitch + x * size), size) } }
            b.Unmap(0, None);
            let dst = D3D12_TEXTURE_COPY_LOCATION { pResource: std::mem::transmute_copy(t), Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX, Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 } };
            let src = D3D12_TEXTURE_COPY_LOCATION { pResource: std::mem::transmute_copy(&b), Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT { Offset: 0, Footprint: D3D12_SUBRESOURCE_FOOTPRINT { Format: t.GetDesc().Format, Width: 80, Height: 45, Depth: 1, RowPitch: pitch as u32 } } } };
            list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
            list.ResourceBarrier(&[barrier(t, cd, srv)]);
            std::mem::forget(b);
        }
        let args = buffer(1920, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS, uav);
        let o0 = tex(10, 6, DXGI_FORMAT_R32_UINT, uav); let o1 = tex(10, 6, DXGI_FORMAT_R8_UINT, uav);
        // constantes : le CBV du heap (index 178) et les données racine
        let cb = buffer(65536, up, none, D3D12_RESOURCE_STATE_GENERIC_READ);
        let rootcb = buffer(256, up, none, D3D12_RESOURCE_STATE_GENERIC_READ);
        let mut p = std::ptr::null_mut(); cb.Map(0, None, Some(&mut p)).unwrap();
        std::ptr::write_bytes(p.cast::<u8>(), 0, 65536);
        for (i, v) in [0.1f32.to_bits(), 0.1777f32.to_bits(), 3, 0, 10, 6, 80, 45].iter().enumerate() { p.cast::<u32>().add(i).write(*v) }
        let mut q = std::ptr::null_mut(); rootcb.Map(0, None, Some(&mut q)).unwrap();
        std::ptr::write_bytes(q.cast::<u8>(), 0, 256);
        let base = 200u32; // tables : index 200 ; CBV : index 178
        q.cast::<u32>().add(4).write(0); q.cast::<u32>().add(5).write(base * 32);
        let kind = D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV;
        let dh: ID3D12DescriptorHeap = device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC { Type: kind, NumDescriptors: 1024, Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, NodeMask: 0 }).unwrap();
        let step = device.GetDescriptorHandleIncrementSize(kind) as usize;
        let at = |i: u32| D3D12_CPU_DESCRIPTOR_HANDLE { ptr: dh.GetCPUDescriptorHandleForHeapStart().ptr + i as usize * step };
        device.CreateConstantBufferView(Some(&D3D12_CONSTANT_BUFFER_VIEW_DESC { BufferLocation: cb.GetGPUVirtualAddress(), SizeInBytes: 65536 }), at(178));
        device.CreateShaderResourceView(&t0, None, at(base)); device.CreateShaderResourceView(&t1, None, at(base + 1));
        device.CreateUnorderedAccessView(&args, None, Some(&D3D12_UNORDERED_ACCESS_VIEW_DESC { Format: DXGI_FORMAT_R32_TYPELESS, ViewDimension: D3D12_UAV_DIMENSION_BUFFER, Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 { Buffer: D3D12_BUFFER_UAV { FirstElement: 0, NumElements: 480, Flags: D3D12_BUFFER_UAV_FLAG_RAW, ..Default::default() } } }), at(base + 2));
        device.CreateUnorderedAccessView(&o0, None, None, at(base + 3)); device.CreateUnorderedAccessView(&o1, None, None, at(base + 4));
        list.SetDescriptorHeaps(&[Some(dh.clone())]);
        list.SetComputeRootSignature(&root); list.SetPipelineState(&pso);
        list.SetComputeRootConstantBufferView(0, rootcb.GetGPUVirtualAddress());
        list.Dispatch(10, 6, 1);
        list.ResourceBarrier(&[barrier(&args, uav, cs), barrier(&o0, uav, cs), barrier(&o1, uav, cs)]);
        let rb = D3D12_HEAP_TYPE_READBACK;
        let ra = buffer(1920, rb, none, cd); list.CopyBufferRegion(&ra, 0, &args, 0, 1920);
        let mut outs = Vec::new();
        for t in [&o0, &o1] {
            let b = buffer(256 * 6, rb, none, cd);
            let src = D3D12_TEXTURE_COPY_LOCATION { pResource: std::mem::transmute_copy(t), Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX, Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 } };
            let dst = D3D12_TEXTURE_COPY_LOCATION { pResource: std::mem::transmute_copy(&b), Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT { Offset: 0, Footprint: D3D12_SUBRESOURCE_FOOTPRINT { Format: t.GetDesc().Format, Width: 10, Height: 6, Depth: 1, RowPitch: 256 } } } };
            list.CopyTextureRegion(&dst, 0, 0, 0, &src, None); outs.push(b);
        }
        list.Close().unwrap();
        queue.ExecuteCommandLists(&[Some(list.cast().unwrap())]);
        let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
        queue.Signal(&fence, 1).unwrap();
        for _ in 0..500 { if fence.GetCompletedValue() >= 1 { break } std::thread::sleep(std::time::Duration::from_millis(10)); }
        println!("fence {}", fence.GetCompletedValue());
        let read = |b: &ID3D12Resource, n: usize| { let mut p = std::ptr::null_mut(); b.Map(0, None, Some(&mut p)).unwrap(); std::slice::from_raw_parts(p.cast::<u8>(), n).to_vec() };
        let a = read(&ra, 1920);
        let inst: Vec<u32> = (0..120).map(|i| u32::from_le_bytes(a[i * 16 + 4..i * 16 + 8].try_into().unwrap())).collect();
        println!("instances non nulles : {:?}", inst.iter().enumerate().filter(|(_, v)| **v != 0).collect::<Vec<_>>());
        let b0 = read(&outs[0], 1536); let b1 = read(&outs[1], 1536);
        println!("masque32 (attendu {v1:#x}) : {:x?}", (0..10).map(|x| u32::from_le_bytes(b0[x * 4..x * 4 + 4].try_into().unwrap())).collect::<Vec<_>>());
        println!("masque8  (attendu {v0:#x}) : {:x?}", &b1[..10]);
    }
}
