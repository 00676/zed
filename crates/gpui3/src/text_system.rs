mod font_features;
mod line;
mod line_wrapper;
mod text_layout_cache;

use anyhow::anyhow;
pub use font_features::*;
pub use line::*;
use line_wrapper::*;
use smallvec::SmallVec;
pub use text_layout_cache::*;

use crate::{
    px, Bounds, DevicePixels, Hsla, Pixels, PlatformTextSystem, Point, Result, SharedString, Size,
    UnderlineStyle,
};
use collections::HashMap;
use core::fmt;
use parking_lot::{Mutex, RwLock, RwLockUpgradableReadGuard};
use std::{
    cmp,
    fmt::{Debug, Display, Formatter},
    hash::{Hash, Hasher},
    ops::{Deref, DerefMut},
    sync::Arc,
};

#[derive(Hash, PartialEq, Eq, Clone, Copy, Debug)]
#[repr(C)]
pub struct FontId(pub usize);

#[derive(Hash, PartialEq, Eq, Clone, Copy, Debug)]
pub struct FontFamilyId(pub usize);

pub const SUBPIXEL_VARIANTS: u8 = 4;

pub struct TextSystem {
    text_layout_cache: Arc<TextLayoutCache>,
    platform_text_system: Arc<dyn PlatformTextSystem>,
    font_ids_by_font: RwLock<HashMap<Font, FontId>>,
    font_metrics: RwLock<HashMap<FontId, FontMetrics>>,
    wrapper_pool: Mutex<HashMap<FontIdWithSize, Vec<LineWrapper>>>,
    font_runs_pool: Mutex<Vec<Vec<(usize, FontId)>>>,
}

impl TextSystem {
    pub fn new(platform_text_system: Arc<dyn PlatformTextSystem>) -> Self {
        TextSystem {
            text_layout_cache: Arc::new(TextLayoutCache::new(platform_text_system.clone())),
            platform_text_system,
            font_metrics: RwLock::new(HashMap::default()),
            font_ids_by_font: RwLock::new(HashMap::default()),
            wrapper_pool: Mutex::new(HashMap::default()),
            font_runs_pool: Default::default(),
        }
    }

    pub fn font_id(&self, font: &Font) -> Result<FontId> {
        let font_id = self.font_ids_by_font.read().get(font).copied();
        if let Some(font_id) = font_id {
            Ok(font_id)
        } else {
            let font_id = self.platform_text_system.font_id(font)?;
            self.font_ids_by_font.write().insert(font.clone(), font_id);
            Ok(font_id)
        }
    }

    pub fn bounding_box(&self, font_id: FontId, font_size: Pixels) -> Result<Bounds<Pixels>> {
        self.read_metrics(font_id, |metrics| metrics.bounding_box(font_size))
    }

    pub fn typographic_bounds(
        &self,
        font_id: FontId,
        font_size: Pixels,
        character: char,
    ) -> Result<Bounds<Pixels>> {
        let glyph_id = self
            .platform_text_system
            .glyph_for_char(font_id, character)
            .ok_or_else(|| anyhow!("glyph not found for character '{}'", character))?;
        let bounds = self
            .platform_text_system
            .typographic_bounds(font_id, glyph_id)?;
        self.read_metrics(font_id, |metrics| {
            (bounds / metrics.units_per_em as f32 * font_size.0).map(px)
        })
    }

    pub fn advance(&self, font_id: FontId, font_size: Pixels, ch: char) -> Result<Size<Pixels>> {
        let glyph_id = self
            .platform_text_system
            .glyph_for_char(font_id, ch)
            .ok_or_else(|| anyhow!("glyph not found for character '{}'", ch))?;
        let result = self.platform_text_system.advance(font_id, glyph_id)?
            / self.units_per_em(font_id)? as f32;

        Ok(result * font_size)
    }

    pub fn units_per_em(&self, font_id: FontId) -> Result<u32> {
        self.read_metrics(font_id, |metrics| metrics.units_per_em as u32)
    }

    pub fn cap_height(&self, font_id: FontId, font_size: Pixels) -> Result<Pixels> {
        self.read_metrics(font_id, |metrics| metrics.cap_height(font_size))
    }

    pub fn x_height(&self, font_id: FontId, font_size: Pixels) -> Result<Pixels> {
        self.read_metrics(font_id, |metrics| metrics.x_height(font_size))
    }

    pub fn ascent(&self, font_id: FontId, font_size: Pixels) -> Result<Pixels> {
        self.read_metrics(font_id, |metrics| metrics.ascent(font_size))
    }

    pub fn descent(&self, font_id: FontId, font_size: Pixels) -> Result<Pixels> {
        self.read_metrics(font_id, |metrics| metrics.descent(font_size))
    }

    pub fn baseline_offset(
        &self,
        font_id: FontId,
        font_size: Pixels,
        line_height: Pixels,
    ) -> Result<Pixels> {
        let ascent = self.ascent(font_id, font_size)?;
        let descent = self.descent(font_id, font_size)?;
        let padding_top = (line_height - ascent - descent) / 2.;
        Ok(padding_top + ascent)
    }

    fn read_metrics<T>(&self, font_id: FontId, read: impl FnOnce(&FontMetrics) -> T) -> Result<T> {
        let lock = self.font_metrics.upgradable_read();

        if let Some(metrics) = lock.get(&font_id) {
            Ok(read(metrics))
        } else {
            let mut lock = RwLockUpgradableReadGuard::upgrade(lock);
            let metrics = lock
                .entry(font_id)
                .or_insert_with(|| self.platform_text_system.font_metrics(font_id));
            Ok(read(metrics))
        }
    }

    pub fn layout_text(
        &self,
        text: &str,
        font_size: Pixels,
        runs: &[(usize, RunStyle)],
        wrap_width: Option<Pixels>,
    ) -> Result<SmallVec<[Arc<Line>; 1]>> {
        let mut runs = runs
            .iter()
            .map(|(run_len, style)| (*run_len, style))
            .peekable();
        let mut font_runs: Vec<(usize, FontId)> =
            self.font_runs_pool.lock().pop().unwrap_or_default();

        let mut lines = SmallVec::new();
        let mut line_start = 0;
        for line in text.split('\n') {
            let line_end = line_start + line.len();

            let mut last_font: Option<&Font> = None;
            let mut decoration_runs = SmallVec::<[DecorationRun; 32]>::new();
            let mut run_start = line_start;
            while run_start < line_end {
                let Some((run_len, run_style)) = runs.peek_mut() else {
                    break;
                };

                let run_len_within_line = cmp::min(line_end, run_start + *run_len) - run_start;

                if last_font == Some(&run_style.font) {
                    font_runs.last_mut().unwrap().0 += run_len_within_line;
                } else {
                    last_font = Some(&run_style.font);
                    font_runs.push((
                        run_len_within_line,
                        self.platform_text_system.font_id(&run_style.font)?,
                    ));
                }

                if decoration_runs.last().map_or(false, |last_run| {
                    last_run.color == run_style.color && last_run.underline == run_style.underline
                }) {
                    decoration_runs.last_mut().unwrap().len += run_len_within_line as u32;
                } else {
                    decoration_runs.push(DecorationRun {
                        len: run_len_within_line as u32,
                        color: run_style.color,
                        underline: run_style.underline.clone(),
                    });
                }

                if run_len_within_line == *run_len {
                    runs.next();
                } else {
                    // Preserve the remainder of the run for the next line
                    *run_len -= run_len_within_line;
                }
                run_start += run_len_within_line;
            }

            let layout = self
                .text_layout_cache
                .layout_line(line, font_size, &font_runs);
            lines.push(Arc::new(Line::new(layout, decoration_runs)));

            line_start = line_end + 1; // Skip `\n` character.
            font_runs.clear();
        }

        self.font_runs_pool.lock().push(font_runs);

        Ok(lines)
    }

    pub fn end_frame(&self) {
        self.text_layout_cache.end_frame()
    }

    pub fn line_wrapper(
        self: &Arc<Self>,
        font: Font,
        font_size: Pixels,
    ) -> Result<LineWrapperHandle> {
        let lock = &mut self.wrapper_pool.lock();
        let font_id = self.font_id(&font)?;
        let wrappers = lock
            .entry(FontIdWithSize { font_id, font_size })
            .or_default();
        let wrapper = wrappers.pop().map(anyhow::Ok).unwrap_or_else(|| {
            Ok(LineWrapper::new(
                font_id,
                font_size,
                self.platform_text_system.clone(),
            ))
        })?;

        Ok(LineWrapperHandle {
            wrapper: Some(wrapper),
            text_system: self.clone(),
        })
    }

    pub fn raster_bounds(&self, params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        self.platform_text_system.glyph_raster_bounds(params)
    }

    pub fn rasterize_glyph(
        &self,
        glyph_id: &RenderGlyphParams,
    ) -> Result<(Size<DevicePixels>, Vec<u8>)> {
        self.platform_text_system.rasterize_glyph(glyph_id)
    }
}

#[derive(Hash, Eq, PartialEq)]
struct FontIdWithSize {
    font_id: FontId,
    font_size: Pixels,
}

pub struct LineWrapperHandle {
    wrapper: Option<LineWrapper>,
    text_system: Arc<TextSystem>,
}

impl Drop for LineWrapperHandle {
    fn drop(&mut self) {
        let mut state = self.text_system.wrapper_pool.lock();
        let wrapper = self.wrapper.take().unwrap();
        state
            .get_mut(&FontIdWithSize {
                font_id: wrapper.font_id.clone(),
                font_size: wrapper.font_size,
            })
            .unwrap()
            .push(wrapper);
    }
}

impl Deref for LineWrapperHandle {
    type Target = LineWrapper;

    fn deref(&self) -> &Self::Target {
        self.wrapper.as_ref().unwrap()
    }
}

impl DerefMut for LineWrapperHandle {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.wrapper.as_mut().unwrap()
    }
}

/// The degree of blackness or stroke thickness of a font. This value ranges from 100.0 to 900.0,
/// with 400.0 as normal.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct FontWeight(pub f32);

impl Default for FontWeight {
    #[inline]
    fn default() -> FontWeight {
        FontWeight::NORMAL
    }
}

impl Hash for FontWeight {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u32(u32::from_be_bytes(self.0.to_be_bytes()));
    }
}

impl Eq for FontWeight {}

impl FontWeight {
    /// Thin weight (100), the thinnest value.
    pub const THIN: FontWeight = FontWeight(100.0);
    /// Extra light weight (200).
    pub const EXTRA_LIGHT: FontWeight = FontWeight(200.0);
    /// Light weight (300).
    pub const LIGHT: FontWeight = FontWeight(300.0);
    /// Normal (400).
    pub const NORMAL: FontWeight = FontWeight(400.0);
    /// Medium weight (500, higher than normal).
    pub const MEDIUM: FontWeight = FontWeight(500.0);
    /// Semibold weight (600).
    pub const SEMIBOLD: FontWeight = FontWeight(600.0);
    /// Bold weight (700).
    pub const BOLD: FontWeight = FontWeight(700.0);
    /// Extra-bold weight (800).
    pub const EXTRA_BOLD: FontWeight = FontWeight(800.0);
    /// Black weight (900), the thickest value.
    pub const BLACK: FontWeight = FontWeight(900.0);
}

/// Allows italic or oblique faces to be selected.
#[derive(Clone, Copy, Eq, PartialEq, Debug, Hash)]
pub enum FontStyle {
    /// A face that is neither italic not obliqued.
    Normal,
    /// A form that is generally cursive in nature.
    Italic,
    /// A typically-sloped version of the regular face.
    Oblique,
}

impl Default for FontStyle {
    fn default() -> FontStyle {
        FontStyle::Normal
    }
}

impl Display for FontStyle {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        Debug::fmt(self, f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunStyle {
    pub font: Font,
    pub color: Hsla,
    pub underline: Option<UnderlineStyle>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
#[repr(C)]
pub struct GlyphId(u32);

impl From<GlyphId> for u32 {
    fn from(value: GlyphId) -> Self {
        value.0
    }
}

impl From<u16> for GlyphId {
    fn from(num: u16) -> Self {
        GlyphId(num as u32)
    }
}

impl From<u32> for GlyphId {
    fn from(num: u32) -> Self {
        GlyphId(num)
    }
}

#[derive(Default, Debug)]
pub struct LineLayout {
    pub font_size: Pixels,
    pub width: Pixels,
    pub ascent: Pixels,
    pub descent: Pixels,
    pub runs: Vec<ShapedRun>,
    pub len: usize,
}

#[derive(Debug)]
pub struct ShapedRun {
    pub font_id: FontId,
    pub glyphs: SmallVec<[ShapedGlyph; 8]>,
}

#[derive(Clone, Debug)]
pub struct ShapedGlyph {
    pub id: GlyphId,
    pub position: Point<Pixels>,
    pub index: usize,
    pub is_emoji: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderGlyphParams {
    pub(crate) font_id: FontId,
    pub(crate) glyph_id: GlyphId,
    pub(crate) font_size: Pixels,
    pub(crate) subpixel_variant: Point<u8>,
    pub(crate) scale_factor: f32,
    pub(crate) is_emoji: bool,
}

impl Eq for RenderGlyphParams {}

impl Hash for RenderGlyphParams {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.font_id.0.hash(state);
        self.glyph_id.0.hash(state);
        self.font_size.0.to_bits().hash(state);
        self.subpixel_variant.hash(state);
        self.scale_factor.to_bits().hash(state);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderEmojiParams {
    pub(crate) font_id: FontId,
    pub(crate) glyph_id: GlyphId,
    pub(crate) font_size: Pixels,
    pub(crate) scale_factor: f32,
}

impl Eq for RenderEmojiParams {}

impl Hash for RenderEmojiParams {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.font_id.0.hash(state);
        self.glyph_id.0.hash(state);
        self.font_size.0.to_bits().hash(state);
        self.scale_factor.to_bits().hash(state);
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Font {
    pub family: SharedString,
    pub features: FontFeatures,
    pub weight: FontWeight,
    pub style: FontStyle,
}

pub fn font(family: impl Into<SharedString>) -> Font {
    Font {
        family: family.into(),
        features: FontFeatures::default(),
        weight: FontWeight::default(),
        style: FontStyle::default(),
    }
}

impl Font {
    pub fn bold(mut self) -> Self {
        self.weight = FontWeight::BOLD;
        self
    }
}

/// A struct for storing font metrics.
/// It is used to define the measurements of a typeface.
#[derive(Clone, Copy, Debug)]
pub struct FontMetrics {
    /// The number of font units that make up the "em square",
    /// a scalable grid for determining the size of a typeface.
    pub(crate) units_per_em: u32,

    /// The vertical distance from the baseline of the font to the top of the glyph covers.
    pub(crate) ascent: f32,

    /// The vertical distance from the baseline of the font to the bottom of the glyph covers.
    pub(crate) descent: f32,

    /// The recommended additional space to add between lines of type.
    pub(crate) line_gap: f32,

    /// The suggested position of the underline.
    pub(crate) underline_position: f32,

    /// The suggested thickness of the underline.
    pub(crate) underline_thickness: f32,

    /// The height of a capital letter measured from the baseline of the font.
    pub(crate) cap_height: f32,

    /// The height of a lowercase x.
    pub(crate) x_height: f32,

    /// The outer limits of the area that the font covers.
    pub(crate) bounding_box: Bounds<f32>,
}

impl FontMetrics {
    /// Returns the vertical distance from the baseline of the font to the top of the glyph covers in pixels.
    pub fn ascent(&self, font_size: Pixels) -> Pixels {
        Pixels((self.ascent / self.units_per_em as f32) * font_size.0)
    }

    /// Returns the vertical distance from the baseline of the font to the bottom of the glyph covers in pixels.
    pub fn descent(&self, font_size: Pixels) -> Pixels {
        Pixels((self.descent / self.units_per_em as f32) * font_size.0)
    }

    /// Returns the recommended additional space to add between lines of type in pixels.
    pub fn line_gap(&self, font_size: Pixels) -> Pixels {
        Pixels((self.line_gap / self.units_per_em as f32) * font_size.0)
    }

    /// Returns the suggested position of the underline in pixels.
    pub fn underline_position(&self, font_size: Pixels) -> Pixels {
        Pixels((self.underline_position / self.units_per_em as f32) * font_size.0)
    }

    /// Returns the suggested thickness of the underline in pixels.
    pub fn underline_thickness(&self, font_size: Pixels) -> Pixels {
        Pixels((self.underline_thickness / self.units_per_em as f32) * font_size.0)
    }

    /// Returns the height of a capital letter measured from the baseline of the font in pixels.
    pub fn cap_height(&self, font_size: Pixels) -> Pixels {
        Pixels((self.cap_height / self.units_per_em as f32) * font_size.0)
    }

    /// Returns the height of a lowercase x in pixels.
    pub fn x_height(&self, font_size: Pixels) -> Pixels {
        Pixels((self.x_height / self.units_per_em as f32) * font_size.0)
    }

    /// Returns the outer limits of the area that the font covers in pixels.
    pub fn bounding_box(&self, font_size: Pixels) -> Bounds<Pixels> {
        (self.bounding_box / self.units_per_em as f32 * font_size.0).map(px)
    }
}
