//! 백엔드 ③ Direct3D 12 직접: D3D11 백엔드와 같은 그리기(인스턴스 하나 = 사각형 하나, 그리기 호출 하나)를 명령 목록으로.
//! 프레임마다 할당자·인스턴스 버퍼를 백버퍼 수만큼 돌려 쓰고, 펜스로 GPU가 끝난 것을 확인한 뒤 재사용한다.

use std::mem::ManuallyDrop;

use windows::core::{s, Interface};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, RECT};
use windows::Win32::Graphics::Direct3D::{D3D_FEATURE_LEVEL_11_0, D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R32G32B32A32_FLOAT, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};

use crate::common::{Sprite, ATLAS};
use crate::shader;
use crate::swap::{PresentInfo, Swap};

pub const MAX: usize = 100_000;
const N: usize = 2;

pub struct D3D12 {
    swap: Swap,
    queue: ID3D12CommandQueue,
    alloc: [ID3D12CommandAllocator; N],
    list: ID3D12GraphicsCommandList,
    rtv_heap: ID3D12DescriptorHeap,
    rtv_step: usize,
    back: [ID3D12Resource; N],
    srv_heap: ID3D12DescriptorHeap,
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
    inst: [ID3D12Resource; N],
    inst_ptr: [*mut Sprite; N],
    fence: ID3D12Fence,
    fence_val: [u64; N],
    next_val: u64,
    event: HANDLE,
    _atlas: ID3D12Resource,
    w: u32,
    h: u32,
}

fn heap(t: D3D12_HEAP_TYPE) -> D3D12_HEAP_PROPERTIES {
    D3D12_HEAP_PROPERTIES { Type: t, ..Default::default() }
}

fn buffer_desc(bytes: u64) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: bytes,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    }
}

fn barrier(res: &ID3D12Resource, before: D3D12_RESOURCE_STATES, after: D3D12_RESOURCE_STATES) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: unsafe { std::mem::transmute_copy(res) },
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

impl D3D12 {
    pub fn new(hwnd: HWND, w: u32, h: u32, atlas_px: &[u8], tearing: bool) -> windows::core::Result<D3D12> {
        unsafe {
            let mut dev: Option<ID3D12Device> = None;
            D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut dev)?;
            let dev = dev.unwrap();
            let queue: ID3D12CommandQueue = dev.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() })?;
            let swap = Swap::new(&queue.cast()?, hwnd, w, h, N as u32, tearing)?;

            let rtv_heap: ID3D12DescriptorHeap = dev.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV, NumDescriptors: N as u32, ..Default::default() })?;
            let rtv_step = dev.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV) as usize;
            let back: [ID3D12Resource; N] = std::array::from_fn(|i| swap.swap.GetBuffer(i as u32).unwrap());
            for (i, b) in back.iter().enumerate() {
                let hnd = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: rtv_heap.GetCPUDescriptorHandleForHeapStart().ptr + i * rtv_step };
                dev.CreateRenderTargetView(b, None, hnd);
            }

            // 루트 서명: b0 = 상수 4개(화면 크기 역수), t0 = 아틀라스, s0 = 고정 샘플러
            let range = D3D12_DESCRIPTOR_RANGE { RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV, NumDescriptors: 1, ..Default::default() };
            let params = [
                D3D12_ROOT_PARAMETER {
                    ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                    Anonymous: D3D12_ROOT_PARAMETER_0 { Constants: D3D12_ROOT_CONSTANTS { ShaderRegister: 0, RegisterSpace: 0, Num32BitValues: 4 } },
                    ShaderVisibility: D3D12_SHADER_VISIBILITY_VERTEX,
                },
                D3D12_ROOT_PARAMETER {
                    ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                    Anonymous: D3D12_ROOT_PARAMETER_0 { DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE { NumDescriptorRanges: 1, pDescriptorRanges: &range } },
                    ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
                },
            ];
            let sampler = D3D12_STATIC_SAMPLER_DESC {
                Filter: D3D12_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
                AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
                AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
                MaxLOD: f32::MAX,
                ShaderRegister: 0,
                ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
                ..Default::default()
            };
            let rsd = D3D12_ROOT_SIGNATURE_DESC {
                NumParameters: params.len() as u32,
                pParameters: params.as_ptr(),
                NumStaticSamplers: 1,
                pStaticSamplers: &sampler,
                Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
            };
            let mut blob = None;
            D3D12SerializeRootSignature(&rsd, D3D_ROOT_SIGNATURE_VERSION_1, &mut blob, None)?;
            let root: ID3D12RootSignature = dev.CreateRootSignature(0, shader::bytes(&blob.unwrap()))?;

            let vsb = shader::compile(s!("vs"), s!("vs_5_0"))?;
            let psb = shader::compile(s!("ps"), s!("ps_5_0"))?;
            let el = |name: windows::core::PCSTR, off: u32| D3D12_INPUT_ELEMENT_DESC {
                SemanticName: name,
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: off,
                InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_INSTANCE_DATA,
                InstanceDataStepRate: 1,
            };
            let elems = [el(s!("RECT"), 0), el(s!("UVRECT"), 16), el(s!("COLOR"), 32)];
            let mut blend = D3D12_BLEND_DESC::default();
            blend.RenderTarget[0] = D3D12_RENDER_TARGET_BLEND_DESC {
                BlendEnable: true.into(),
                LogicOpEnable: false.into(),
                SrcBlend: D3D12_BLEND_ONE,
                DestBlend: D3D12_BLEND_INV_SRC_ALPHA,
                BlendOp: D3D12_BLEND_OP_ADD,
                SrcBlendAlpha: D3D12_BLEND_ONE,
                DestBlendAlpha: D3D12_BLEND_INV_SRC_ALPHA,
                BlendOpAlpha: D3D12_BLEND_OP_ADD,
                LogicOp: D3D12_LOGIC_OP_NOOP,
                RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
            };
            let mut pd = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
                pRootSignature: std::mem::transmute_copy(&root),
                VS: D3D12_SHADER_BYTECODE { pShaderBytecode: vsb.GetBufferPointer(), BytecodeLength: vsb.GetBufferSize() },
                PS: D3D12_SHADER_BYTECODE { pShaderBytecode: psb.GetBufferPointer(), BytecodeLength: psb.GetBufferSize() },
                BlendState: blend,
                SampleMask: u32::MAX,
                RasterizerState: D3D12_RASTERIZER_DESC { FillMode: D3D12_FILL_MODE_SOLID, CullMode: D3D12_CULL_MODE_NONE, DepthClipEnable: true.into(), ..Default::default() },
                InputLayout: D3D12_INPUT_LAYOUT_DESC { pInputElementDescs: elems.as_ptr(), NumElements: elems.len() as u32 },
                PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
                NumRenderTargets: 1,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                ..Default::default()
            };
            pd.RTVFormats[0] = DXGI_FORMAT_B8G8R8A8_UNORM;
            let pso: ID3D12PipelineState = dev.CreateGraphicsPipelineState(&pd)?;
            let _ = ManuallyDrop::into_inner(std::mem::take(&mut pd.pRootSignature));

            // 인스턴스 버퍼: 업로드 힙, 계속 매핑해 둠
            let mut inst_ptr = [std::ptr::null_mut(); N];
            let inst: [ID3D12Resource; N] = std::array::from_fn(|i| {
                let mut r: Option<ID3D12Resource> = None;
                dev.CreateCommittedResource(&heap(D3D12_HEAP_TYPE_UPLOAD), D3D12_HEAP_FLAG_NONE, &buffer_desc((MAX * std::mem::size_of::<Sprite>()) as u64), D3D12_RESOURCE_STATE_GENERIC_READ, None, &mut r).unwrap();
                let r = r.unwrap();
                let mut p = std::ptr::null_mut();
                r.Map(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }), Some(&mut p)).unwrap();
                inst_ptr[i] = p as *mut Sprite;
                r
            });

            let alloc: [ID3D12CommandAllocator; N] = std::array::from_fn(|_| dev.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).unwrap());
            let list: ID3D12GraphicsCommandList = dev.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &alloc[0], &pso)?;
            let fence: ID3D12Fence = dev.CreateFence(0, D3D12_FENCE_FLAG_NONE)?;
            let event = CreateEventW(None, false, false, None)?;

            // 아틀라스: 기본 힙 텍스처 + 업로드 힙에서 복사
            let td = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Width: ATLAS as u64,
                Height: ATLAS,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                ..Default::default()
            };
            let mut atlas: Option<ID3D12Resource> = None;
            dev.CreateCommittedResource(&heap(D3D12_HEAP_TYPE_DEFAULT), D3D12_HEAP_FLAG_NONE, &td, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut atlas)?;
            let atlas = atlas.unwrap();
            let mut fp = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
            let mut total = 0u64;
            dev.GetCopyableFootprints(&td, 0, 1, 0, Some(&mut fp), None, None, Some(&mut total));
            let mut up: Option<ID3D12Resource> = None;
            dev.CreateCommittedResource(&heap(D3D12_HEAP_TYPE_UPLOAD), D3D12_HEAP_FLAG_NONE, &buffer_desc(total), D3D12_RESOURCE_STATE_GENERIC_READ, None, &mut up)?;
            let up = up.unwrap();
            let mut p = std::ptr::null_mut();
            up.Map(0, None, Some(&mut p))?;
            let pitch = fp.Footprint.RowPitch as usize;
            for y in 0..ATLAS as usize {
                let row = ATLAS as usize * 4;
                std::ptr::copy_nonoverlapping(atlas_px.as_ptr().add(y * row), (p as *mut u8).add(fp.Offset as usize + y * pitch), row);
            }
            up.Unmap(0, None);
            let dst = D3D12_TEXTURE_COPY_LOCATION { pResource: std::mem::transmute_copy(&atlas), Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX, Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 } };
            let src = D3D12_TEXTURE_COPY_LOCATION { pResource: std::mem::transmute_copy(&up), Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: fp } };
            list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
            list.ResourceBarrier(&[barrier(&atlas, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE)]);
            list.Close()?;
            queue.ExecuteCommandLists(&[Some(list.cast()?)]);
            queue.Signal(&fence, 1)?;
            fence.SetEventOnCompletion(1, event)?;
            WaitForSingleObject(event, INFINITE);

            let srv_heap: ID3D12DescriptorHeap = dev.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                NumDescriptors: 1,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                ..Default::default()
            })?;
            dev.CreateShaderResourceView(&atlas, None, srv_heap.GetCPUDescriptorHandleForHeapStart());

            Ok(D3D12 {
                swap,
                queue,
                alloc,
                list,
                rtv_heap,
                rtv_step,
                back,
                srv_heap,
                root,
                pso,
                inst,
                inst_ptr,
                fence,
                fence_val: [1; N],
                next_val: 2,
                event,
                _atlas: atlas,
                w,
                h,
            })
        }
    }

    pub fn wait(&self) {
        self.swap.wait();
    }

    pub fn tearing(&self) -> bool {
        self.swap.tearing
    }

    pub fn frame(&mut self, sprites: &[Sprite], vsync: bool) -> windows::core::Result<PresentInfo> {
        let n = sprites.len().min(MAX);
        unsafe {
            let i = self.swap.swap.GetCurrentBackBufferIndex() as usize;
            if self.fence.GetCompletedValue() < self.fence_val[i] {
                self.fence.SetEventOnCompletion(self.fence_val[i], self.event)?;
                WaitForSingleObject(self.event, INFINITE);
            }
            std::ptr::copy_nonoverlapping(sprites.as_ptr(), self.inst_ptr[i], n);
            self.alloc[i].Reset()?;
            let l = &self.list;
            l.Reset(&self.alloc[i], &self.pso)?;
            l.SetGraphicsRootSignature(&self.root);
            l.SetDescriptorHeaps(&[Some(self.srv_heap.clone())]);
            let k = [2.0 / self.w as f32, 2.0 / self.h as f32, 0.0, 0.0];
            l.SetGraphicsRoot32BitConstants(0, 4, k.as_ptr() as *const _, 0);
            l.SetGraphicsRootDescriptorTable(1, self.srv_heap.GetGPUDescriptorHandleForHeapStart());
            l.RSSetViewports(&[D3D12_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: self.w as f32, Height: self.h as f32, MinDepth: 0.0, MaxDepth: 1.0 }]);
            l.RSSetScissorRects(&[RECT { left: 0, top: 0, right: self.w as i32, bottom: self.h as i32 }]);
            l.ResourceBarrier(&[barrier(&self.back[i], D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_RENDER_TARGET)]);
            let rtv = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: self.rtv_heap.GetCPUDescriptorHandleForHeapStart().ptr + i * self.rtv_step };
            l.ClearRenderTargetView(rtv, &[0.04, 0.04, 0.06, 1.0], None);
            l.OMSetRenderTargets(1, Some(&rtv), false, None);
            l.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            let stride = std::mem::size_of::<Sprite>() as u32;
            l.IASetVertexBuffers(0, Some(&[D3D12_VERTEX_BUFFER_VIEW { BufferLocation: self.inst[i].GetGPUVirtualAddress(), SizeInBytes: (n as u32).max(1) * stride, StrideInBytes: stride }]));
            l.DrawInstanced(4, n as u32, 0, 0);
            l.ResourceBarrier(&[barrier(&self.back[i], D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_PRESENT)]);
            l.Close()?;
            self.queue.ExecuteCommandLists(&[Some(l.cast()?)]);
            let info = self.swap.present(vsync);
            self.queue.Signal(&self.fence, self.next_val)?;
            self.fence_val[i] = self.next_val;
            self.next_val += 1;
            Ok(info)
        }
    }
}

impl Drop for D3D12 {
    fn drop(&mut self) {
        unsafe {
            let v = self.next_val;
            if self.queue.Signal(&self.fence, v).is_ok() && self.fence.SetEventOnCompletion(v, self.event).is_ok() {
                WaitForSingleObject(self.event, 2000);
            }
            let _ = CloseHandle(self.event);
        }
    }
}
