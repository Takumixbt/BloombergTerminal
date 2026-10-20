//! Offscreen rendering without a window: used by pixel tests and the frame-time benchmark.

use std::time::{Duration, Instant};

use egui_wgpu::wgpu;

use crate::gpu::{ChartRenderer, Frame};

pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub struct Headless {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub renderer: ChartRenderer,
    pub adapter_info: wgpu::AdapterInfo,
    target: wgpu::Texture,
    view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    frame_no: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct FrameTiming {
    /// CPU time to upload uniforms/data and record commands.
    pub cpu: Duration,
    /// Wall time from start until the GPU finished the frame.
    pub total: Duration,
}

impl Headless {
    /// Returns `None` when no adapter (not even a software one) is available.
    pub fn new(width: u32, height: u32) -> Option<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            ..Default::default()
        }))
        .ok()?;
        let adapter_info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("headless"),
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .ok()?;
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("headless-target"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let renderer = ChartRenderer::new(&device, FORMAT, 1);
        Some(Self { device, queue, renderer, adapter_info, target, view, width, height, frame_no: 0 })
    }

    /// Render `frames` (each a pane filling the target) and wait for the GPU.
    pub fn render(&mut self, frames: &mut [Frame]) -> FrameTiming {
        let start = Instant::now();
        self.frame_no += 1;
        for f in frames.iter_mut() {
            f.frame_no = self.frame_no;
            self.renderer.prepare(&self.device, &self.queue, f);
        }
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("headless-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            for f in frames.iter() {
                self.renderer.paint(f.pane, &mut pass);
            }
        }
        self.queue.submit([encoder.finish()]);
        let cpu = start.elapsed();
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        FrameTiming { cpu, total: start.elapsed() }
    }

    /// Read the target back as tightly packed RGBA8 rows.
    pub fn read_pixels(&self) -> Vec<u8> {
        let row = self.width * 4;
        let padded = row.div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (padded * self.height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: None },
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        buf.map_async(wgpu::MapMode::Read, .., |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let data = buf.slice(..).get_mapped_range().expect("mapped readback buffer");
        let mut out = Vec::with_capacity((row * self.height) as usize);
        for y in 0..self.height {
            let s = (y * padded) as usize;
            out.extend_from_slice(&data[s..s + row as usize]);
        }
        out
    }
}
