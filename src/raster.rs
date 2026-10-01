use unicode_width::UnicodeWidthChar;
use zellij_utils::data::{PaletteColor, StyleDeclaration, Styling};
use zellij_utils::shared::eightbit_to_rgb;

use crate::trace;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct Style {
    fg: Option<(u8, u8, u8)>,
    bg: Option<(u8, u8, u8)>,
    bold: bool,
}

#[derive(Clone, Copy)]
struct Cell {
    ch: char,
    style: Style,
}

struct Screen {
    cols: usize,
    rows: usize,
    cells: Vec<Cell>,
    x: usize,
    y: usize,
    style: Style,
}

impl Screen {
    fn new(cols: usize, rows: usize) -> Self {
        let blank = Cell {
            ch: ' ',
            style: Style::default(),
        };
        Self {
            cols: cols.max(1),
            rows: rows.max(1),
            cells: vec![blank; cols.max(1) * rows.max(1)],
            x: 0,
            y: 0,
            style: Style::default(),
        }
    }

    fn idx(&self, x: usize, y: usize) -> usize {
        y * self.cols + x
    }

    fn put(&mut self, ch: char) {
        if ch == '\r' {
            self.x = 0;
            return;
        }
        if ch == '\n' {
            self.newline();
            return;
        }
        if ch == '\t' {
            self.x = (self.x + 8) / 8 * 8;
            if self.x >= self.cols {
                self.newline();
            }
            return;
        }
        if ch.is_control() {
            return;
        }
        let w = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
        if self.x >= self.cols {
            self.newline();
        }
        if self.y >= self.rows {
            return;
        }
        let i = self.idx(self.x, self.y);
        self.cells[i] = Cell {
            ch,
            style: self.style,
        };
        self.x += w;
        if w == 2 && self.x - 1 < self.cols {
            let j = self.idx(self.x - 1, self.y);
            if j < self.cells.len() && self.x - 1 != self.x.saturating_sub(w) {
                // second column of a wide char stays blank
            }
        }
    }

    fn put_str(&mut self, s: &str) {
        for ch in s.chars() {
            self.put(ch);
        }
    }

    fn newline(&mut self) {
        self.x = 0;
        self.y = self.y.saturating_add(1).min(self.rows.saturating_sub(1));
    }

    fn cup(&mut self, row: usize, col: usize) {
        self.y = row.saturating_sub(1).min(self.rows - 1);
        self.x = col.saturating_sub(1).min(self.cols - 1);
    }

    fn el(&mut self, mode: u32) {
        let y = self.y;
        let start = match mode {
            1 => 0,
            2 => 0,
            _ => self.x,
        };
        let end = match mode {
            1 => self.x.saturating_add(1).min(self.cols),
            _ => self.cols,
        };
        for x in start..end {
            let i = self.idx(x, y);
            self.cells[i] = Cell {
                ch: ' ',
                style: self.style,
            };
        }
    }
}

/// Paint ANSI (plus Zellij UI DCS) into a rows×cols grid, then a traced SVG.
///
/// `styling` is the host theme used when expanding semantic DCS (`ribbon`,
/// `text`, `table`, `nested_list`) the same way Zellij maps `color_range` onto
/// `Styling` slots after `render()`.
const TERM_BG: (u8, u8, u8) = (0, 0, 0);
const TERM_FG: (u8, u8, u8) = (229, 229, 229);

pub fn ansi_svg(ansi: &str, cols: u32, rows: u32, styling: &Styling) -> String {
    let screen = paint(ansi, cols as usize, rows as usize, styling);
    screen.to_svg()
}

impl Screen {
    fn to_svg(&self) -> String {
        let m = trace::metrics();
        let cell_w = m.cell_w;
        let cell_h = m.cell_h;
        let w = self.cols as f32 * cell_w;
        let h = self.rows as f32 * cell_h;
        let (br, bg, bb) = TERM_BG;
        let mut out = String::new();
        out.push_str(&format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="{w:.2}" height="{h:.2}" viewBox="0 0 {w:.2} {h:.2}">
<rect width="100%" height="100%" fill="rgb({br},{bg},{bb})"/>
"#
        ));
        for y in 0..self.rows {
            let mut x = 0;
            while x < self.cols {
                let cell = self.cells[self.idx(x, y)];
                let run_bg = cell.style.bg.unwrap_or(TERM_BG);
                let mut end = x + 1;
                while end < self.cols {
                    let n = self.cells[self.idx(end, y)];
                    if n.style.bg.unwrap_or(TERM_BG) != run_bg {
                        break;
                    }
                    end += 1;
                }
                if run_bg != TERM_BG {
                    let px = x as f32 * cell_w;
                    let py = y as f32 * cell_h;
                    let pw = (end - x) as f32 * cell_w;
                    out.push_str(&format!(
                        r#"<rect x="{px:.2}" y="{py:.2}" width="{pw:.2}" height="{cell_h:.2}" fill="rgb({},{},{})"/>"#,
                        run_bg.0, run_bg.1, run_bg.2
                    ));
                    out.push('\n');
                }
                x = end;
            }
            x = 0;
            while x < self.cols {
                let cell = self.cells[self.idx(x, y)];
                let cw = UnicodeWidthChar::width(cell.ch).unwrap_or(1).max(1);
                if cell.ch != ' ' && cell.ch != '\0' {
                    let d = trace::glyph_path(cell.ch);
                    if !d.is_empty() {
                        let (fr, fg, fb) = cell.style.fg.unwrap_or(TERM_FG);
                        let tx = x as f32 * cell_w;
                        let ty = y as f32 * cell_h + m.ascender;
                        let stroke = if cell.style.bold {
                            format!(
                                r#" stroke="rgb({fr},{fg},{fb})" stroke-width="0.6" paint-order="stroke fill""#
                            )
                        } else {
                            String::new()
                        };
                        // Font outlines are y-up; SVG is y-down, so scale(s,-s) at the baseline.
                        out.push_str(&format!(
                            r#"<path transform="translate({tx:.2} {ty:.2}) scale({s:.5} {ns:.5})" d="{d}" fill="rgb({fr},{fg},{fb})"{stroke}/>"#,
                            s = m.scale,
                            ns = -m.scale,
                        ));
                        out.push('\n');
                    }
                }
                x += cw;
            }
        }
        out.push_str("</svg>\n");
        out
    }
}

fn paint(input: &str, cols: usize, rows: usize, styling: &Styling) -> Screen {
    let mut scr = Screen::new(cols, rows);
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b {
            if bytes.get(i + 1) == Some(&b'P') {
                if let Some((n, body)) = read_dcs(&bytes[i..]) {
                    apply_dcs(&mut scr, body, styling);
                    i += n;
                    continue;
                }
            }
            if bytes.get(i + 1) == Some(&b'[') {
                i = apply_csi(&mut scr, bytes, i);
                continue;
            }
            i += 1;
            continue;
        }
        let ch = input[i..].chars().next().unwrap();
        i += ch.len_utf8();
        scr.put(ch);
    }
    scr
}

fn read_dcs(bytes: &[u8]) -> Option<(usize, &str)> {
    if bytes.len() < 3 {
        return None;
    }
    let mut i = 2;
    while i + 1 < bytes.len() {
        if bytes[i] == 0x1b && bytes[i + 1] == b'\\' {
            let body = std::str::from_utf8(&bytes[2..i]).ok()?;
            return Some((i + 2, body));
        }
        i += 1;
    }
    None
}

fn apply_dcs(scr: &mut Screen, body: &str, styling: &Styling) {
    let body = body.strip_prefix('z').unwrap_or(body);
    let mut parts = body.split(';');
    let kind = parts.next().unwrap_or("");
    let rest: Vec<&str> = parts.collect();
    let (coords, payload) = split_coords(&rest);
    if let Some((x, y, _, _)) = coords {
        scr.x = x.min(scr.cols - 1);
        scr.y = y.min(scr.rows - 1);
    }
    let saved = scr.style;
    match kind {
        "text" => {
            if let Some(item) = payload.first().and_then(|p| parse_ui_item(p)) {
                let decl = if item.selected {
                    styling.text_selected
                } else {
                    styling.text_unselected
                };
                paint_ui_text(scr, &item, decl, styling, item.selected || item.opaque);
            }
        }
        "ribbon" => {
            if let Some(item) = payload.first().and_then(|p| parse_ui_item(p)) {
                let decl = if item.disabled {
                    styling.ribbon_unselected
                } else if item.selected {
                    styling.ribbon_selected
                } else {
                    styling.ribbon_unselected
                };
                // Zellij pads a space on each side of the ribbon label.
                let mut padded = item.clone();
                padded.text = format!(" {} ", item.text);
                padded.indices = shift_indices(&item.indices, 1);
                paint_ui_text(scr, &padded, decl, styling, true);
            }
        }
        "table" if payload.len() >= 2 => {
            let columns: usize = payload[0].parse().unwrap_or(1).max(1);
            let cells: Vec<UiItem> = payload[2..]
                .iter()
                .filter_map(|p| parse_ui_item(p))
                .collect();
            let start_x = scr.x;
            for (n, cell) in cells.iter().enumerate() {
                let row = n / columns;
                if n > 0 && n % columns == 0 {
                    scr.y = (scr.y + 1).min(scr.rows - 1);
                    scr.x = start_x;
                }
                if n % columns != 0 {
                    let gap = Style {
                        fg: Some(palette_rgb(styling.table_cell_unselected.base)),
                        bg: None,
                        bold: false,
                    };
                    let prev = scr.style;
                    scr.style = gap;
                    scr.put_str("  ");
                    scr.style = prev;
                }
                let decl = if row == 0 {
                    styling.table_title
                } else if cell.selected {
                    styling.table_cell_selected
                } else {
                    styling.table_cell_unselected
                };
                // Zellij only paints a table cell background when the cell
                // is selected or opaque. Title-row spaces must stay clear.
                paint_ui_text(scr, cell, decl, styling, cell.selected || cell.opaque);
            }
        }
        "nested_list" => {
            let start_x = scr.x;
            for (n, p) in payload.iter().enumerate() {
                let indent = p.chars().take_while(|c| *c == '|').count();
                if n > 0 {
                    scr.y = (scr.y + 1).min(scr.rows - 1);
                    scr.x = start_x;
                }
                if let Some(item) = parse_ui_item(&p[indent..]) {
                    let decl = if item.selected {
                        styling.list_selected
                    } else {
                        styling.list_unselected
                    };
                    let prev = scr.style;
                    scr.style = style_from_decl(decl, item.selected || item.opaque, true);
                    for _ in 0..indent {
                        scr.put_str("  ");
                    }
                    scr.style = prev;
                    paint_ui_text(scr, &item, decl, styling, item.selected || item.opaque);
                }
            }
        }
        _ => {}
    }
    scr.style = saved;
}

fn shift_indices(indices: &[Vec<usize>], by: usize) -> Vec<Vec<usize>> {
    indices
        .iter()
        .map(|level| level.iter().map(|i| i + by).collect())
        .collect()
}

fn paint_ui_text(
    scr: &mut Screen,
    item: &UiItem,
    decl: StyleDeclaration,
    styling: &Styling,
    fill_bg: bool,
) {
    let base = style_from_decl(decl, fill_bg, !item.disabled);
    if item.disabled {
        let mut s = base;
        // Zellij italics for disabled; we keep dimmer emphasis via base only.
        s.bold = false;
        let prev = scr.style;
        scr.style = s;
        scr.put_str(&item.text);
        scr.style = prev;
        return;
    }
    for (i, ch) in item.text.chars().enumerate() {
        let mut s = base;
        if let Some(fg) = style_of_index(item, i, decl, styling) {
            s.fg = Some(palette_rgb(fg));
        }
        if item.is_unbold(i) {
            s.bold = false;
        }
        let prev = scr.style;
        scr.style = s;
        scr.put(ch);
        scr.style = prev;
    }
}

fn style_from_decl(decl: StyleDeclaration, fill_bg: bool, bold: bool) -> Style {
    Style {
        fg: Some(palette_rgb(decl.base)),
        bg: if fill_bg {
            Some(palette_rgb(decl.background))
        } else {
            None
        },
        bold,
    }
}

fn style_of_index(
    item: &UiItem,
    index: usize,
    decl: StyleDeclaration,
    styling: &Styling,
) -> Option<PaletteColor> {
    const ERROR: usize = 6;
    const SUCCESS: usize = 7;
    if item.level_has(ERROR, index) {
        return Some(styling.exit_code_error.base);
    }
    if item.level_has(SUCCESS, index) {
        return Some(styling.exit_code_success.base);
    }
    let emph = [
        decl.emphasis_0,
        decl.emphasis_1,
        decl.emphasis_2,
        decl.emphasis_3,
    ];
    for i in (0..=3).rev() {
        if item.level_has(i, index) {
            return Some(emph[i]);
        }
    }
    Some(decl.base)
}

fn palette_rgb(c: PaletteColor) -> (u8, u8, u8) {
    match c {
        PaletteColor::Rgb((r, g, b)) => (r, g, b),
        PaletteColor::EightBit(n) => eightbit_to_rgb(n),
    }
}

#[derive(Clone, Debug)]
struct UiItem {
    text: String,
    selected: bool,
    opaque: bool,
    disabled: bool,
    indices: Vec<Vec<usize>>,
}

impl UiItem {
    fn level_has(&self, level: usize, index: usize) -> bool {
        self.indices
            .get(level)
            .map(|v| v.contains(&index))
            .unwrap_or(false)
    }
    fn is_unbold(&self, index: usize) -> bool {
        self.level_has(5, index)
    }
}

fn parse_ui_item(item: &str) -> Option<UiItem> {
    let mut s = item.to_string();
    let selected = take_flag(&mut s, 'x');
    let opaque = take_flag(&mut s, 'z');
    let disabled = take_flag(&mut s, 'd');
    let (indices, rest) = split_indices(&s);
    let nums: Result<Vec<u8>, _> = rest
        .split(',')
        .filter(|p| !p.is_empty())
        .map(|p| p.parse::<u8>())
        .collect();
    let nums = nums.ok()?;
    let text = if nums.is_empty() {
        String::new()
    } else {
        String::from_utf8(nums).ok()?
    };
    Some(UiItem {
        text,
        selected,
        opaque,
        disabled,
        indices,
    })
}

fn take_flag(s: &mut String, flag: char) -> bool {
    const FLAGS: [char; 3] = ['x', 'z', 'd'];
    let prefix = s.chars().take_while(|c| FLAGS.contains(c)).count();
    if let Some(pos) = s[..prefix].find(flag) {
        s.remove(pos);
        true
    } else {
        false
    }
}

fn split_indices(s: &str) -> (Vec<Vec<usize>>, String) {
    match s.rfind('$') {
        Some(pos) => {
            let head = &s[..=pos];
            let rest = s[pos + 1..].to_string();
            let indices = head
                .split('$')
                .map(|chunk| {
                    chunk
                        .split(',')
                        .filter(|p| !p.is_empty())
                        .filter_map(|p| p.parse().ok())
                        .collect()
                })
                .collect();
            (indices, rest)
        }
        None => (vec![], s.to_string()),
    }
}

type DcsCoords = (usize, usize, Option<usize>, Option<usize>);

fn split_coords<'a>(parts: &'a [&'a str]) -> (Option<DcsCoords>, &'a [&'a str]) {
    if let Some(first) = parts.first() {
        if first.contains('/') {
            let mut it = first.split('/');
            let x = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let y = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let w = it.next().and_then(|s| s.parse().ok());
            let h = it.next().and_then(|s| s.parse().ok());
            return (Some((x, y, w, h)), &parts[1..]);
        }
    }
    (None, parts)
}

fn apply_csi(scr: &mut Screen, bytes: &[u8], start: usize) -> usize {
    // start at ESC, ESC [
    let mut i = start + 2;
    let mut params = String::new();
    while i < bytes.len() {
        let b = bytes[i];
        i += 1;
        if b.is_ascii_alphabetic() {
            match b {
                b'm' => apply_sgr(scr, &params),
                b'H' | b'f' => {
                    let (r, c) = cup_params(&params);
                    scr.cup(r, c);
                }
                b'K' => {
                    let n = params.parse().unwrap_or(0);
                    scr.el(n);
                }
                b'A' => {
                    let n = params.parse().unwrap_or(1).max(1) as usize;
                    scr.y = scr.y.saturating_sub(n);
                }
                b'B' => {
                    let n = params.parse().unwrap_or(1).max(1) as usize;
                    scr.y = (scr.y + n).min(scr.rows - 1);
                }
                b'C' => {
                    let n = params.parse().unwrap_or(1).max(1) as usize;
                    scr.x = (scr.x + n).min(scr.cols - 1);
                }
                b'D' => {
                    let n = params.parse().unwrap_or(1).max(1) as usize;
                    scr.x = scr.x.saturating_sub(n);
                }
                b'J' => {
                    // clear from cursor / whole screen
                    let n: u32 = params.parse().unwrap_or(0);
                    if n == 2 || n == 3 {
                        *scr = Screen::new(scr.cols, scr.rows);
                    } else if n == 0 {
                        scr.el(0);
                        for y in (scr.y + 1)..scr.rows {
                            for x in 0..scr.cols {
                                let i = scr.idx(x, y);
                                scr.cells[i] = Cell {
                                    ch: ' ',
                                    style: Style::default(),
                                };
                            }
                        }
                    }
                }
                _ => {}
            }
            return i;
        }
        params.push(b as char);
    }
    i
}

fn cup_params(params: &str) -> (usize, usize) {
    let mut it = params.split(';');
    let r = it.next().and_then(|s| s.parse().ok()).unwrap_or(1).max(1);
    let c = it.next().and_then(|s| s.parse().ok()).unwrap_or(1).max(1);
    (r, c)
}

fn apply_sgr(scr: &mut Screen, params: &str) {
    if params.is_empty() || params == "0" {
        scr.style = Style::default();
        return;
    }
    let nums: Vec<i32> = params.split(';').filter_map(|p| p.parse().ok()).collect();
    let mut i = 0;
    while i < nums.len() {
        match nums[i] {
            0 => scr.style = Style::default(),
            1 => scr.style.bold = true,
            22 => scr.style.bold = false,
            39 => scr.style.fg = None,
            49 => scr.style.bg = None,
            38 if i + 2 < nums.len() && nums[i + 1] == 5 => {
                scr.style.fg = Some(bit_rgb(nums[i + 2]));
                i += 2;
            }
            48 if i + 2 < nums.len() && nums[i + 1] == 5 => {
                scr.style.bg = Some(bit_rgb(nums[i + 2]));
                i += 2;
            }
            38 if i + 4 < nums.len() && nums[i + 1] == 2 => {
                scr.style.fg = Some((
                    nums[i + 2].clamp(0, 255) as u8,
                    nums[i + 3].clamp(0, 255) as u8,
                    nums[i + 4].clamp(0, 255) as u8,
                ));
                i += 4;
            }
            48 if i + 4 < nums.len() && nums[i + 1] == 2 => {
                scr.style.bg = Some((
                    nums[i + 2].clamp(0, 255) as u8,
                    nums[i + 3].clamp(0, 255) as u8,
                    nums[i + 4].clamp(0, 255) as u8,
                ));
                i += 4;
            }
            n @ 30..=37 => scr.style.fg = Some(bit_rgb(n - 30)),
            n @ 40..=47 => scr.style.bg = Some(bit_rgb(n - 40)),
            n @ 90..=97 => scr.style.fg = Some(bit_rgb(n - 90 + 8)),
            n @ 100..=107 => scr.style.bg = Some(bit_rgb(n - 100 + 8)),
            _ => {}
        }
        i += 1;
    }
}

fn bit_rgb(n: i32) -> (u8, u8, u8) {
    eightbit_to_rgb(n.clamp(0, 255) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zellij_utils::data::DEFAULT_STYLES;

    #[test]
    fn ribbon_color_range_uses_emphasis_on_theme() {
        // `0$103` is color_range(0) on byte 'g'
        let dcs = "\u{1b}Pzribbon;0$103\u{1b}\\";
        let scr = paint(dcs, 8, 1, &DEFAULT_STYLES);
        let g = scr.cells.iter().find(|c| c.ch == 'g').expect("g");
        let expected_fg = palette_rgb(DEFAULT_STYLES.ribbon_unselected.emphasis_0);
        let expected_bg = palette_rgb(DEFAULT_STYLES.ribbon_unselected.background);
        assert_eq!(g.style.fg, Some(expected_fg));
        assert_eq!(g.style.bg, Some(expected_bg));
        assert!(g.style.bold);
    }

    #[test]
    fn selected_ribbon_uses_ribbon_selected_slot() {
        let dcs = "\u{1b}Pzribbon;x80,65,78,69\u{1b}\\"; // "PANE"
        let scr = paint(dcs, 12, 1, &DEFAULT_STYLES);
        let p = scr.cells.iter().find(|c| c.ch == 'P').expect("P");
        assert_eq!(
            p.style.bg,
            Some(palette_rgb(DEFAULT_STYLES.ribbon_selected.background))
        );
        assert_eq!(
            p.style.fg,
            Some(palette_rgb(DEFAULT_STYLES.ribbon_selected.base))
        );
    }

    #[test]
    fn svg_traces_nerd_font_glyphs_as_paths() {
        let ansi = "\u{1b}[38;5;154m\u{1b}[48;5;16m\u{1b}[38;5;16m\u{1b}[48;5;154m Tab \u{1b}[38;5;154m\u{1b}[48;5;16m";
        let svg = ansi_svg(ansi, 8, 1, &DEFAULT_STYLES);
        assert!(
            svg.contains("<path "),
            "glyphs must be traced outlines: {svg}"
        );
        assert!(!svg.contains("<polygon"));
        assert!(!svg.contains("@font-face"));
        assert!(!svg.contains(''));
        assert!(!svg.contains(''));
        let paths = svg.matches("<path ").count();
        assert!(paths >= 5, "expected traced , T, a, b, ; got {paths}");
    }

    #[test]
    fn table_title_spaces_do_not_fill_background() {
        // 4-column title row of single spaces, as session-manager welcome emits.
        let dcs = "\u{1b}Pztable;0/0/80/3;4;1;32;32;32;32\u{1b}\\";
        let scr = paint(dcs, 16, 1, &DEFAULT_STYLES);
        for cell in &scr.cells {
            assert_eq!(cell.style.bg, None, "empty title cells must not light up");
        }
    }
}
