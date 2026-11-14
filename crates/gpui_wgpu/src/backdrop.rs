//! Liquid-glass backdrops. Glass copies and refracts what is already drawn, so
//! the main pass must end before its pixels can be read. Backdrops are grouped
//! so one break serves every glass whose source is final by then; the group's
//! copies and blurs run together in a shared atlas, and each glass is then drawn
//! inline when the main pass resumes. Without a copyable surface the frame
//! renders offscreen and is blitted.

use collections::FxHashMap;
use gpui::{
    Backdrop, BackdropKind, Bounds, DevicePixels, PrimitiveBatch, ScaledPixels, Scene, Size,
};

/// Per-draw uniform slot; a multiple of the 256-byte dynamic offset alignment.
const UNIFORM_STRIDE: u64 = 512;
const UNIFORM_SIZE: u64 = std::mem::size_of::<Uniforms>() as u64;

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    surface0: [f32; 4],
    surface1: [f32; 4],
    highlight: [f32; 4],
    glow: [f32; 4],
    shadow: [f32; 4],
    inner_shadow: [f32; 4],
    radial_glow: [f32; 4],
    viewport: [f32; 2],
    draw_origin: [f32; 2],
    draw_size: [f32; 2],
    clip_origin: [f32; 2],
    clip_size: [f32; 2],
    shape_origin: [f32; 2],
    shape_size: [f32; 2],
    region_offset: [f32; 2],
    region_size: [f32; 2],
    source_texture_size: [f32; 2],
    restore_origin: [f32; 2],
    restore_texture_size: [f32; 2],
    layer_scale: [f32; 2],
    content_offset: [f32; 2],
    /// Where the region's texel (0, 0) sits in the shared blur atlas.
    atlas_origin: [f32; 2],
    radius: f32,
    padding: f32,
    saturation: f32,
    refraction_height: f32,
    refraction_amount: f32,
    depth_effect: f32,
    chromatic_aberration: f32,
    restore: f32,
    blur_sigma: f32,
    radial_glow_edge: f32,
    opacity: f32,
    downscale: f32,
    scale_content: f32,
    _pad: [f32; 5],
}

const _: () = assert!(std::mem::size_of::<Uniforms>() == 304);

/// Like Skia, large blurs run on a halved image until sigma is at most this.
const MAX_BLUR_SIGMA: f32 = 4.0;
const MAX_DOWNSAMPLES: u32 = 6;
/// Downsample, blur and glass draws one backdrop may need.
const PASSES_PER_BACKDROP: u64 = 3;

/// Skia's conversion from an Android blur radius to a gaussian sigma.
fn radius_to_sigma(radius: f32) -> f32 {
    if radius > 0.0 {
        0.57735 * radius + 0.5
    } else {
        0.0
    }
}

struct Texture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
}

impl Texture {
    fn new(
        device: &wgpu::Device,
        label: &str,
        format: wgpu::TextureFormat,
        size: (u32, u32),
        usage: wgpu::TextureUsages,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size.0.max(1),
                height: size.1.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            size: (size.0.max(1), size.1.max(1)),
        }
    }

    fn extent(&self) -> [f32; 2] {
        [self.size.0 as f32, self.size.1 as f32]
    }
}

/// Integer pixel rectangle inside a render target.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl Rect {
    fn from_bounds(bounds: Bounds<ScaledPixels>) -> Self {
        let x0 = bounds.origin.x.0.floor() as i32;
        let y0 = bounds.origin.y.0.floor() as i32;
        let x1 = (bounds.origin.x.0 + bounds.size.width.0).ceil() as i32;
        let y1 = (bounds.origin.y.0 + bounds.size.height.0).ceil() as i32;
        Self {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
    }

    fn intersect(self, other: Rect) -> Rect {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = (self.x + self.w).min(other.x + other.w);
        let y1 = (self.y + self.h).min(other.y + other.h);
        Rect {
            x: x0,
            y: y0,
            w: (x1 - x0).max(0),
            h: (y1 - y0).max(0),
        }
    }

    fn is_empty(self) -> bool {
        self.w <= 0 || self.h <= 0
    }
}

struct Slot {
    texture: Texture,
    rect: Rect,
}

/// One glass of the group being prepared.
struct Job {
    index: usize,
    draw: Rect,
    clip: Rect,
    /// Screen pixels copied as the glass's source; empty when it has none.
    source: Rect,
    /// Top-left of the source in both atlas textures.
    atlas: (i32, i32),
    sigma: f32,
    downsamples: u32,
    /// One downsample pass if needed, then one blur; zero samples the copy directly.
    steps: u32,
}

/// A prepared glass, drawn when the main pass reaches its backdrop.
struct GlassDraw {
    index: usize,
    offset: u32,
    bind_group: wgpu::BindGroup,
}

pub(crate) struct BackdropRenderer {
    format: wgpu::TextureFormat,
    layout: wgpu::BindGroupLayout,
    blur: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    glass: wgpu::RenderPipeline,
    blit: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    uniform_capacity: u64,
    uniform_cursor: u64,
    uniform_data: Vec<u8>,
    placeholder: Texture,
    /// Offscreen frame, used when the surface cannot be a copy source.
    offscreen: Option<Texture>,
    /// The texture this frame draws into: the surface frame or `offscreen`.
    target: Option<Texture>,
    /// Ping-pong blur atlas; every glass of a group owns the same rectangle in both.
    atlas: [Option<Texture>; 2],
    slots: FxHashMap<u8, Slot>,
    group_starts: Vec<usize>,
    dirty: Vec<Rect>,
    jobs: Vec<Job>,
    draws: Vec<GlassDraw>,
}

const ATLAS_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::RENDER_ATTACHMENT
    .union(wgpu::TextureUsages::TEXTURE_BINDING)
    .union(wgpu::TextureUsages::COPY_DST);
const SLOT_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::TEXTURE_BINDING
    .union(wgpu::TextureUsages::COPY_DST)
    .union(wgpu::TextureUsages::COPY_SRC);
const TARGET_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::RENDER_ATTACHMENT
    .union(wgpu::TextureUsages::TEXTURE_BINDING)
    .union(wgpu::TextureUsages::COPY_SRC);

/// Pixels a primitive batch may write; content masks bound every primitive.
fn batch_rects(scene: &Scene, batch: &PrimitiveBatch, out: &mut Vec<Rect>) {
    fn clipped(bounds: Bounds<ScaledPixels>, mask: Bounds<ScaledPixels>) -> Rect {
        Rect::from_bounds(bounds).intersect(Rect::from_bounds(mask))
    }
    match batch {
        PrimitiveBatch::Quads(range) => out.extend(
            scene.quads[range.clone()]
                .iter()
                .map(|q| clipped(q.bounds, q.content_mask.bounds)),
        ),
        PrimitiveBatch::Shadows(range) => {
            out.extend(scene.shadows[range.clone()].iter().map(|s| {
                let margin = s.blur_radius * 3.0;
                let bounds = Bounds {
                    origin: s.bounds.origin - gpui::point(margin, margin),
                    size: s.bounds.size + gpui::size(margin * 2.0, margin * 2.0),
                };
                clipped(bounds, s.content_mask.bounds)
            }))
        }
        PrimitiveBatch::Underlines(range) => out.extend(
            scene.underlines[range.clone()]
                .iter()
                .map(|u| clipped(u.bounds, u.content_mask.bounds)),
        ),
        PrimitiveBatch::Paths(range) => out.extend(
            scene.paths[range.clone()]
                .iter()
                .map(|p| clipped(p.bounds, p.content_mask.bounds)),
        ),
        PrimitiveBatch::MonochromeSprites { range, .. } => {
            out.extend(scene.monochrome_sprites[range.clone()].iter().map(|s| {
                if s.transformation == gpui::TransformationMatrix::unit() {
                    clipped(s.bounds, s.content_mask.bounds)
                } else {
                    Rect::from_bounds(s.content_mask.bounds)
                }
            }))
        }
        PrimitiveBatch::SubpixelSprites { range, .. } => {
            out.extend(scene.subpixel_sprites[range.clone()].iter().map(|s| {
                if s.transformation == gpui::TransformationMatrix::unit() {
                    clipped(s.bounds, s.content_mask.bounds)
                } else {
                    Rect::from_bounds(s.content_mask.bounds)
                }
            }))
        }
        PrimitiveBatch::PolychromeSprites { range, .. } => out.extend(
            scene.polychrome_sprites[range.clone()]
                .iter()
                .map(|s| clipped(s.bounds, s.content_mask.bounds)),
        ),
        PrimitiveBatch::Surfaces(_) | PrimitiveBatch::Backdrops(_) => {}
    }
}

/// The unscaled glass layer on screen: the shape, its padding and its scale.
fn layer_rect(effect: &gpui::BackdropEffect) -> Rect {
    let shape = effect.shape;
    let scale = effect.layer_scale.map(|s| if s > 0.0 { s } else { 1.0 });
    let padding = effect.padding;
    if effect.scale_content {
        // Scaled content samples its source where it sits unscaled.
        return Rect::from_bounds(Bounds {
            origin: gpui::point(
                ScaledPixels(shape.origin.x.0 + effect.content_offset[0] - padding),
                ScaledPixels(shape.origin.y.0 + effect.content_offset[1] - padding),
            ),
            size: gpui::size(
                ScaledPixels(shape.size.width.0 + padding * 2.0),
                ScaledPixels(shape.size.height.0 + padding * 2.0),
            ),
        });
    }
    let center = shape.center();
    let half = [
        (shape.size.width.0 / 2.0 + padding) * scale[0],
        (shape.size.height.0 / 2.0 + padding) * scale[1],
    ];
    Rect::from_bounds(Bounds {
        origin: gpui::point(
            ScaledPixels(center.x.0 - half[0]),
            ScaledPixels(center.y.0 - half[1]),
        ),
        size: gpui::size(ScaledPixels(half[0] * 2.0), ScaledPixels(half[1] * 2.0)),
    })
}

/// Splits a frame's backdrops into groups sharing one break of the main pass,
/// pushing the index of each group's first backdrop to `starts`. A backdrop
/// joins the open group unless something drawn since the group's break
/// overlaps the pixels it reads, or it captures a slot the group already uses.
fn plan_groups(
    scene: &Scene,
    batches: &[PrimitiveBatch],
    screen: Rect,
    starts: &mut Vec<usize>,
    dirty: &mut Vec<Rect>,
) {
    starts.clear();
    dirty.clear();
    let mut used = SlotSet::default();
    for batch in batches {
        let PrimitiveBatch::Backdrops(range) = batch else {
            if !starts.is_empty() {
                batch_rects(scene, batch, dirty);
            }
            continue;
        };
        for index in range.clone() {
            let backdrop = &scene.backdrops[index];
            let effect = &backdrop.effect;
            let clip = Rect::from_bounds(backdrop.content_mask.bounds).intersect(screen);
            let (reads, captures) = match backdrop.kind {
                BackdropKind::Capture => (
                    Some(Rect::from_bounds(backdrop.bounds).intersect(clip)),
                    (effect.slot != 0).then_some(effect.slot),
                ),
                BackdropKind::Glass => (
                    (effect.source == 0).then(|| layer_rect(effect).intersect(screen)),
                    None,
                ),
            };
            let stale = reads.is_some_and(|read| {
                !read.is_empty() && dirty.iter().any(|d| !d.intersect(read).is_empty())
            });
            let recaptured = captures.is_some_and(|slot| used.contains(slot));
            if starts.is_empty() || stale || recaptured {
                starts.push(index);
                dirty.clear();
                used = SlotSet::default();
            }
            match backdrop.kind {
                BackdropKind::Capture => used.insert(effect.slot),
                BackdropKind::Glass => {
                    used.insert(effect.source);
                    used.insert(effect.slot);
                    dirty.push(Rect::from_bounds(backdrop.bounds).intersect(clip));
                }
            }
        }
    }
}

/// Slots as a 256-bit set.
#[derive(Default)]
struct SlotSet([u64; 4]);

impl SlotSet {
    fn insert(&mut self, slot: u8) {
        self.0[usize::from(slot >> 6)] |= 1 << (slot & 63);
    }

    fn contains(&self, slot: u8) -> bool {
        self.0[usize::from(slot >> 6)] & (1 << (slot & 63)) != 0
    }
}

impl BackdropRenderer {
    pub(crate) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        cache: Option<&wgpu::PipelineCache>,
    ) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("backdrop_shaders"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders_backdrop.wgsl").into()),
        });
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("backdrop_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(UNIFORM_SIZE),
                    },
                    count: None,
                },
                texture_entry(1),
                texture_entry(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("backdrop_pipeline_layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_backdrop"),
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache,
            })
        };
        let blur = pipeline("fs_blur", None);
        let down = pipeline("fs_down", None);
        let glass = pipeline(
            "fs_glass",
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        );
        let blit = pipeline("fs_blit", None);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("backdrop_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniform_capacity = UNIFORM_STRIDE * 16;
        let uniforms = Self::uniform_buffer(device, uniform_capacity);
        let placeholder = Texture::new(
            device,
            "backdrop_placeholder",
            format,
            (1, 1),
            wgpu::TextureUsages::TEXTURE_BINDING,
        );
        Self {
            format,
            layout,
            blur,
            down,
            glass,
            blit,
            sampler,
            uniforms,
            uniform_capacity,
            uniform_cursor: 0,
            uniform_data: Vec::new(),
            placeholder,
            offscreen: None,
            target: None,
            atlas: [None, None],
            slots: FxHashMap::default(),
            group_starts: Vec::new(),
            dirty: Vec::new(),
            jobs: Vec::new(),
            draws: Vec::new(),
        }
    }

    fn uniform_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("backdrop_uniforms"),
            size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Prepares uniform space for `count` backdrops. Draws into `direct` when
    /// the frame can be copied from; otherwise returns the offscreen view the
    /// frame must render into before `blit`.
    pub(crate) fn begin_frame(
        &mut self,
        device: &wgpu::Device,
        size: Size<DevicePixels>,
        count: usize,
        direct: Option<(&wgpu::Texture, &wgpu::TextureView)>,
    ) -> Option<wgpu::TextureView> {
        let needed = UNIFORM_STRIDE * (count as u64 * PASSES_PER_BACKDROP + 1);
        if needed > self.uniform_capacity {
            self.uniform_capacity = needed.next_power_of_two();
            self.uniforms = Self::uniform_buffer(device, self.uniform_capacity);
        }
        self.uniform_cursor = 0;
        self.uniform_data.clear();
        for slot in self.slots.values_mut() {
            slot.rect = Rect::default();
        }
        if let Some((texture, view)) = direct {
            self.target = Some(Texture {
                texture: texture.clone(),
                view: view.clone(),
                size: (texture.width(), texture.height()),
            });
            return None;
        }
        let extent = (size.width.0.max(1) as u32, size.height.0.max(1) as u32);
        if self.offscreen.as_ref().map(|t| t.size) != Some(extent) {
            self.offscreen = Some(Texture::new(
                device,
                "backdrop_offscreen",
                self.format,
                extent,
                TARGET_USAGE,
            ));
        }
        let offscreen = self.offscreen.as_ref()?;
        self.target = Some(Texture {
            texture: offscreen.texture.clone(),
            view: offscreen.view.clone(),
            size: offscreen.size,
        });
        Some(offscreen.view.clone())
    }

    fn screen(&self) -> Rect {
        self.target.as_ref().map_or(Rect::default(), |target| Rect {
            x: 0,
            y: 0,
            w: target.size.0 as i32,
            h: target.size.1 as i32,
        })
    }

    pub(crate) fn plan(&mut self, scene: &Scene, batches: &[PrimitiveBatch]) {
        let screen = self.screen();
        plan_groups(
            scene,
            batches,
            screen,
            &mut self.group_starts,
            &mut self.dirty,
        );
    }

    /// The backdrops of the group starting at `index`, if one does.
    pub(crate) fn group_at(&self, index: usize, count: usize) -> Option<std::ops::Range<usize>> {
        let position = self.group_starts.binary_search(&index).ok()?;
        let end = self
            .group_starts
            .get(position + 1)
            .copied()
            .unwrap_or(count);
        Some(index..end)
    }

    fn push_uniforms(&mut self, uniforms: &Uniforms) -> u32 {
        let offset = self.uniform_cursor;
        let end = (offset + UNIFORM_SIZE) as usize;
        self.uniform_data.resize(end, 0);
        self.uniform_data[offset as usize..end].copy_from_slice(bytemuck::bytes_of(uniforms));
        self.uniform_cursor += UNIFORM_STRIDE;
        offset as u32
    }

    pub(crate) fn flush_uniforms(&self, queue: &wgpu::Queue) {
        if !self.uniform_data.is_empty() {
            queue.write_buffer(&self.uniforms, 0, &self.uniform_data);
        }
    }

    fn bind_group(
        &self,
        device: &wgpu::Device,
        source: &wgpu::TextureView,
        restore: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("backdrop_bind_group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.uniforms,
                        offset: 0,
                        size: wgpu::BufferSize::new(UNIFORM_SIZE),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(restore),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    fn grow(
        device: &wgpu::Device,
        slot: &mut Option<Texture>,
        format: wgpu::TextureFormat,
        label: &str,
        size: (u32, u32),
        usage: wgpu::TextureUsages,
    ) {
        let fits = slot
            .as_ref()
            .is_some_and(|t| t.size.0 >= size.0 && t.size.1 >= size.1);
        if !fits {
            let current = slot.as_ref().map_or((0, 0), |t| t.size);
            *slot = Some(Texture::new(
                device,
                label,
                format,
                (current.0.max(size.0), current.1.max(size.1)),
                usage,
            ));
        }
    }

    fn copy(
        encoder: &mut wgpu::CommandEncoder,
        from: &wgpu::Texture,
        from_origin: (i32, i32),
        to: &wgpu::Texture,
        to_origin: (i32, i32),
        size: (i32, i32),
    ) {
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: from,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: from_origin.0 as u32,
                    y: from_origin.1 as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: to,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: to_origin.0 as u32,
                    y: to_origin.1 as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: size.0 as u32,
                height: size.1 as u32,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Runs a group's captures, copies and blurs once the main pass has ended.
    /// Its glass is drawn later by `draw_glass`, inline in the main pass.
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        backdrops: &[Backdrop],
        first: usize,
    ) {
        self.draws.clear();
        let screen = self.screen();
        let Some(target) = self.target.as_ref().map(|t| t.texture.clone()) else {
            return;
        };

        // Captures first: a glass in this group reads a slot only after its capture.
        let mut jobs = std::mem::take(&mut self.jobs);
        jobs.clear();
        let mut shelf = (0, 0, 0); // x, y and height of the current atlas row
        let mut atlas_width = screen.w;
        for (offset, backdrop) in backdrops.iter().enumerate() {
            let effect = &backdrop.effect;
            let clip = Rect::from_bounds(backdrop.content_mask.bounds).intersect(screen);
            match backdrop.kind {
                BackdropKind::Capture => {
                    let rect = Rect::from_bounds(backdrop.bounds).intersect(clip);
                    if rect.is_empty() || effect.slot == 0 {
                        continue;
                    }
                    let mut texture = self.slots.remove(&effect.slot).map(|slot| slot.texture);
                    Self::grow(
                        device,
                        &mut texture,
                        self.format,
                        "backdrop_slot",
                        (rect.w as u32, rect.h as u32),
                        SLOT_USAGE,
                    );
                    let Some(texture) = texture else { continue };
                    Self::copy(
                        encoder,
                        &target,
                        (rect.x, rect.y),
                        &texture.texture,
                        (0, 0),
                        (rect.w, rect.h),
                    );
                    self.slots.insert(effect.slot, Slot { texture, rect });
                }
                BackdropKind::Glass => {
                    let draw = Rect::from_bounds(backdrop.bounds).intersect(clip);
                    if draw.is_empty() {
                        continue;
                    }
                    let layer = layer_rect(effect);
                    let source = if effect.source == 0 {
                        layer.intersect(screen)
                    } else {
                        self.slots
                            .get(&effect.source)
                            .map_or(Rect::default(), |slot| layer.intersect(slot.rect))
                    };
                    let scale = effect.layer_scale.map(|s| if s > 0.0 { s } else { 1.0 });
                    let mut sigma =
                        radius_to_sigma(effect.blur_radius * (scale[0] + scale[1]) / 2.0);
                    let mut downsamples = 0;
                    let mut steps = 0;
                    let mut atlas = (0, 0);
                    if !source.is_empty() {
                        if sigma > 0.0 {
                            let mut size = (source.w, source.h);
                            while sigma > MAX_BLUR_SIGMA
                                && downsamples < MAX_DOWNSAMPLES
                                && size.0.min(size.1) >= 4
                            {
                                size = ((size.0 + 1) / 2, (size.1 + 1) / 2);
                                downsamples += 1;
                                sigma /= 2.0;
                            }
                            steps = u32::from(downsamples > 0) + 1;
                        }
                        atlas_width = atlas_width.max(source.w);
                        if shelf.0 + source.w > atlas_width {
                            shelf = (0, shelf.1 + shelf.2, 0);
                        }
                        atlas = (shelf.0, shelf.1);
                        shelf.0 += source.w;
                        shelf.2 = shelf.2.max(source.h);
                    }
                    jobs.push(Job {
                        index: first + offset,
                        draw,
                        clip,
                        source,
                        atlas,
                        sigma,
                        downsamples,
                        steps,
                    });
                }
            }
        }
        let atlas_height = shelf.1 + shelf.2;
        if atlas_height > 0 {
            for (index, label) in [(0, "backdrop_atlas_a"), (1, "backdrop_atlas_b")] {
                Self::grow(
                    device,
                    &mut self.atlas[index],
                    self.format,
                    label,
                    (atlas_width as u32, atlas_height as u32),
                    ATLAS_USAGE,
                );
            }
        }

        // Every glass ends its passes in atlas[steps_total % 2], so each starts
        // in whichever texture leaves it there after its own steps.
        let total = jobs.iter().map(|job| job.steps).max().unwrap_or(0);
        for job in &jobs {
            if job.source.is_empty() {
                continue;
            }
            let Some(to) = self.atlas[((total - job.steps) % 2) as usize].as_ref() else {
                continue;
            };
            let backdrop = &backdrops[job.index - first];
            let (from, from_origin) = if backdrop.effect.source == 0 {
                (&target, (job.source.x, job.source.y))
            } else {
                let Some(slot) = self.slots.get(&backdrop.effect.source) else {
                    continue;
                };
                (
                    &slot.texture.texture,
                    (job.source.x - slot.rect.x, job.source.y - slot.rect.y),
                )
            };
            Self::copy(
                encoder,
                from,
                from_origin,
                &to.texture,
                job.atlas,
                (job.source.w, job.source.h),
            );
        }

        if total > 0 {
            self.blur_steps(device, encoder, &jobs, total);
        }

        for job in &jobs {
            let backdrop = &backdrops[job.index - first];
            self.prepare_glass(device, backdrop, job, total);
        }
        self.jobs = jobs;
    }

    /// Runs step `s` of every glass in one pass from atlas[s % 2] to the other.
    /// A glass with fewer steps than the group joins at the later ones.
    fn blur_steps(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        jobs: &[Job],
        total: u32,
    ) {
        let (Some(a), Some(b)) = (self.atlas[0].as_ref(), self.atlas[1].as_ref()) else {
            return;
        };
        let bind_groups = [
            self.bind_group(device, &a.view, &self.placeholder.view),
            self.bind_group(device, &b.view, &self.placeholder.view),
        ];
        let views = [a.view.clone(), b.view.clone()];
        let extent = a.extent();
        let mut draws: Vec<(bool, u32)> = Vec::with_capacity(jobs.len());
        for step in 0..total {
            draws.clear();
            for job in jobs {
                let Some(own) = (step + job.steps).checked_sub(total) else {
                    continue;
                };
                let factor = 1 << job.downsamples;
                let small = (
                    (job.source.w + factor - 1) / factor,
                    (job.source.h + factor - 1) / factor,
                );
                let origin = [job.atlas.0 as f32, job.atlas.1 as f32];
                let down = own == 0 && job.downsamples > 0;
                let uniforms = if down {
                    Uniforms {
                        draw_origin: origin,
                        draw_size: [small.0 as f32, small.1 as f32],
                        region_size: [job.source.w as f32, job.source.h as f32],
                        downscale: factor as f32,
                        ..Uniforms::default()
                    }
                } else {
                    Uniforms {
                        draw_origin: origin,
                        draw_size: [small.0 as f32, small.1 as f32],
                        region_size: [small.0 as f32, small.1 as f32],
                        blur_sigma: job.sigma,
                        ..Uniforms::default()
                    }
                };
                let offset = self.push_uniforms(&Uniforms {
                    viewport: extent,
                    source_texture_size: extent,
                    atlas_origin: origin,
                    ..uniforms
                });
                draws.push((down, offset));
            }
            let from = (step % 2) as usize;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("backdrop_blur_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &views[1 - from],
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            for &(down, offset) in &draws {
                pass.set_pipeline(if down { &self.down } else { &self.blur });
                pass.set_bind_group(0, &bind_groups[from], &[offset]);
                pass.draw(0..4, 0..1);
            }
        }
    }

    fn prepare_glass(&mut self, device: &wgpu::Device, backdrop: &Backdrop, job: &Job, total: u32) {
        let Some(target) = self.target.as_ref() else {
            return;
        };
        let effect = &backdrop.effect;
        let shape = effect.shape;
        let scale = effect.layer_scale.map(|s| if s > 0.0 { s } else { 1.0 });
        let source_view = match self.atlas[(total % 2) as usize].as_ref() {
            Some(atlas) if !job.source.is_empty() => atlas,
            _ => &self.placeholder,
        };
        let restore_slot = (effect.slot != 0)
            .then(|| self.slots.get(&effect.slot))
            .flatten();
        let restore_texture = restore_slot.map_or(&self.placeholder, |slot| &slot.texture);
        let min_dimension = shape.size.width.0.min(shape.size.height.0);
        let highlight_half_width = effect.highlight_width.min(min_dimension / 2.0).ceil();
        let uniforms = Uniforms {
            surface0: effect.surface[0],
            surface1: effect.surface[1],
            highlight: [
                effect.highlight_alpha,
                highlight_half_width,
                radius_to_sigma(effect.highlight_blur),
                if effect.highlight_ambient { -1.0 } else { 1.0 },
            ],
            glow: [
                effect.glow,
                effect.glow_position[0],
                effect.glow_position[1],
                0.0,
            ],
            shadow: [
                effect.shadow_alpha,
                radius_to_sigma(effect.shadow_radius),
                effect.shadow_offset,
                effect.shadow_color_alpha,
            ],
            inner_shadow: [
                effect.inner_shadow_alpha,
                radius_to_sigma(effect.inner_shadow_radius),
                effect.inner_shadow_offset,
                effect.inner_shadow_color_alpha,
            ],
            radial_glow: effect.radial_glow,
            radial_glow_edge: effect.radial_glow_edge,
            opacity: effect.opacity,
            viewport: target.extent(),
            draw_origin: [job.draw.x as f32, job.draw.y as f32],
            draw_size: [job.draw.w as f32, job.draw.h as f32],
            clip_origin: [job.clip.x as f32, job.clip.y as f32],
            clip_size: [job.clip.w as f32, job.clip.h as f32],
            shape_origin: [shape.origin.x.0, shape.origin.y.0],
            shape_size: [shape.size.width.0, shape.size.height.0],
            region_offset: [job.source.x as f32, job.source.y as f32],
            layer_scale: scale,
            region_size: [job.source.w.max(0) as f32, job.source.h.max(0) as f32],
            source_texture_size: source_view.extent(),
            atlas_origin: [job.atlas.0 as f32, job.atlas.1 as f32],
            restore_origin: restore_slot
                .map_or([0.0; 2], |slot| [slot.rect.x as f32, slot.rect.y as f32]),
            restore_texture_size: restore_texture.extent(),
            radius: effect.corner_radius.min(min_dimension / 2.0),
            padding: effect.padding,
            saturation: effect.saturation,
            refraction_height: effect.refraction_height,
            refraction_amount: effect.refraction_amount,
            depth_effect: effect.depth_effect as u8 as f32,
            chromatic_aberration: effect.chromatic_aberration as u8 as f32,
            restore: restore_slot.is_some() as u8 as f32,
            downscale: (1u32 << job.downsamples) as f32,
            scale_content: effect.scale_content as u8 as f32,
            content_offset: effect.content_offset,
            ..Uniforms::default()
        };
        let bind_group = self.bind_group(device, &source_view.view, &restore_texture.view);
        let offset = self.push_uniforms(&uniforms);
        self.draws.push(GlassDraw {
            index: job.index,
            offset,
            bind_group,
        });
    }

    /// Draws backdrop `index` if it is a glass prepared by the current group.
    /// The caller re-binds its own pipeline and groups for the next draw.
    pub(crate) fn draw_glass(&self, pass: &mut wgpu::RenderPass<'_>, index: usize) {
        let Some(draw) = self.draws.iter().find(|draw| draw.index == index) else {
            return;
        };
        pass.set_pipeline(&self.glass);
        pass.set_bind_group(0, &draw.bind_group, &[draw.offset]);
        pass.draw(0..4, 0..1);
    }

    /// Copies the offscreen target to the surface.
    pub(crate) fn blit(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        frame_view: &wgpu::TextureView,
    ) {
        let Some(target) = self.target.as_ref() else {
            return;
        };
        let uniforms = Uniforms {
            viewport: target.extent(),
            draw_size: target.extent(),
            source_texture_size: target.extent(),
            ..Uniforms::default()
        };
        let bind_group = self.bind_group(device, &target.view, &self.placeholder.view);
        let offset = self.push_uniforms(&uniforms);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("backdrop_blit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: frame_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        });
        pass.set_pipeline(&self.blit);
        pass.set_bind_group(0, &bind_group, &[offset]);
        pass.draw(0..4, 0..1);
    }
}

/// Group starts for a `width` x `height` target, for tests.
#[doc(hidden)]
pub fn backdrop_group_starts(scene: &Scene, width: i32, height: i32) -> Vec<usize> {
    let batches: Vec<_> = scene.batches().collect();
    let mut starts = Vec::new();
    let screen = Rect {
        x: 0,
        y: 0,
        w: width,
        h: height,
    };
    plan_groups(scene, &batches, screen, &mut starts, &mut Vec::new());
    starts
}
