use super::atlas::AtlasAllocator;
use crate::{
    fonts::{FontId, GlyphId},
    geometry::vector::{vec2f, Vector2F, Vector2I},
    platform::{self, RasterizationOptions},
};
use ordered_float::OrderedFloat;
use wgpu::{Device, Queue, Texture, TextureDescriptor, TextureFormat};
use std::{borrow::Cow, collections::HashMap, sync::Arc};

#[derive(Hash, Eq, PartialEq)]
struct GlyphDescriptor {
    font_id: FontId,
    font_size: OrderedFloat<f32>,
    glyph_id: GlyphId,
    subpixel_variant: (u8, u8),
}

#[derive(Clone)]
pub struct GlyphSprite {
    pub atlas_id: usize,
    pub atlas_origin: Vector2I,
    pub offset: Vector2I,
    pub size: Vector2I,
}

#[derive(Hash, Eq, PartialEq)]
struct IconDescriptor {
    path: Cow<'static, str>,
    width: i32,
    height: i32,
}

#[derive(Clone)]
pub struct IconSprite {
    pub atlas_id: usize,
    pub atlas_origin: Vector2I,
    pub size: Vector2I,
}

pub struct SpriteCache {
    fonts: Arc<dyn platform::FontSystem>,
    atlases: AtlasAllocator,
    glyphs: HashMap<GlyphDescriptor, Option<GlyphSprite>>,
    icons: HashMap<IconDescriptor, IconSprite>,
    scale_factor: f32,
}

impl SpriteCache {
    pub fn new(
        device: Arc<Device>,
        queue: Arc<Queue>,
        size: Vector2I,
        scale_factor: f32,
        fonts: Arc<dyn platform::FontSystem>,
    ) -> Self {
        let descriptor = TextureDescriptor {
            size: wgpu::Extent3d {
                width: size.x() as u32,
                height: size.y() as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            label: None,
        };

        Self {
            fonts,
            atlases: AtlasAllocator::new(device, queue, descriptor),
            glyphs: Default::default(),
            icons: Default::default(),
            scale_factor,
        }
    }

    pub fn set_scale_factor(&mut self, scale_factor: f32) {
        if scale_factor != self.scale_factor {
            self.icons.clear();
            self.glyphs.clear();
            self.atlases.clear();
        }
        self.scale_factor = scale_factor;
    }

    pub fn render_glyph(
        &mut self,
        font_id: FontId,
        font_size: f32,
        glyph_id: GlyphId,
        target_position: Vector2F,
    ) -> Option<GlyphSprite> {
        const SUBPIXEL_VARIANTS: u8 = 4;

        let scale_factor = self.scale_factor;
        let target_position = target_position * scale_factor;
        let fonts = &self.fonts;
        let atlases = &mut self.atlases;
        let subpixel_variant = (
            (target_position.x().fract() * SUBPIXEL_VARIANTS as f32).round() as u8
                % SUBPIXEL_VARIANTS,
            (target_position.y().fract() * SUBPIXEL_VARIANTS as f32).round() as u8
                % SUBPIXEL_VARIANTS,
        );
        self.glyphs
            .entry(GlyphDescriptor {
                font_id,
                font_size: OrderedFloat(font_size),
                glyph_id,
                subpixel_variant,
            })
            .or_insert_with(|| {
                let subpixel_shift = vec2f(
                    subpixel_variant.0 as f32 / SUBPIXEL_VARIANTS as f32,
                    subpixel_variant.1 as f32 / SUBPIXEL_VARIANTS as f32,
                );
                let (glyph_bounds, mask) = fonts.rasterize_glyph(
                    font_id,
                    font_size,
                    glyph_id,
                    subpixel_shift,
                    scale_factor,
                    RasterizationOptions::Alpha,
                )?;

                let (alloc_id, atlas_bounds) = atlases.upload(glyph_bounds.size(), &mask);
                Some(GlyphSprite {
                    atlas_id: alloc_id.atlas_id,
                    atlas_origin: atlas_bounds.origin(),
                    offset: glyph_bounds.origin(),
                    size: glyph_bounds.size(),
                })
            })
            .clone()
    }

    pub fn render_icon(
        &mut self,
        size: Vector2I,
        path: Cow<'static, str>,
        svg: usvg::Tree,
    ) -> IconSprite {
        let atlases = &mut self.atlases;
        self.icons
            .entry(IconDescriptor {
                path,
                width: size.x(),
                height: size.y(),
            })
            .or_insert_with(|| {
                let mut pixmap = tiny_skia::Pixmap::new(size.x() as u32, size.y() as u32).unwrap();
                resvg::render(&svg, usvg::FitTo::Width(size.x() as u32), pixmap.as_mut());
                let mask = pixmap
                    .pixels()
                    .iter()
                    .map(|a| a.alpha())
                    .collect::<Vec<_>>();

                let (alloc_id, atlas_bounds) = atlases.upload(size, &mask);
                IconSprite {
                    atlas_id: alloc_id.atlas_id,
                    atlas_origin: atlas_bounds.origin(),
                    size,
                }
            })
            .clone()
    }

    pub fn atlas_texture(&self, atlas_id: usize) -> Option<&Texture> {
        self.atlases.texture(atlas_id)
    }
}
