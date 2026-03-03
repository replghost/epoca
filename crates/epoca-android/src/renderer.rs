use crate::layout::{LayoutNode, Rect};
use crate::text::{GlyphAtlas, TextEngine};
use crate::theme::{Color, Theme};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use epoca_protocol::NodeKind;

/// Vertex for solid-color rectangles.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct RectVertex {
    position: [f32; 2],
    color: [f32; 4],
}

/// Vertex for textured glyph quads.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct GlyphVertex {
    position: [f32; 2],
    tex_coord: [f32; 2],
    color: [f32; 4],
}

/// 2D renderer using wgpu. Draws colored rectangles and text glyphs.
pub struct Renderer {
    rect_pipeline: wgpu::RenderPipeline,
    glyph_pipeline: wgpu::RenderPipeline,
    viewport_buffer: wgpu::Buffer,
    viewport_bind_group: wgpu::BindGroup,
    atlas_texture: wgpu::Texture,
    atlas_bind_group: wgpu::BindGroup,
    atlas_bind_group_layout: wgpu::BindGroupLayout,
    pub glyph_atlas: GlyphAtlas,
    #[allow(dead_code)]
    surface_format: wgpu::TextureFormat,
}

impl Renderer {
    pub fn new(
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        // Viewport uniform buffer (vec2<f32>)
        let viewport_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("viewport"),
            contents: bytemuck::cast_slice(&[800.0f32, 600.0]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let viewport_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("viewport_layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let viewport_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport_bg"),
            layout: &viewport_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: viewport_buffer.as_entire_binding(),
            }],
        });

        // Glyph atlas texture (1024x1024 R8Unorm)
        let atlas_size = 1024u32;
        let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph_atlas"),
            size: wgpu::Extent3d {
                width: atlas_size,
                height: atlas_size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let atlas_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let atlas_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("atlas_layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let atlas_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("atlas_bg"),
            layout: &atlas_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&atlas_sampler),
                },
            ],
        });

        // Rect pipeline
        let rect_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rect_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("rect.wgsl").into()),
        });

        let rect_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("rect_pipeline_layout"),
                bind_group_layouts: &[&viewport_bind_group_layout],
                push_constant_ranges: &[],
            });

        let rect_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rect_pipeline"),
            layout: Some(&rect_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &rect_shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<RectVertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 8,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x4,
                        },
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &rect_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Glyph pipeline
        let glyph_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("glyph_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("glyph.wgsl").into()),
        });

        let glyph_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("glyph_pipeline_layout"),
                bind_group_layouts: &[&viewport_bind_group_layout, &atlas_bind_group_layout],
                push_constant_ranges: &[],
            });

        let glyph_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("glyph_pipeline"),
            layout: Some(&glyph_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &glyph_shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GlyphVertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 8,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 16,
                            shader_location: 2,
                            format: wgpu::VertexFormat::Float32x4,
                        },
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &glyph_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            rect_pipeline,
            glyph_pipeline,
            viewport_buffer,
            viewport_bind_group,
            atlas_texture,
            atlas_bind_group,
            atlas_bind_group_layout,
            glyph_atlas: GlyphAtlas::new(atlas_size, atlas_size),
            surface_format,
        }
    }

    /// Update viewport size uniform.
    pub fn resize(&self, queue: &wgpu::Queue, width: f32, height: f32) {
        queue.write_buffer(
            &self.viewport_buffer,
            0,
            bytemuck::cast_slice(&[width, height]),
        );
    }

    /// Render a frame from the layout tree.
    pub fn render_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        layout: &LayoutNode,
        text_engine: &mut TextEngine,
        theme: &Theme,
    ) {
        let mut rect_verts: Vec<RectVertex> = Vec::new();
        let mut glyph_verts: Vec<GlyphVertex> = Vec::new();

        // Walk layout tree and collect draw commands.
        self.collect_draw_commands(layout, text_engine, theme, &mut rect_verts, &mut glyph_verts);

        // Upload glyph atlas if dirty.
        if self.glyph_atlas.dirty {
            self.upload_atlas(device, queue);
            self.glyph_atlas.dirty = false;
        }

        // Create vertex buffers.
        let rect_buffer = if !rect_verts.is_empty() {
            Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("rect_verts"),
                contents: bytemuck::cast_slice(&rect_verts),
                usage: wgpu::BufferUsages::VERTEX,
            }))
        } else {
            None
        };

        let glyph_buffer = if !glyph_verts.is_empty() {
            Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("glyph_verts"),
                contents: bytemuck::cast_slice(&glyph_verts),
                usage: wgpu::BufferUsages::VERTEX,
            }))
        } else {
            None
        };

        // Encode render pass.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame"),
        });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: theme.bg.r as f64,
                            g: theme.bg.g as f64,
                            b: theme.bg.b as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });

            // Draw rectangles.
            if let Some(ref buf) = rect_buffer {
                pass.set_pipeline(&self.rect_pipeline);
                pass.set_bind_group(0, &self.viewport_bind_group, &[]);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..rect_verts.len() as u32, 0..1);
            }

            // Draw glyphs.
            if let Some(ref buf) = glyph_buffer {
                pass.set_pipeline(&self.glyph_pipeline);
                pass.set_bind_group(0, &self.viewport_bind_group, &[]);
                pass.set_bind_group(1, &self.atlas_bind_group, &[]);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..glyph_verts.len() as u32, 0..1);
            }
        }

        queue.submit(std::iter::once(encoder.finish()));
    }

    fn collect_draw_commands(
        &mut self,
        node: &LayoutNode,
        text_engine: &mut TextEngine,
        theme: &Theme,
        rect_verts: &mut Vec<RectVertex>,
        glyph_verts: &mut Vec<GlyphVertex>,
    ) {
        let b = &node.bounds;

        match &node.kind {
            NodeKind::Button => {
                let is_primary = node
                    .props
                    .get("variant")
                    .and_then(|v| v.as_str())
                    .map(|s| s == "primary")
                    .unwrap_or(false);

                let bg = if is_primary {
                    theme.primary
                } else {
                    theme.button_bg
                };
                let text_color = if is_primary {
                    theme.primary_text
                } else {
                    theme.text
                };

                push_rect(rect_verts, b, bg);

                let label = node
                    .props
                    .get("label")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Button");

                self.push_text(
                    glyph_verts,
                    text_engine,
                    label,
                    theme.font_size,
                    b.x + theme.button_pad_h,
                    b.y + theme.button_pad_v,
                    b.w - theme.button_pad_h * 2.0,
                    text_color,
                );
            }
            NodeKind::Text => {
                let content = node
                    .props
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let is_heading = node
                    .props
                    .get("style")
                    .and_then(|v| v.as_str())
                    .map(|s| s == "heading")
                    .unwrap_or(false);
                let font_size = if is_heading {
                    theme.heading_size
                } else {
                    theme.font_size
                };

                self.push_text(
                    glyph_verts,
                    text_engine,
                    content,
                    font_size,
                    b.x,
                    b.y,
                    b.w,
                    theme.text,
                );
            }
            NodeKind::Input => {
                // Input background
                push_rect(rect_verts, b, theme.input_bg);
                // Border
                push_border(rect_verts, b, theme.border, 1.0);

                let value = node.props.get("value").and_then(|v| v.as_str());
                let placeholder = node.props.get("placeholder").and_then(|v| v.as_str());

                let (text, color) = if let Some(val) = value {
                    if val.is_empty() {
                        (placeholder.unwrap_or(""), theme.text_muted)
                    } else {
                        (val, theme.text)
                    }
                } else {
                    (placeholder.unwrap_or(""), theme.text_muted)
                };

                if !text.is_empty() {
                    self.push_text(
                        glyph_verts,
                        text_engine,
                        text,
                        theme.font_size,
                        b.x + 8.0,
                        b.y + (b.h - theme.font_size) / 2.0,
                        b.w - 16.0,
                        color,
                    );
                }
            }
            NodeKind::Divider => {
                push_rect(rect_verts, b, theme.border);
            }
            _ => {}
        }

        // Recurse into children.
        for child in &node.children {
            self.collect_draw_commands(child, text_engine, theme, rect_verts, glyph_verts);
        }
    }

    fn push_text(
        &mut self,
        glyph_verts: &mut Vec<GlyphVertex>,
        text_engine: &mut TextEngine,
        text: &str,
        font_size: f32,
        origin_x: f32,
        origin_y: f32,
        max_width: f32,
        color: Color,
    ) {
        let glyphs = text_engine.shape(text, font_size, max_width);
        let color_arr = color.to_array();

        for glyph in &glyphs {
            let entry = match self.glyph_atlas.get_or_insert(glyph.cache_key, text_engine) {
                Some(e) => e,
                None => continue,
            };

            if entry.width == 0 || entry.height == 0 {
                continue;
            }

            let x0 = origin_x + glyph.x as f32 + entry.left as f32;
            let y0 = origin_y + glyph.y as f32 - entry.top as f32;
            let x1 = x0 + entry.width as f32;
            let y1 = y0 + entry.height as f32;

            // Two triangles for the quad.
            glyph_verts.extend_from_slice(&[
                GlyphVertex { position: [x0, y0], tex_coord: [entry.u0, entry.v0], color: color_arr },
                GlyphVertex { position: [x1, y0], tex_coord: [entry.u1, entry.v0], color: color_arr },
                GlyphVertex { position: [x0, y1], tex_coord: [entry.u0, entry.v1], color: color_arr },
                GlyphVertex { position: [x1, y0], tex_coord: [entry.u1, entry.v0], color: color_arr },
                GlyphVertex { position: [x1, y1], tex_coord: [entry.u1, entry.v1], color: color_arr },
                GlyphVertex { position: [x0, y1], tex_coord: [entry.u0, entry.v1], color: color_arr },
            ]);
        }
    }

    fn upload_atlas(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.atlas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.glyph_atlas.data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(self.glyph_atlas.width),
                rows_per_image: Some(self.glyph_atlas.height),
            },
            wgpu::Extent3d {
                width: self.glyph_atlas.width,
                height: self.glyph_atlas.height,
                depth_or_array_layers: 1,
            },
        );

        // Recreate bind group with the updated texture view.
        let atlas_view = self
            .atlas_texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        self.atlas_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("atlas_bg"),
            layout: &self.atlas_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&atlas_sampler),
                },
            ],
        });
    }
}

/// Push 6 vertices for a filled rectangle.
fn push_rect(verts: &mut Vec<RectVertex>, r: &Rect, color: Color) {
    let c = color.to_array();
    let (x0, y0, x1, y1) = (r.x, r.y, r.x + r.w, r.y + r.h);
    verts.extend_from_slice(&[
        RectVertex { position: [x0, y0], color: c },
        RectVertex { position: [x1, y0], color: c },
        RectVertex { position: [x0, y1], color: c },
        RectVertex { position: [x1, y0], color: c },
        RectVertex { position: [x1, y1], color: c },
        RectVertex { position: [x0, y1], color: c },
    ]);
}

/// Push border lines as thin rectangles (1px each side).
fn push_border(verts: &mut Vec<RectVertex>, r: &Rect, color: Color, width: f32) {
    let (x0, y0, x1, y1) = (r.x, r.y, r.x + r.w, r.y + r.h);
    // Top
    push_rect(verts, &Rect::new(x0, y0, r.w, width), color);
    // Bottom
    push_rect(verts, &Rect::new(x0, y1 - width, r.w, width), color);
    // Left
    push_rect(verts, &Rect::new(x0, y0, width, r.h), color);
    // Right
    push_rect(verts, &Rect::new(x1 - width, y0, width, r.h), color);
}
