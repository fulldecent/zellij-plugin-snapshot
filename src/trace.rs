//! Outline glyphs from the bundled Nerd Font into SVG path data.

use std::sync::OnceLock;
use ttf_parser::{Face, GlyphId, OutlineBuilder};

pub const FONT_PX: f32 = 16.0;

fn font_bytes() -> &'static [u8] {
    include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/fonts/JetBrainsMonoNerdFont-Regular.ttf"
    ))
}

fn face() -> Face<'static> {
    Face::parse(font_bytes(), 0).expect("parse Nerd Font")
}

pub struct Metrics {
    pub cell_w: f32,
    pub cell_h: f32,
    pub ascender: f32,
    pub scale: f32,
}

pub fn metrics() -> Metrics {
    let face = face();
    let units = face.units_per_em() as f32;
    let scale = FONT_PX / units;
    let gid = face.glyph_index('M').unwrap_or(GlyphId(0));
    let advance = face.glyph_hor_advance(gid).unwrap_or(face.units_per_em()) as f32 * scale;
    let ascender = face.ascender() as f32 * scale;
    let descender = face.descender() as f32 * scale;
    let line_gap = face.line_gap() as f32 * scale;
    let cell_h = (ascender - descender + line_gap).max(FONT_PX);
    Metrics {
        cell_w: advance,
        cell_h,
        ascender,
        scale,
    }
}

struct PathSink {
    d: String,
}

impl OutlineBuilder for PathSink {
    fn move_to(&mut self, x: f32, y: f32) {
        self.d.push_str(&format!("M{x:.3} {y:.3}"));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.d.push_str(&format!("L{x:.3} {y:.3}"));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.d.push_str(&format!("Q{x1:.3} {y1:.3} {x:.3} {y:.3}"));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.d
            .push_str(&format!("C{x1:.3} {y1:.3} {x2:.3} {y2:.3} {x:.3} {y:.3}"));
    }
    fn close(&mut self) {
        self.d.push('Z');
    }
}

/// Font-unit outline of `ch`, or empty if the glyph has no contours (e.g. space).
pub fn glyph_path(ch: char) -> String {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static CACHE: OnceLock<Mutex<HashMap<char, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let g = cache.lock().unwrap();
        if let Some(d) = g.get(&ch) {
            return d.clone();
        }
    }
    let face = face();
    let gid = face.glyph_index(ch).unwrap_or(GlyphId(0));
    let mut sink = PathSink { d: String::new() };
    let _ = face.outline_glyph(gid, &mut sink);
    cache.lock().unwrap().insert(ch, sink.d.clone());
    sink.d
}
