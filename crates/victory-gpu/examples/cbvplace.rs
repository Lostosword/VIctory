//! Constant buffers at addresses that are not on 256 bytes: what the host reads there.
#[cfg(not(windows))]
fn main() {}
#[cfg(windows)]
fn main() {
    use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
    use windows::Win32::Graphics::Direct3D::*;
    use windows::Win32::Graphics::Direct3D12::*;
    use windows::Win32::Graphics::Dxgi::Common::*;
    use windows::core::{Interface, s};
    const SOURCE: &str = "cbuffer C : register(b0) { uint4 v[8]; }; RWByteAddressBuffer o : register(u0); cbuffer K : register(b1) { uint at; }; \
        [numthreads(1,1,1)] void main(uint3 id : SV_DispatchThreadID) { for (uint i = 0; i < 8; i++) o.Store4(at + i * 16, v[i]); }";
    unsafe {
        let mut device: Option<ID3D12Device> = None;
        D3D12CreateDevice(None, D3D_FEATURE_LEVEL_12_0, &mut device).unwrap();
        let device = device.unwrap();
        let queue: ID3D12CommandQueue = device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC::default()).unwrap();
        let alloc: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).unwrap();
        let list: ID3D12GraphicsCommandList = device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &alloc, None).unwrap();
        let mut code = None;
        let mut errors = None;
        let compiled = D3DCompile(SOURCE.as_ptr().cast(), SOURCE.len(), None, None, None, s!("main"), s!("cs_5_1"), 0, 0, &mut code, Some(&mut errors));
        if compiled.is_err() {
            let e: ID3DBlob = errors.unwrap();
            println!("compile: {}", String::from_utf8_lossy(std::slice::from_raw_parts(e.GetBufferPointer().cast::<u8>(), e.GetBufferSize())));
            return;
        }
        let code: ID3DBlob = code.unwrap();
        let range = [D3D12_DESCRIPTOR_RANGE { RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_CBV, NumDescriptors: 1, BaseShaderRegister: 0, RegisterSpace: 0, OffsetInDescriptorsFromTableStart: 0 }];
        for root_view in [false, true] {
            let params = [
                if root_view {
                    D3D12_ROOT_PARAMETER { ParameterType: D3D12_ROOT_PARAMETER_TYPE_CBV, Anonymous: D3D12_ROOT_PARAMETER_0 { Descriptor: D3D12_ROOT_DESCRIPTOR { ShaderRegister: 0, RegisterSpace: 0 } }, ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL }
                } else {
                    D3D12_ROOT_PARAMETER { ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE, Anonymous: D3D12_ROOT_PARAMETER_0 { DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE { NumDescriptorRanges: 1, pDescriptorRanges: range.as_ptr() } }, ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL }
                },
                D3D12_ROOT_PARAMETER { ParameterType: D3D12_ROOT_PARAMETER_TYPE_UAV, Anonymous: D3D12_ROOT_PARAMETER_0 { Descriptor: D3D12_ROOT_DESCRIPTOR { ShaderRegister: 0, RegisterSpace: 0 } }, ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL },
                D3D12_ROOT_PARAMETER { ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS, Anonymous: D3D12_ROOT_PARAMETER_0 { Constants: D3D12_ROOT_CONSTANTS { ShaderRegister: 1, RegisterSpace: 0, Num32BitValues: 1 } }, ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL },
            ];
            let desc = D3D12_ROOT_SIGNATURE_DESC { NumParameters: 3, pParameters: params.as_ptr(), ..Default::default() };
            let mut blob = None;
            D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &mut blob, None).unwrap();
            let blob: ID3DBlob = blob.unwrap();
            let root: ID3D12RootSignature = device.CreateRootSignature(0, std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())).unwrap();
            let pso: ID3D12PipelineState = device
                .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                    pRootSignature: std::mem::transmute_copy(&root),
                    CS: D3D12_SHADER_BYTECODE { pShaderBytecode: code.GetBufferPointer(), BytecodeLength: code.GetBufferSize() },
                    ..Default::default()
                })
                .expect("pso");
            let buffer = |size: u64, t, flags, state| -> ID3D12Resource {
                let d = D3D12_RESOURCE_DESC { Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: size, Height: 1, DepthOrArraySize: 1, MipLevels: 1, SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR, Flags: flags, ..Default::default() };
                let mut r: Option<ID3D12Resource> = None;
                device.CreateCommittedResource(&D3D12_HEAP_PROPERTIES { Type: t, ..Default::default() }, D3D12_HEAP_FLAG_NONE, &d, state, None, &mut r).unwrap();
                r.unwrap()
            };
            let upload = buffer(65536, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_GENERIC_READ);
            let out = buffer(4096, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
            let back = buffer(4096, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COPY_DEST);
            let mut p = std::ptr::null_mut();
            upload.Map(0, None, Some(&mut p)).unwrap();
            for i in 0..16384u32 {
                p.cast::<u32>().add(i as usize).write(i * 4);
            }
            let heap: ID3D12DescriptorHeap = device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, NumDescriptors: 16, Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, NodeMask: 0 }).unwrap();
            let step = device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV) as usize;
            let offsets = [0u64, 16, 48, 112, 272, 1040, 4, 256];
            list.SetDescriptorHeaps(&[Some(heap.clone())]);
            list.SetPipelineState(&pso);
            list.SetComputeRootSignature(&root);
            list.SetComputeRootUnorderedAccessView(1, out.GetGPUVirtualAddress());
            for (i, &o) in offsets.iter().enumerate() {
                let at = upload.GetGPUVirtualAddress() + o;
                if root_view {
                    list.SetComputeRootConstantBufferView(0, at);
                } else {
                    let cpu = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: heap.GetCPUDescriptorHandleForHeapStart().ptr + i * step };
                    // (a size of the shader's own bytes, on 16, where the rule asks 256)
                    device.CreateConstantBufferView(Some(&D3D12_CONSTANT_BUFFER_VIEW_DESC { BufferLocation: at, SizeInBytes: if i % 2 == 0 { 256 } else { 128 } }), cpu);
                    let gpu = D3D12_GPU_DESCRIPTOR_HANDLE { ptr: heap.GetGPUDescriptorHandleForHeapStart().ptr + (i * step) as u64 };
                    list.SetComputeRootDescriptorTable(0, gpu);
                }
                list.SetComputeRoot32BitConstant(2, i as u32 * 128, 0);
                list.Dispatch(1, 1, 1);
            }
            let barrier = D3D12_RESOURCE_BARRIER { Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION, Anonymous: D3D12_RESOURCE_BARRIER_0 { Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER { pResource: std::mem::transmute_copy(&out), Subresource: 0, StateBefore: D3D12_RESOURCE_STATE_UNORDERED_ACCESS, StateAfter: D3D12_RESOURCE_STATE_COPY_SOURCE }) }, ..Default::default() };
            list.ResourceBarrier(&[barrier]);
            list.CopyBufferRegion(&back, 0, &out, 0, 4096);
            list.Close().unwrap();
            queue.ExecuteCommandLists(&[Some(list.cast().unwrap())]);
            let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
            queue.Signal(&fence, 1).unwrap();
            while fence.GetCompletedValue() < 1 {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let mut p = std::ptr::null_mut();
            back.Map(0, None, Some(&mut p)).unwrap();
            for (i, &o) in offsets.iter().enumerate() {
                let w: Vec<u32> = (0..32).map(|k| p.cast::<u32>().add(i * 32 + k).read()).collect();
                let good = w.iter().enumerate().all(|(k, &v)| u64::from(v) == o + k as u64 * 4);
                println!("{} offset {o}: {} first {:?}", if root_view { "root" } else { "table" }, if good { "EXACT" } else { "WRONG" }, &w[..4]);
            }
            alloc.Reset().unwrap();
            list.Reset(&alloc, None).unwrap();
        }
    }
}
