use cosmic_text::{
    Attrs, Buffer, CacheKey, Family, FontSystem, Metrics, Shaping, SwashCache, SwashContent,
    SwashImage,
};
use std::collections::HashMap;

/// Wraps cosmic-text for font loading, text measurement, and glyph rasterization.
pub struct TextEngine {
    pub font_system: FontSystem,
    pub swash_cache: SwashCache,
}

/// A positioned glyph ready for atlas upload and rendering.
pub struct PositionedGlyph {
    pub cache_key: CacheKey,
    /// Pixel position of the glyph top-left relative to the text origin.
    pub x: i32,
    pub y: i32,
}

impl TextEngine {
    pub fn new() -> Self {
        Self {
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
        }
    }

    /// Measure text and return (width, height) in pixels.
    pub fn measure(&mut self, text: &str, font_size: f32, max_width: f32) -> (f32, f32) {
        if text.is_empty() {
            return (0.0, font_size * 1.2);
        }

        let line_height = (font_size * 1.3).ceil();
        let metrics = Metrics::new(font_size, line_height);
        let mut buffer = Buffer::new_empty(metrics);
        buffer.set_size(&mut self.font_system, Some(max_width), None);
        buffer.set_text(
            &mut self.font_system,
            text,
            Attrs::new().family(Family::SansSerif),
            Shaping::Advanced,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);

        let mut width = 0.0f32;
        let mut height = 0.0f32;

        for run in buffer.layout_runs() {
            width = width.max(run.line_w);
            height = run.line_top + line_height;
        }

        // Ensure minimum height of one line
        if height < line_height {
            height = line_height;
        }

        (width.ceil(), height.ceil())
    }

    /// Shape text and return positioned glyphs for rendering.
    pub fn shape(
        &mut self,
        text: &str,
        font_size: f32,
        max_width: f32,
    ) -> Vec<PositionedGlyph> {
        if text.is_empty() {
            return Vec::new();
        }

        let line_height = (font_size * 1.3).ceil();
        let metrics = Metrics::new(font_size, line_height);
        let mut buffer = Buffer::new_empty(metrics);
        buffer.set_size(&mut self.font_system, Some(max_width), None);
        buffer.set_text(
            &mut self.font_system,
            text,
            Attrs::new().family(Family::SansSerif),
            Shaping::Advanced,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);

        let mut glyphs = Vec::new();

        for run in buffer.layout_runs() {
            for glyph in run.glyphs.iter() {
                let physical = glyph.physical((0.0, 0.0), 1.0);
                glyphs.push(PositionedGlyph {
                    cache_key: physical.cache_key,
                    x: physical.x,
                    y: physical.y + run.line_y as i32,
                });
            }
        }

        glyphs
    }

    /// Rasterize a single glyph, returning its bitmap image.
    pub fn rasterize(&mut self, cache_key: CacheKey) -> Option<SwashImage> {
        self.swash_cache
            .get_image_uncached(&mut self.font_system, cache_key)
    }
}

/// A texture atlas for caching rasterized glyphs on the GPU.
pub struct GlyphAtlas {
    /// Atlas dimensions.
    pub width: u32,
    pub height: u32,
    /// CPU-side pixel data (R8).
    pub data: Vec<u8>,
    /// Current packing cursor.
    cursor_x: u32,
    cursor_y: u32,
    row_height: u32,
    /// Map from CacheKey to atlas region.
    entries: HashMap<CacheKey, GlyphEntry>,
    /// Whether the texture needs re-upload.
    pub dirty: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct GlyphEntry {
    /// UV coordinates in the atlas (0..1 range).
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
    /// Pixel dimensions of the glyph.
    pub width: u32,
    pub height: u32,
    /// Placement offset from the glyph origin.
    pub left: i32,
    pub top: i32,
}

impl GlyphAtlas {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![0u8; (width * height) as usize],
            cursor_x: 0,
            cursor_y: 0,
            row_height: 0,
            entries: HashMap::new(),
            dirty: false,
        }
    }

    /// Get or insert a glyph into the atlas.
    /// Returns None if the glyph couldn't be rasterized or doesn't fit.
    pub fn get_or_insert(
        &mut self,
        cache_key: CacheKey,
        text_engine: &mut TextEngine,
    ) -> Option<GlyphEntry> {
        if let Some(entry) = self.entries.get(&cache_key) {
            return Some(*entry);
        }

        let image = text_engine.rasterize(cache_key)?;
        let gw = image.placement.width;
        let gh = image.placement.height;

        if gw == 0 || gh == 0 {
            // Whitespace glyph — insert a zero-size entry.
            let entry = GlyphEntry {
                u0: 0.0,
                v0: 0.0,
                u1: 0.0,
                v1: 0.0,
                width: 0,
                height: 0,
                left: image.placement.left,
                top: image.placement.top,
            };
            self.entries.insert(cache_key, entry);
            return Some(entry);
        }

        // Advance to next row if this glyph doesn't fit.
        if self.cursor_x + gw > self.width {
            self.cursor_y += self.row_height;
            self.cursor_x = 0;
            self.row_height = 0;
        }

        if self.cursor_y + gh > self.height {
            // Atlas is full.
            return None;
        }

        // Copy glyph pixels into atlas.
        let pixel_data = match image.content {
            SwashContent::Mask => &image.data,
            SwashContent::Color | SwashContent::SubpixelMask => {
                // For color/subpixel glyphs, take only the alpha channel.
                // (Simple fallback — budget devices rarely need color emoji.)
                &image.data
            }
        };

        for row in 0..gh {
            let src_start = (row * gw) as usize;
            let dst_start = ((self.cursor_y + row) * self.width + self.cursor_x) as usize;
            let row_len = gw as usize;

            if image.content == SwashContent::Mask {
                if src_start + row_len <= pixel_data.len()
                    && dst_start + row_len <= self.data.len()
                {
                    self.data[dst_start..dst_start + row_len]
                        .copy_from_slice(&pixel_data[src_start..src_start + row_len]);
                }
            } else {
                // Color: extract alpha from RGBA.
                for col in 0..gw as usize {
                    let src_idx = (row as usize * gw as usize + col) * 4 + 3;
                    if src_idx < pixel_data.len() && dst_start + col < self.data.len() {
                        self.data[dst_start + col] = pixel_data[src_idx];
                    }
                }
            }
        }

        let entry = GlyphEntry {
            u0: self.cursor_x as f32 / self.width as f32,
            v0: self.cursor_y as f32 / self.height as f32,
            u1: (self.cursor_x + gw) as f32 / self.width as f32,
            v1: (self.cursor_y + gh) as f32 / self.height as f32,
            width: gw,
            height: gh,
            left: image.placement.left,
            top: image.placement.top,
        };

        self.entries.insert(cache_key, entry);
        self.cursor_x += gw + 1; // 1px padding between glyphs
        self.row_height = self.row_height.max(gh + 1);
        self.dirty = true;

        Some(entry)
    }

    /// Clear the atlas (e.g., when it's full and needs rebuilding).
    pub fn clear(&mut self) {
        self.data.fill(0);
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.row_height = 0;
        self.entries.clear();
        self.dirty = true;
    }
}
