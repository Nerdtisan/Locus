// SPDX-License-Identifier: AGPL-3.0-or-later
//! Renderer C: a wgpu paint callback drawing instanced, shader-anti-aliased
//! segments inside egui's own render pass (SPEC §12.2).
//!
//! Lines live on the GPU as `f32` offsets from an *anchor* point. A frame that
//! only pans or zooms uploads nothing but a 48-byte uniform. `f32` cannot hold
//! a 2 m coordinate to 0.001 mm-view precision, so the buffer is re-uploaded
//! relative to a new anchor (the view centre) whenever the f32 error at the
//! view centre would exceed a tenth of a pixel, or the drawing changes.

use crate::doc::Doc;
use crate::view::{View, V2};
use eframe::egui_wgpu::{self, wgpu};
use std::sync::Arc;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Instance {
    a: [f32; 2],
    b: [f32; 2],
    color: u32,
    id: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    center_rel: [f32; 2],
    px_per_mm: f32,
    half_width: f32,
    viewport: [f32; 2],
    hover: i32,
    selected: i32,
    srgb_target: u32,
    hover_rgb: u32,
    _pad: [u32; 2],
}

/// GPU objects, stored in egui-wgpu's callback resources.
struct Resources {
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    instance_buf: wgpu::Buffer,
    capacity: u64,
    count: u32,
    srgb_target: bool,
}

pub fn init(rs: &egui_wgpu::RenderState, msaa: u32) {
    let device = &rs.device;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("spike lines"),
        source: wgpu::ShaderSource::Wgsl(include_str!("line.wgsl").into()),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("spike lines"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("spike lines"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("spike lines"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Instance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![
                    0 => Float32x2, 1 => Float32x2, 2 => Uint32, 3 => Uint32
                ],
            }],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState {
            count: msaa.max(1),
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: rs.target_format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("spike lines uniforms"),
        size: std::mem::size_of::<Uniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("spike lines"),
        layout: &bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buf.as_entire_binding(),
        }],
    });
    let capacity = 1024 * std::mem::size_of::<Instance>() as u64;
    let instance_buf = new_instance_buf(device, capacity);
    rs.renderer.write().callback_resources.insert(Resources {
        pipeline,
        uniform_buf,
        bind_group,
        instance_buf,
        capacity,
        count: 0,
        srgb_target: rs.target_format.is_srgb(),
    });
}

fn new_instance_buf(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("spike lines instances"),
        size,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// CPU-side bookkeeping: what is on the GPU and relative to which anchor.
#[derive(Debug, Default)]
pub struct GpuLines {
    uploaded_rev: Option<u64>,
    anchor: V2,
    pub uploads: u64,
}

impl GpuLines {
    fn needs_upload(&self, doc: &Doc, view: &View, ppp: f32) -> bool {
        if self.uploaded_rev != Some(doc.revision) {
            return true;
        }
        // f32 rounding error at the view centre, in physical pixels.
        let off = view.center.sub(self.anchor).len();
        off * view.pt_per_mm * ppp as f64 * f32::EPSILON as f64 > 0.1
    }

    /// Queue the paint callback. Returns how many instances were (re)built on
    /// the CPU this frame (0 for a pure pan/zoom).
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &mut self,
        painter: &egui::Painter,
        doc: &Doc,
        view: &View,
        rect: egui::Rect,
        ppp: f32,
        hover: Option<usize>,
        selected: Option<usize>,
        hover_color: egui::Color32,
    ) -> usize {
        let mut instances = None;
        if self.needs_upload(doc, view, ppp) {
            self.anchor = view.center;
            let list: Vec<Instance> = doc
                .lines
                .iter()
                .enumerate()
                .filter(|(_, l)| doc.is_visible(l))
                .map(|(i, l)| {
                    let a = l.a.sub(self.anchor);
                    let b = l.b.sub(self.anchor);
                    let c = doc.color(l);
                    Instance {
                        a: [a.x as f32, a.y as f32],
                        b: [b.x as f32, b.y as f32],
                        color: u32::from_le_bytes(c.to_array()),
                        id: i as u32,
                    }
                })
                .collect();
            instances = Some(Arc::new(list));
            self.uploaded_rev = Some(doc.revision);
            self.uploads += 1;
        }
        let built = instances.as_ref().map_or(0, |v| v.len());
        let rel = view.center.sub(self.anchor);
        let to_i32 = |o: Option<usize>| o.map_or(-1, |i| i as i32);
        let uniforms = Uniforms {
            center_rel: [rel.x as f32, rel.y as f32],
            px_per_mm: (view.pt_per_mm * ppp as f64) as f32,
            half_width: crate::render::LINE_WIDTH_PT * ppp * 0.5,
            viewport: [rect.width() * ppp, rect.height() * ppp],
            hover: to_i32(hover),
            selected: to_i32(selected),
            srgb_target: 0,
            hover_rgb: u32::from_le_bytes(hover_color.to_array()),
            _pad: [0; 2],
        };
        painter.add(egui_wgpu::Callback::new_paint_callback(
            rect,
            LinesCallback {
                instances,
                uniforms,
            },
        ));
        built
    }
}

struct LinesCallback {
    instances: Option<Arc<Vec<Instance>>>,
    uniforms: Uniforms,
}

impl egui_wgpu::CallbackTrait for LinesCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(res) = resources.get_mut::<Resources>() else {
            return Vec::new();
        };
        if let Some(list) = &self.instances {
            let bytes: &[u8] = bytemuck::cast_slice(list.as_slice());
            if bytes.len() as u64 > res.capacity {
                res.capacity = (bytes.len() as u64).next_power_of_two();
                res.instance_buf = new_instance_buf(device, res.capacity);
            }
            if !bytes.is_empty() {
                queue.write_buffer(&res.instance_buf, 0, bytes);
            }
            res.count = list.len() as u32;
        }
        let mut u = self.uniforms;
        u.srgb_target = u32::from(res.srgb_target);
        queue.write_buffer(&res.uniform_buf, 0, bytemuck::bytes_of(&u));
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(res) = resources.get::<Resources>() else {
            return;
        };
        if res.count == 0 {
            return;
        }
        let bytes = res.count as u64 * std::mem::size_of::<Instance>() as u64;
        pass.set_pipeline(&res.pipeline);
        pass.set_bind_group(0, &res.bind_group, &[]);
        pass.set_vertex_buffer(0, res.instance_buf.slice(..bytes));
        pass.draw(0..6, 0..res.count);
    }
}
