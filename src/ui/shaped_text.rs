use std::collections::{HashMap, VecDeque};

use eframe::egui::{self, Event, ImeEvent, Key};

const TEXTURE_CACHE_LIMIT: usize = 768;
const MAX_RASTER_WIDTH_PX: u32 = 4096;
const MAX_RASTER_HEIGHT_PX: u32 = 512;

#[derive(Clone, Hash, PartialEq, Eq)]
struct TextureKey {
    text: String,
    font_family: Option<String>,
    font_size_px_bits: u32,
    width_px: u32,
    height_px: u32,
    color: [u8; 4],
}

pub struct ShapedTextRenderer {
    inner: Option<ShapedTextRendererInner>,
}

#[derive(Clone, Default)]
struct SinglelineEditState {
    cursor: usize,
    selection_anchor: Option<usize>,
    preedit: String,
}

pub struct SinglelineEditOutput {
    pub response: egui::Response,
    pub enter: bool,
    pub tab: bool,
    pub escaped: bool,
}

struct ShapedTextRendererInner {
    font_system: cosmic_text::FontSystem,
    swash_cache: cosmic_text::SwashCache,
    textures: HashMap<TextureKey, egui::TextureHandle>,
    texture_order: VecDeque<TextureKey>,
    next_texture_id: u64,
}

impl ShapedTextRenderer {
    pub fn new() -> Self {
        Self { inner: None }
    }

    pub fn paint(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        rect: egui::Rect,
        text: &str,
        font_size: f32,
        color: egui::Color32,
        font_family: Option<&str>,
    ) {
        self.inner()
            .paint(ctx, painter, rect, text, font_size, color, font_family);
    }

    pub fn measure_width(&mut self, text: &str, font_size: f32, font_family: Option<&str>) -> f32 {
        self.inner().measure_width(text, font_size, font_family)
    }

    pub fn singleline_editor(
        &mut self,
        ui: &mut egui::Ui,
        id: egui::Id,
        rect: egui::Rect,
        text: &mut String,
        font_size: f32,
        color: egui::Color32,
        font_family: Option<&str>,
    ) -> SinglelineEditOutput {
        let response = ui.interact(rect, id, egui::Sense::click());
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        if response.clicked() {
            response.request_focus();
        }
        if !response.has_focus() {
            response.request_focus();
        }

        let mut state = ui
            .data_mut(|data| data.get_temp::<SinglelineEditState>(id))
            .unwrap_or_default();
        state.cursor = clamp_to_char_boundary(text, state.cursor);
        if response.gained_focus() {
            state.cursor = text.len();
            state.selection_anchor = None;
            state.preedit.clear();
        }
        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let content_rect = editor_content_rect(rect);
                state.cursor = self.inner().hit_index(
                    text,
                    font_size,
                    font_family,
                    content_rect.size(),
                    pos - content_rect.min,
                    ui.ctx().pixels_per_point(),
                );
                state.selection_anchor = None;
            }
        }

        let mut enter = false;
        let mut tab = false;
        let mut escaped = false;

        if response.has_focus() || response.gained_focus() {
            let events = ui.input(|input| input.events.clone());
            for event in events {
                match event {
                    Event::Text(input) => {
                        if input != "\n" && input != "\r" {
                            insert_text(text, &mut state, &input);
                        }
                    }
                    Event::Paste(input) => {
                        let single_line = input.replace(['\r', '\n'], " ");
                        insert_text(text, &mut state, &single_line);
                    }
                    Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => match key {
                        Key::Enter => enter = true,
                        Key::Tab => tab = true,
                        Key::Escape => escaped = true,
                        Key::Backspace if modifiers.command => {
                            let cursor = state.cursor;
                            delete_selection_or_range(text, &mut state, 0, cursor);
                        }
                        Key::Backspace => {
                            if selected_range(&state).is_some() {
                                delete_selection(text, &mut state);
                            } else {
                                let start = prev_char_boundary(text, state.cursor);
                                let cursor = state.cursor;
                                delete_selection_or_range(text, &mut state, start, cursor);
                            }
                        }
                        Key::Delete => {
                            if selected_range(&state).is_some() {
                                delete_selection(text, &mut state);
                            } else {
                                let end = next_char_boundary(text, state.cursor);
                                let cursor = state.cursor;
                                delete_selection_or_range(text, &mut state, cursor, end);
                            }
                        }
                        Key::ArrowLeft => {
                            let next = prev_char_boundary(text, state.cursor);
                            move_cursor(&mut state, next, modifiers.shift);
                        }
                        Key::ArrowRight => {
                            let next = next_char_boundary(text, state.cursor);
                            move_cursor(&mut state, next, modifiers.shift);
                        }
                        Key::Home => move_cursor(&mut state, 0, modifiers.shift),
                        Key::End => move_cursor(&mut state, text.len(), modifiers.shift),
                        Key::A if modifiers.command => {
                            state.cursor = text.len();
                            state.selection_anchor = Some(0);
                        }
                        Key::C if modifiers.command => {
                            if let Some((start, end)) = selected_range(&state) {
                                ui.ctx().copy_text(text[start..end].to_owned());
                            }
                        }
                        Key::X if modifiers.command => {
                            if let Some((start, end)) = selected_range(&state) {
                                ui.ctx().copy_text(text[start..end].to_owned());
                                delete_selection(text, &mut state);
                            }
                        }
                        _ => {}
                    },
                    Event::Ime(ImeEvent::Preedit(preedit)) => {
                        state.preedit = preedit;
                    }
                    Event::Ime(ImeEvent::Commit(commit)) => {
                        state.preedit.clear();
                        insert_text(text, &mut state, &commit);
                    }
                    Event::Ime(ImeEvent::Disabled) => {
                        state.preedit.clear();
                    }
                    _ => {}
                }
            }
        }

        state.cursor = clamp_to_char_boundary(text, state.cursor);
        let content_rect = editor_content_rect(rect);
        let painted_text = text_with_preedit(text, state.cursor, &state.preedit);
        self.paint(
            ui.ctx(),
            ui.painter(),
            content_rect,
            &painted_text,
            font_size,
            color,
            font_family,
        );

        if response.has_focus() || response.gained_focus() {
            let caret_index = state.cursor + state.preedit.len();
            let caret_x = self.inner().cursor_x(
                &painted_text,
                caret_index,
                font_size,
                font_family,
                content_rect.size(),
                ui.ctx().pixels_per_point(),
            );
            let time = ui.input(|input| input.time);
            if (time * 2.0) as i64 % 2 == 0 {
                let x = content_rect.left() + caret_x;
                ui.painter().line_segment(
                    [
                        egui::pos2(x, content_rect.center().y - font_size * 0.65),
                        egui::pos2(x, content_rect.center().y + font_size * 0.65),
                    ],
                    egui::Stroke::new(1.0, color),
                );
            }
            ui.ctx().request_repaint();
        }

        ui.data_mut(|data| data.insert_temp(id, state));

        SinglelineEditOutput {
            response,
            enter,
            tab,
            escaped,
        }
    }

    #[cfg(test)]
    fn rasterize(
        &mut self,
        text: &str,
        width_px: u32,
        height_px: u32,
        font_size_px: f32,
        color: egui::Color32,
        font_family: Option<&str>,
    ) -> egui::ColorImage {
        self.inner()
            .rasterize(text, width_px, height_px, font_size_px, color, font_family)
    }

    fn inner(&mut self) -> &mut ShapedTextRendererInner {
        self.inner.get_or_insert_with(ShapedTextRendererInner::new)
    }
}

impl ShapedTextRendererInner {
    fn new() -> Self {
        Self {
            font_system: cosmic_text::FontSystem::new(),
            swash_cache: cosmic_text::SwashCache::new(),
            textures: HashMap::new(),
            texture_order: VecDeque::new(),
            next_texture_id: 0,
        }
    }

    fn paint(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        rect: egui::Rect,
        text: &str,
        font_size: f32,
        color: egui::Color32,
        font_family: Option<&str>,
    ) {
        if text.is_empty() || rect.width() <= 1.0 || rect.height() <= 1.0 {
            return;
        }

        let pixels_per_point = ctx.pixels_per_point().max(1.0);
        let requested_width_px = (rect.width() * pixels_per_point).ceil().max(1.0) as u32;
        let requested_height_px = (rect.height() * pixels_per_point).ceil().max(1.0) as u32;
        let width_px = requested_width_px.min(MAX_RASTER_WIDTH_PX);
        let height_px = requested_height_px.min(MAX_RASTER_HEIGHT_PX);
        let font_size_px = (font_size * pixels_per_point).max(1.0);
        let family = font_family
            .filter(|name| !name.is_empty())
            .map(str::to_owned);

        let key = TextureKey {
            text: text.to_owned(),
            font_family: family,
            font_size_px_bits: font_size_px.to_bits(),
            width_px,
            height_px,
            color: [color.r(), color.g(), color.b(), color.a()],
        };

        let texture_id = if let Some(texture) = self.textures.get(&key) {
            texture.id()
        } else {
            let image = self.rasterize(text, width_px, height_px, font_size_px, color, font_family);
            let name = format!("colomin_shaped_text_{}", self.next_texture_id);
            self.next_texture_id = self.next_texture_id.wrapping_add(1);
            let texture = ctx.load_texture(name, image, egui::TextureOptions::LINEAR);
            let texture_id = texture.id();
            self.insert_texture(key, texture);
            texture_id
        };

        let draw_width = width_px as f32 / pixels_per_point;
        let draw_height = height_px as f32 / pixels_per_point;
        let draw_rect = if requested_width_px > width_px && is_rtl_text(text) {
            egui::Rect::from_min_size(
                egui::pos2(rect.right() - draw_width, rect.top()),
                egui::vec2(draw_width, draw_height),
            )
        } else {
            egui::Rect::from_min_size(rect.min, egui::vec2(draw_width, draw_height))
        };

        painter.with_clip_rect(rect).image(
            texture_id,
            draw_rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }

    fn measure_width(&mut self, text: &str, font_size: f32, font_family: Option<&str>) -> f32 {
        if text.is_empty() {
            return 0.0;
        }

        let metrics = cosmic_text::Metrics::relative(font_size.max(1.0), 1.25);
        let mut buffer = cosmic_text::Buffer::new(&mut self.font_system, metrics);
        buffer.set_wrap(cosmic_text::Wrap::None);
        buffer.set_size(None, Some(metrics.line_height));
        let attrs = attrs_for_family(font_family);
        buffer.set_text(text, &attrs, cosmic_text::Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);

        let mut width = 0.0_f32;
        for run in buffer.layout_runs() {
            width = width.max(run.line_w);
        }
        width
    }

    fn cursor_x(
        &mut self,
        text: &str,
        cursor: usize,
        font_size: f32,
        font_family: Option<&str>,
        size_points: egui::Vec2,
        pixels_per_point: f32,
    ) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let width_px = (size_points.x * pixels_per_point).ceil().max(1.0);
        let height_px = (size_points.y * pixels_per_point).ceil().max(1.0);
        let font_size_px = (font_size * pixels_per_point).max(1.0);
        let metrics = cosmic_text::Metrics::new(font_size_px, height_px);
        let mut buffer = cosmic_text::Buffer::new(&mut self.font_system, metrics);
        buffer.set_wrap(cosmic_text::Wrap::None);
        buffer.set_size(Some(width_px), Some(height_px));
        let attrs = attrs_for_family(font_family);
        buffer.set_text(text, &attrs, cosmic_text::Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);

        let cursor = cosmic_text::Cursor::new(0, clamp_to_char_boundary(text, cursor));
        buffer
            .cursor_position(&cursor)
            .map(|(x, _)| x / pixels_per_point)
            .unwrap_or(0.0)
    }

    fn hit_index(
        &mut self,
        text: &str,
        font_size: f32,
        font_family: Option<&str>,
        size_points: egui::Vec2,
        offset_points: egui::Vec2,
        pixels_per_point: f32,
    ) -> usize {
        if text.is_empty() {
            return 0;
        }
        let width_px = (size_points.x * pixels_per_point).ceil().max(1.0);
        let height_px = (size_points.y * pixels_per_point).ceil().max(1.0);
        let font_size_px = (font_size * pixels_per_point).max(1.0);
        let metrics = cosmic_text::Metrics::new(font_size_px, height_px);
        let mut buffer = cosmic_text::Buffer::new(&mut self.font_system, metrics);
        buffer.set_wrap(cosmic_text::Wrap::None);
        buffer.set_size(Some(width_px), Some(height_px));
        let attrs = attrs_for_family(font_family);
        buffer.set_text(text, &attrs, cosmic_text::Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);

        let x = (offset_points.x * pixels_per_point).clamp(0.0, width_px);
        let y = (offset_points.y * pixels_per_point).clamp(0.0, height_px);
        buffer
            .hit(x, y)
            .map(|cursor| clamp_to_char_boundary(text, cursor.index))
            .unwrap_or_else(|| text.len())
    }

    fn insert_texture(&mut self, key: TextureKey, texture: egui::TextureHandle) {
        if self.textures.len() >= TEXTURE_CACHE_LIMIT {
            if let Some(oldest) = self.texture_order.pop_front() {
                self.textures.remove(&oldest);
            }
        }
        self.texture_order.push_back(key.clone());
        self.textures.insert(key, texture);
    }

    fn rasterize(
        &mut self,
        text: &str,
        width_px: u32,
        height_px: u32,
        font_size_px: f32,
        color: egui::Color32,
        font_family: Option<&str>,
    ) -> egui::ColorImage {
        let size = [width_px as usize, height_px as usize];
        let mut pixels = vec![egui::Color32::TRANSPARENT; size[0] * size[1]];
        let metrics = cosmic_text::Metrics::new(font_size_px.max(1.0), height_px as f32);
        let mut buffer = cosmic_text::Buffer::new(&mut self.font_system, metrics);
        buffer.set_wrap(cosmic_text::Wrap::None);
        buffer.set_size(Some(width_px as f32), Some(height_px as f32));
        let attrs = attrs_for_family(font_family);
        buffer.set_text(text, &attrs, cosmic_text::Shaping::Advanced, None);

        let text_color = cosmic_text::Color::rgba(color.r(), color.g(), color.b(), color.a());
        buffer.draw(
            &mut self.font_system,
            &mut self.swash_cache,
            text_color,
            |x, y, w, h, color| {
                let src = egui::Color32::from_rgba_unmultiplied(
                    color.r(),
                    color.g(),
                    color.b(),
                    color.a(),
                );
                if src.a() == 0 {
                    return;
                }
                for dy in 0..h as i32 {
                    let py = y + dy;
                    if !(0..height_px as i32).contains(&py) {
                        continue;
                    }
                    for dx in 0..w as i32 {
                        let px = x + dx;
                        if !(0..width_px as i32).contains(&px) {
                            continue;
                        }
                        let idx = py as usize * size[0] + px as usize;
                        blend_over(&mut pixels[idx], src);
                    }
                }
            },
        );

        egui::ColorImage::new(size, pixels)
    }
}

fn attrs_for_family(font_family: Option<&str>) -> cosmic_text::Attrs<'_> {
    if let Some(name) = font_family.filter(|name| !name.is_empty()) {
        cosmic_text::Attrs::new().family(cosmic_text::Family::Name(name))
    } else {
        cosmic_text::Attrs::new()
    }
}

fn editor_content_rect(rect: egui::Rect) -> egui::Rect {
    rect.shrink2(egui::vec2(4.0, 0.0))
}

fn text_with_preedit(text: &str, cursor: usize, preedit: &str) -> String {
    if preedit.is_empty() {
        return text.to_owned();
    }
    let cursor = clamp_to_char_boundary(text, cursor);
    let mut combined = String::with_capacity(text.len() + preedit.len());
    combined.push_str(&text[..cursor]);
    combined.push_str(preedit);
    combined.push_str(&text[cursor..]);
    combined
}

fn selected_range(state: &SinglelineEditState) -> Option<(usize, usize)> {
    let anchor = state.selection_anchor?;
    if anchor == state.cursor {
        None
    } else {
        Some((anchor.min(state.cursor), anchor.max(state.cursor)))
    }
}

fn move_cursor(state: &mut SinglelineEditState, cursor: usize, extending_selection: bool) {
    if extending_selection && state.selection_anchor.is_none() {
        state.selection_anchor = Some(state.cursor);
    } else if !extending_selection {
        state.selection_anchor = None;
    }
    state.cursor = cursor;
    state.preedit.clear();
}

fn insert_text(text: &mut String, state: &mut SinglelineEditState, input: &str) {
    if input.is_empty() {
        return;
    }
    delete_selection(text, state);
    state.cursor = clamp_to_char_boundary(text, state.cursor);
    text.insert_str(state.cursor, input);
    state.cursor += input.len();
    state.selection_anchor = None;
    state.preedit.clear();
}

fn delete_selection(text: &mut String, state: &mut SinglelineEditState) {
    if let Some((start, end)) = selected_range(state) {
        delete_selection_or_range(text, state, start, end);
    }
}

fn delete_selection_or_range(
    text: &mut String,
    state: &mut SinglelineEditState,
    start: usize,
    end: usize,
) {
    let start = clamp_to_char_boundary(text, start);
    let end = clamp_to_char_boundary(text, end);
    if start < end {
        text.replace_range(start..end, "");
    }
    state.cursor = start;
    state.selection_anchor = None;
    state.preedit.clear();
}

fn clamp_to_char_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn prev_char_boundary(text: &str, index: usize) -> usize {
    let index = clamp_to_char_boundary(text, index);
    text[..index]
        .char_indices()
        .last()
        .map(|(idx, _)| idx)
        .unwrap_or(0)
}

fn next_char_boundary(text: &str, index: usize) -> usize {
    let index = clamp_to_char_boundary(text, index);
    text[index..]
        .char_indices()
        .nth(1)
        .map(|(offset, _)| index + offset)
        .unwrap_or(text.len())
}

pub fn should_shape_text(text: &str) -> bool {
    text.chars().any(|ch| !ch.is_ascii() && !ch.is_control())
}

fn is_rtl_text(text: &str) -> bool {
    text.chars().any(is_rtl_codepoint)
}

fn is_rtl_codepoint(ch: char) -> bool {
    matches!(
        ch as u32,
        0x0590..=0x08FF
            | 0xFB1D..=0xFDFF
            | 0xFE70..=0xFEFF
            | 0x10800..=0x10FFF
            | 0x1E800..=0x1EFFF
    )
}

fn blend_over(dst: &mut egui::Color32, src: egui::Color32) {
    let src_a = src.a() as f32 / 255.0;
    if src_a <= 0.0 {
        return;
    }
    let dst_a = dst.a() as f32 / 255.0;
    let out_a = src_a + dst_a * (1.0 - src_a);
    if out_a <= 0.0 {
        *dst = egui::Color32::TRANSPARENT;
        return;
    }

    let blend = |src_c: u8, dst_c: u8| -> u8 {
        (((src_c as f32 * src_a) + (dst_c as f32 * dst_a * (1.0 - src_a))) / out_a)
            .round()
            .clamp(0.0, 255.0) as u8
    };

    *dst = egui::Color32::from_rgba_unmultiplied(
        blend(src.r(), dst.r()),
        blend(src.g(), dst.g()),
        blend(src.b(), dst.b()),
        (out_a * 255.0).round().clamp(0.0, 255.0) as u8,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_shape_text_detects_non_ascii_scripts() {
        assert!(should_shape_text("تهران"));
        assert!(should_shape_text("東京"));
        assert!(!should_shape_text("ali@example.com"));
    }

    #[test]
    fn rasterize_produces_visible_pixels_for_persian_text() {
        let mut renderer = ShapedTextRenderer::new();
        let image = renderer.rasterize("شهر", 160, 48, 24.0, egui::Color32::BLACK, None);

        assert!(image.pixels.iter().any(|pixel| pixel.a() > 0));
    }

    #[test]
    fn measure_width_handles_shaped_text() {
        let mut renderer = ShapedTextRenderer::new();
        assert!(renderer.measure_width("تهران", 14.0, None) > 0.0);
    }

    #[test]
    fn edit_helpers_preserve_utf8_boundaries() {
        let mut text = "علی".to_owned();
        let mut state = SinglelineEditState {
            cursor: text.len(),
            selection_anchor: None,
            preedit: String::new(),
        };

        let start = prev_char_boundary(&text, state.cursor);
        let cursor = state.cursor;
        delete_selection_or_range(&mut text, &mut state, start, cursor);
        insert_text(&mut text, &mut state, "ا");

        assert_eq!(text, "علا");
    }
}
