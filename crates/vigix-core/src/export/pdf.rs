//! Escritor PDF sin dependencias: texto Helvetica (WinAnsi), líneas, rectángulos, tablas con
//! salto de página y cabecera repetida, banner de estado y marcas de agua en cada página.
//!
//! * Los caracteres que WinAnsi no puede representar se sustituyen (p. ej. `₂` -> `2`, `→` -> `->`)
//!   y se **cuentan** para informarlo (contrato 9.2: contenido omitido o simplificado).
//! * Determinista: sin fecha de creación incrustada ni identificadores aleatorios.

// Las primitivas de dibujo (página, coordenadas, grosor, color...) tienen naturalmente muchos parámetros.
#![allow(clippy::too_many_arguments)]

use std::fmt::Write as _;

pub type Rgb = (f64, f64, f64);

pub const BLACK: Rgb = (0.0, 0.0, 0.0);
pub const WHITE: Rgb = (1.0, 1.0, 1.0);
pub const GRAY: Rgb = (0.45, 0.45, 0.45);
pub const LIGHT: Rgb = (0.93, 0.94, 0.96);
pub const RED: Rgb = (0.75, 0.1, 0.1);
pub const AMBER: Rgb = (0.85, 0.55, 0.05);
pub const BLUE: Rgb = (0.1, 0.3, 0.65);
pub const GREEN: Rgb = (0.1, 0.5, 0.25);

pub const A4: (f64, f64) = (595.276, 841.89);
pub const A4_LANDSCAPE: (f64, f64) = (841.89, 595.276);

// Anchos Helvetica (AFM) para los códigos 32..=126, en milésimas de em.
const HELV: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278,
    278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722,
    667, 944, 667, 667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556,
    556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];
const HELV_B: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 333,
    333, 584, 584, 584, 611, 975, 722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667, 611, 722,
    667, 944, 667, 667, 611, 333, 278, 333, 584, 556, 333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611, 611,
    611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,
];

/// Codifica un carácter Unicode en WinAnsi (cp1252). `None` si no es representable.
fn winansi(c: char) -> Option<u8> {
    let u = c as u32;
    match u {
        0x20..=0x7E => Some(u as u8),
        0xA0..=0xFF => Some(u as u8),
        0x20AC => Some(0x80),
        0x2026 => Some(0x85),
        0x2018 => Some(0x91),
        0x2019 => Some(0x92),
        0x201C => Some(0x93),
        0x201D => Some(0x94),
        0x2022 => Some(0x95),
        0x2013 => Some(0x96),
        0x2014 => Some(0x97),
        0x2122 => Some(0x99),
        _ => None,
    }
}

/// Sustituciones legibles antes de recurrir a `?`.
fn simplify(c: char) -> Option<&'static str> {
    Some(match c {
        '₀' => "0",
        '₁' => "1",
        '₂' => "2",
        '₃' => "3",
        '→' => "->",
        '←' => "<-",
        '≥' => ">=",
        '≤' => "<=",
        '≈' => "~",
        '✓' => "OK",
        '⛔' => "[X]",
        '⚠' => "[!]",
        'ℹ' => "[i]",
        '‑' | '−' => "-",
        '\u{2009}' | '\u{202F}' => " ",
        '≠' => "!=",
        'Δ' => "Delta",
        'Σ' => "Suma",
        _ => return None,
    })
}

/// Texto -> bytes WinAnsi. Devuelve (bytes, nº de caracteres sustituidos).
pub fn encode_text(s: &str) -> (Vec<u8>, usize) {
    let mut out = Vec::with_capacity(s.len());
    let mut replaced = 0;
    for c in s.chars() {
        if c == '\n' || c == '\r' || c == '\t' {
            out.push(b' ');
        } else if let Some(b) = winansi(c) {
            out.push(b);
        } else if let Some(r) = simplify(c) {
            out.extend_from_slice(r.as_bytes());
            replaced += 1;
        } else {
            out.push(b'?');
            replaced += 1;
        }
    }
    (out, replaced)
}

fn glyph_width(b: u8, bold: bool) -> f64 {
    let t = if bold { &HELV_B } else { &HELV };
    match b {
        32..=126 => t[(b - 32) as usize] as f64,
        0x80 => 556.0,
        0x85 => 1000.0,
        0x91 | 0x92 => 222.0,
        0x93 | 0x94 => 333.0,
        0x95 => 350.0,
        0x96 => 556.0,
        0x97 => 1000.0,
        0x99 => 1000.0,
        // Letras acentuadas Latin-1: ancho de la letra base (aprox.)
        0xC0..=0xC5 => t[(b'A' - 32) as usize] as f64,
        0xC7 => t[(b'C' - 32) as usize] as f64,
        0xC8..=0xCB => t[(b'E' - 32) as usize] as f64,
        0xCC..=0xCF => t[(b'I' - 32) as usize] as f64,
        0xD1 => t[(b'N' - 32) as usize] as f64,
        0xD2..=0xD6 => t[(b'O' - 32) as usize] as f64,
        0xD9..=0xDC => t[(b'U' - 32) as usize] as f64,
        0xE0..=0xE5 => t[(b'a' - 32) as usize] as f64,
        0xE7 => t[(b'c' - 32) as usize] as f64,
        0xE8..=0xEB => t[(b'e' - 32) as usize] as f64,
        0xEC..=0xEF => t[(b'i' - 32) as usize] as f64,
        0xF1 => t[(b'n' - 32) as usize] as f64,
        0xF2..=0xF6 => t[(b'o' - 32) as usize] as f64,
        0xF9..=0xFC => t[(b'u' - 32) as usize] as f64,
        0xB0 => 400.0,
        0xB1 => 584.0,
        0xB2 | 0xB3 => 333.0,
        0xB5 => 556.0,
        0xBF | 0xA1 => 611.0,
        0xAB | 0xBB => 556.0,
        0xD7 => 584.0,
        _ => 556.0,
    }
}

/// Ancho de un texto en puntos.
pub fn text_width(s: &str, size: f64, bold: bool) -> f64 {
    let (bytes, _) = encode_text(s);
    bytes.iter().map(|b| glyph_width(*b, bold)).sum::<f64>() * size / 1000.0
}

fn pdf_string(bytes: &[u8]) -> String {
    let mut o = String::with_capacity(bytes.len() + 2);
    o.push('(');
    for &b in bytes {
        match b {
            b'(' => o.push_str("\\("),
            b')' => o.push_str("\\)"),
            b'\\' => o.push_str("\\\\"),
            32..=126 => o.push(b as char),
            _ => {
                let _ = write!(o, "\\{:03o}", b);
            }
        }
    }
    o.push(')');
    o
}

fn utf16_hex(s: &str) -> String {
    let mut o = String::from("<FEFF");
    for u in s.encode_utf16() {
        let _ = write!(o, "{:04X}", u);
    }
    o.push('>');
    o
}

fn fnum(x: f64) -> String {
    let s = format!("{:.2}", if x.is_finite() { x } else { 0.0 });
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s.is_empty() || s == "-0" {
        "0".to_string()
    } else {
        s
    }
}

fn color_op(c: Rgb, stroke: bool) -> String {
    format!("{} {} {} {}\n", fnum(c.0 * 100.0 / 100.0), fnum(c.1), fnum(c.2), if stroke { "RG" } else { "rg" }).replace("  ", " ")
}

struct Page {
    w: f64,
    h: f64,
    ops: String,
}

pub struct Pdf {
    pages: Vec<Page>,
    pub replaced_chars: usize,
}

impl Default for Pdf {
    fn default() -> Self {
        Self::new()
    }
}

impl Pdf {
    pub fn new() -> Pdf {
        Pdf { pages: Vec::new(), replaced_chars: 0 }
    }

    pub fn add_page(&mut self, size: (f64, f64)) -> usize {
        self.pages.push(Page { w: size.0, h: size.1, ops: String::new() });
        self.pages.len() - 1
    }
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }
    pub fn page_size(&self, p: usize) -> (f64, f64) {
        (self.pages[p].w, self.pages[p].h)
    }

    pub fn text(&mut self, p: usize, x: f64, y: f64, size: f64, bold: bool, color: Rgb, s: &str) {
        let (bytes, rep) = encode_text(s);
        self.replaced_chars += rep;
        let page = &mut self.pages[p];
        page.ops.push_str(&color_op(color, false));
        let _ = writeln!(
            page.ops,
            "BT /{} {} Tf {} {} Td {} Tj ET",
            if bold { "F2" } else { "F1" },
            fnum(size),
            fnum(x),
            fnum(y),
            pdf_string(&bytes)
        );
    }

    pub fn text_rotated(&mut self, p: usize, x: f64, y: f64, size: f64, angle_deg: f64, bold: bool, color: Rgb, s: &str) {
        let (bytes, rep) = encode_text(s);
        self.replaced_chars += rep;
        let (sn, cs) = (angle_deg.to_radians().sin(), angle_deg.to_radians().cos());
        let page = &mut self.pages[p];
        page.ops.push_str(&color_op(color, false));
        let _ = writeln!(
            page.ops,
            "BT /{} {} Tf {:.4} {:.4} {:.4} {:.4} {} {} Tm {} Tj ET",
            if bold { "F2" } else { "F1" },
            fnum(size),
            cs,
            sn,
            -sn,
            cs,
            fnum(x),
            fnum(y),
            pdf_string(&bytes)
        );
    }

    pub fn line(&mut self, p: usize, x1: f64, y1: f64, x2: f64, y2: f64, width: f64, color: Rgb) {
        let page = &mut self.pages[p];
        page.ops.push_str(&color_op(color, true));
        let _ = writeln!(page.ops, "{} w {} {} m {} {} l S", fnum(width), fnum(x1), fnum(y1), fnum(x2), fnum(y2));
    }

    pub fn polyline(&mut self, p: usize, pts: &[(f64, f64)], closed: bool, width: f64, color: Rgb, fill: Option<Rgb>) {
        if pts.len() < 2 {
            return;
        }
        let page = &mut self.pages[p];
        let mut s = String::new();
        if let Some(f) = fill {
            s.push_str(&color_op(f, false));
        }
        s.push_str(&color_op(color, true));
        let _ = write!(s, "{} w {} {} m ", fnum(width), fnum(pts[0].0), fnum(pts[0].1));
        for q in &pts[1..] {
            let _ = write!(s, "{} {} l ", fnum(q.0), fnum(q.1));
        }
        if closed {
            s.push_str("h ");
        }
        s.push_str(match (closed, fill.is_some()) {
            (true, true) => "B\n",
            (true, false) => "S\n",
            (false, _) => "S\n",
        });
        page.ops.push_str(&s);
    }

    pub fn rect(&mut self, p: usize, x: f64, y: f64, w: f64, h: f64, stroke: Option<(f64, Rgb)>, fill: Option<Rgb>) {
        let page = &mut self.pages[p];
        let mut s = String::new();
        if let Some(f) = fill {
            s.push_str(&color_op(f, false));
        }
        if let Some((lw, c)) = stroke {
            s.push_str(&color_op(c, true));
            let _ = write!(s, "{} w ", fnum(lw));
        }
        let _ = write!(s, "{} {} {} {} re ", fnum(x), fnum(y), fnum(w), fnum(h));
        s.push_str(match (stroke.is_some(), fill.is_some()) {
            (true, true) => "B\n",
            (true, false) => "S\n",
            (false, true) => "f\n",
            _ => "n\n",
        });
        page.ops.push_str(&s);
    }

    pub fn circle(&mut self, p: usize, cx: f64, cy: f64, r: f64, width: f64, color: Rgb, fill: Option<Rgb>) {
        // 4 curvas de Bézier
        let k = 0.5522847498 * r;
        let page = &mut self.pages[p];
        let mut s = String::new();
        if let Some(f) = fill {
            s.push_str(&color_op(f, false));
        }
        s.push_str(&color_op(color, true));
        let _ = writeln!(
            s,
            "{} w {} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {}",
            fnum(width),
            fnum(cx + r),
            fnum(cy),
            fnum(cx + r),
            fnum(cy + k),
            fnum(cx + k),
            fnum(cy + r),
            fnum(cx),
            fnum(cy + r),
            fnum(cx - k),
            fnum(cy + r),
            fnum(cx - r),
            fnum(cy + k),
            fnum(cx - r),
            fnum(cy),
            fnum(cx - r),
            fnum(cy - k),
            fnum(cx - k),
            fnum(cy - r),
            fnum(cx),
            fnum(cy - r),
            fnum(cx + k),
            fnum(cy - r),
            fnum(cx + r),
            fnum(cy - k),
            fnum(cx + r),
            fnum(cy),
            if fill.is_some() { "B" } else { "S" }
        );
        page.ops.push_str(&s);
    }

    /// Inserta operaciones al **principio** de la página (queda detrás del contenido): marcas de agua.
    pub fn prepend_text_rotated(&mut self, p: usize, x: f64, y: f64, size: f64, angle_deg: f64, color: Rgb, s: &str) {
        let before = std::mem::take(&mut self.pages[p].ops);
        self.text_rotated(p, x, y, size, angle_deg, true, color, s);
        let wm = std::mem::take(&mut self.pages[p].ops);
        self.pages[p].ops = format!("{}{}", wm, before);
    }

    /// Serializa el PDF. `title`/`subject`/`keywords` van en el diccionario Info (UTF-16BE).
    pub fn finish(self, title: &str, producer: &str, subject: &str, keywords: &str) -> Vec<u8> {
        let n = self.pages.len().max(1);
        let mut objs: Vec<Vec<u8>> = Vec::new(); // objeto i+1
        let catalog = b"<< /Type /Catalog /Pages 2 0 R >>".to_vec();
        let kids: String = (0..n).map(|i| format!("{} 0 R", 5 + 2 * i)).collect::<Vec<_>>().join(" ");
        objs.push(catalog);
        objs.push(format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids, n).into_bytes());
        objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec());
        objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>".to_vec());
        let pages: Vec<Page> = if self.pages.is_empty() { vec![Page { w: A4.0, h: A4.1, ops: String::new() }] } else { self.pages };
        for (i, pg) in pages.iter().enumerate() {
            objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {} 0 R >>", fnum(pg.w), fnum(pg.h), 6 + 2 * i).into_bytes());
            let mut c = format!("<< /Length {} >>\nstream\n", pg.ops.len()).into_bytes();
            c.extend_from_slice(pg.ops.as_bytes());
            c.extend_from_slice(b"\nendstream");
            objs.push(c);
        }
        let info_id = objs.len() + 1;
        objs.push(
            format!(
                "<< /Title {} /Producer {} /Subject {} /Keywords {} >>",
                utf16_hex(title),
                utf16_hex(producer),
                utf16_hex(subject),
                utf16_hex(keywords)
            )
            .into_bytes(),
        );
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for off in offsets {
            out.extend_from_slice(format!("{:010} 00000 n \n", off).as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size {} /Root 1 0 R /Info {} 0 R >>\nstartxref\n{}\n%%EOF\n", objs.len() + 1, info_id, xref).as_bytes(),
        );
        out
    }
}

// =============================================================================== diseño
/// Marco de página con banner de estado en cada página y pie con numeración.
pub struct Frame {
    pub banner: String,
    pub banner_color: Rgb,
    pub footer_left: String,
    pub page_word: String,
    pub of_word: String,
    pub watermark: Option<(String, Rgb)>,
    pub eval_mark: Option<String>,
}

/// Motor de flujo para texto y tablas con saltos de página.
pub struct Layout {
    pub pdf: Pdf,
    pub frame: Frame,
    page: usize,
    y: f64,
    pub margin: f64,
    top_band: f64,
    bottom_band: f64,
    size: (f64, f64),
}

impl Layout {
    pub fn new(frame: Frame) -> Layout {
        let mut l = Layout { pdf: Pdf::new(), frame, page: 0, y: 0.0, margin: 40.0, top_band: 34.0, bottom_band: 30.0, size: A4 };
        l.new_page(A4);
        l
    }

    pub fn new_page(&mut self, size: (f64, f64)) -> usize {
        self.size = size;
        self.page = self.pdf.add_page(size);
        self.y = size.1 - self.margin - self.top_band;
        self.page
    }

    pub fn content_width(&self) -> f64 {
        self.size.0 - 2.0 * self.margin
    }
    pub fn page_index(&self) -> usize {
        self.page
    }
    pub fn cursor(&self) -> f64 {
        self.y
    }
    pub fn set_cursor(&mut self, y: f64) {
        self.y = y;
    }
    pub fn size(&self) -> (f64, f64) {
        self.size
    }
    fn bottom(&self) -> f64 {
        self.margin + self.bottom_band
    }

    fn ensure(&mut self, h: f64) {
        if self.y - h < self.bottom() {
            let s = self.size;
            self.new_page(s);
        }
    }

    pub fn spacer(&mut self, h: f64) {
        self.y -= h;
    }

    /// Parte `s` en líneas que caben en `width` (rompe palabras largas por carácter).
    pub fn wrap(s: &str, size: f64, bold: bool, width: f64) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        for para in s.split('\n') {
            let mut cur = String::new();
            for word in para.split(' ') {
                let candidate = if cur.is_empty() { word.to_string() } else { format!("{} {}", cur, word) };
                if text_width(&candidate, size, bold) * 1.03 <= width {
                    cur = candidate;
                    continue;
                }
                if !cur.is_empty() {
                    lines.push(std::mem::take(&mut cur));
                }
                // palabra más ancha que la línea: partir por caracteres
                let mut chunk = String::new();
                for ch in word.chars() {
                    let t = format!("{}{}", chunk, ch);
                    if text_width(&t, size, bold) * 1.03 > width && !chunk.is_empty() {
                        lines.push(std::mem::take(&mut chunk));
                    }
                    chunk.push(ch);
                }
                cur = chunk;
            }
            lines.push(cur);
        }
        lines
    }

    pub fn heading(&mut self, text: &str, level: u8) {
        let (size, before, after) = match level {
            1 => (16.0, 14.0, 6.0),
            2 => (12.5, 10.0, 4.0),
            _ => (10.5, 6.0, 3.0),
        };
        let lines = Self::wrap(text, size, true, self.content_width());
        self.ensure(before + after + size * 1.25 * lines.len() as f64 + 30.0);
        self.y -= before;
        for l in lines {
            self.y -= size;
            self.pdf.text(self.page, self.margin, self.y, size, true, BLACK, &l);
            self.y -= size * 0.25;
        }
        if level == 1 {
            self.pdf.line(self.page, self.margin, self.y - 2.0, self.margin + self.content_width(), self.y - 2.0, 0.8, GRAY);
        }
        self.y -= after;
    }

    pub fn paragraph(&mut self, text: &str, size: f64, bold: bool, color: Rgb) {
        let lines = Self::wrap(text, size, bold, self.content_width());
        for l in lines {
            self.ensure(size * 1.3);
            self.y -= size * 1.15;
            self.pdf.text(self.page, self.margin, self.y, size, bold, color, &l);
            self.y -= size * 0.15;
        }
        self.y -= 3.0;
    }

    pub fn bullet(&mut self, text: &str, size: f64) {
        let w = self.content_width() - 12.0;
        let lines = Self::wrap(text, size, false, w);
        for (i, l) in lines.iter().enumerate() {
            self.ensure(size * 1.3);
            self.y -= size * 1.15;
            if i == 0 {
                self.pdf.text(self.page, self.margin + 2.0, self.y, size, true, GRAY, "-");
            }
            self.pdf.text(self.page, self.margin + 12.0, self.y, size, false, BLACK, l);
            self.y -= size * 0.15;
        }
    }

    /// Tabla con anchos proporcionales al contenido (acotados), encabezado repetido y cebreado.
    pub fn table(&mut self, headers: &[String], rows: &[Vec<String>], size: f64) {
        let ncols = headers.len().max(1);
        let total_w = self.content_width();
        let pad = 3.0;
        // anchos por contenido
        let mut want = vec![0.0f64; ncols];
        for (c, h) in headers.iter().enumerate() {
            want[c] = text_width(h, size, true) + 2.0 * pad;
        }
        for r in rows.iter().take(400) {
            for (c, cell) in r.iter().enumerate().take(ncols) {
                let longest = cell.split('\n').map(|l| text_width(l, size, false)).fold(0.0, f64::max);
                want[c] = want[c].max(longest + 2.0 * pad);
            }
        }
        let min_w = 34.0;
        let mut widths: Vec<f64> = want.iter().map(|w| w.max(min_w)).collect();
        let sum: f64 = widths.iter().sum();
        if sum > total_w {
            // reparto: columnas cortas conservan su ancho, las largas se reducen proporcionalmente
            let mut fixed = 0.0;
            let mut flex = 0.0;
            for w in &widths {
                if *w <= total_w / ncols as f64 {
                    fixed += *w;
                } else {
                    flex += *w;
                }
            }
            let avail = (total_w - fixed).max(min_w);
            let k = if flex > 0.0 { avail / flex } else { 1.0 };
            for w in widths.iter_mut() {
                if *w > total_w / ncols as f64 {
                    *w = (*w * k).max(min_w);
                }
            }
            let s2: f64 = widths.iter().sum();
            if s2 > total_w {
                let k2 = total_w / s2;
                for w in widths.iter_mut() {
                    *w *= k2;
                }
            }
        } else {
            let extra = (total_w - sum) / ncols as f64;
            for w in widths.iter_mut() {
                *w += extra;
            }
        }
        let line_h = size * 1.2;
        let draw_header = |this: &mut Layout, widths: &[f64]| {
            let hh = line_h + 2.0 * pad;
            this.ensure(hh + line_h * 2.0);
            let top = this.y;
            this.pdf.rect(this.page, this.margin, top - hh, total_w, hh, None, Some(LIGHT));
            let mut x = this.margin;
            for (c, h) in headers.iter().enumerate() {
                let lines = Layout::wrap(h, size, true, widths[c] - 2.0 * pad);
                this.pdf.text(this.page, x + pad, top - pad - size, size, true, BLACK, lines.first().map(String::as_str).unwrap_or(""));
                x += widths[c];
            }
            this.pdf.line(this.page, this.margin, top - hh, this.margin + total_w, top - hh, 0.6, GRAY);
            this.y = top - hh;
        };
        draw_header(self, &widths);
        for (ri, r) in rows.iter().enumerate() {
            let cells: Vec<Vec<String>> =
                (0..ncols).map(|c| Layout::wrap(r.get(c).map(String::as_str).unwrap_or(""), size, false, widths[c] - 2.0 * pad)).collect();
            let nlines = cells.iter().map(|l| l.len()).max().unwrap_or(1).max(1);
            let rh = nlines as f64 * line_h + 2.0 * pad;
            if self.y - rh < self.bottom() {
                let s = self.size;
                self.new_page(s);
                draw_header(self, &widths);
            }
            let top = self.y;
            if ri % 2 == 1 {
                self.pdf.rect(self.page, self.margin, top - rh, total_w, rh, None, Some((0.975, 0.98, 0.985)));
            }
            let mut x = self.margin;
            for (c, lines) in cells.iter().enumerate() {
                for (li, l) in lines.iter().enumerate() {
                    self.pdf.text(self.page, x + pad, top - pad - size - li as f64 * line_h, size, false, BLACK, l);
                }
                x += widths[c];
            }
            self.pdf.line(self.page, self.margin, top - rh, self.margin + total_w, top - rh, 0.25, (0.8, 0.8, 0.8));
            self.y = top - rh;
        }
        self.y -= 6.0;
    }

    pub fn kv_table(&mut self, rows: &[(String, String)], size: f64) {
        let lw = (self.content_width() * 0.32).min(190.0);
        let vw = self.content_width() - lw;
        for (k, v) in rows {
            let kl = Self::wrap(k, size, true, lw - 6.0);
            let vl = Self::wrap(v, size, false, vw - 6.0);
            let n = kl.len().max(vl.len()).max(1);
            let rh = n as f64 * size * 1.2 + 6.0;
            self.ensure(rh);
            let top = self.y;
            self.pdf.rect(self.page, self.margin, top - rh, lw, rh, None, Some(LIGHT));
            for (i, l) in kl.iter().enumerate() {
                self.pdf.text(self.page, self.margin + 3.0, top - 3.0 - size - i as f64 * size * 1.2, size, true, BLACK, l);
            }
            for (i, l) in vl.iter().enumerate() {
                self.pdf.text(self.page, self.margin + lw + 3.0, top - 3.0 - size - i as f64 * size * 1.2, size, false, BLACK, l);
            }
            self.pdf.line(self.page, self.margin, top - rh, self.margin + self.content_width(), top - rh, 0.25, (0.8, 0.8, 0.8));
            self.y = top - rh;
        }
        self.y -= 6.0;
    }

    /// Estampa banner, marcas de agua y pie en todas las páginas y serializa.
    pub fn finish(mut self, title: &str, producer: &str, subject: &str, keywords: &str) -> (Vec<u8>, usize) {
        let total = self.pdf.page_count();
        for p in 0..total {
            let (w, h) = self.pdf.page_size(p);
            // banner superior
            let bh = 22.0;
            let c = self.frame.banner_color;
            let banner = self.frame.banner.clone();
            self.pdf.rect(p, 0.0, h - bh, w, bh, None, Some(c));
            let bs = if text_width(&banner, 9.5, true) > w - 30.0 { 7.5 } else { 9.5 };
            let banner = fit_text(&banner, bs, w - 30.0, true);
            let tw = text_width(&banner, bs, true);
            self.pdf.text(p, ((w - tw) / 2.0).max(8.0), h - bh + 7.0, bs, true, WHITE, &banner);
            // pie: el texto izquierdo se recorta para no pisar la numeración
            self.pdf.line(p, self.margin, self.margin + 16.0, w - self.margin, self.margin + 16.0, 0.4, GRAY);
            let pg = format!("{} {} {} {}", self.frame.page_word, p + 1, self.frame.of_word, total);
            let pw = text_width(&pg, 8.0, false);
            let fl = fit_text(&self.frame.footer_left.clone(), 7.5, (w - 2.0 * self.margin) - pw - 14.0, false);
            self.pdf.text(p, self.margin, self.margin + 5.0, 7.5, false, GRAY, &fl);
            self.pdf.text(p, w - self.margin - pw, self.margin + 5.0, 8.0, false, GRAY, &pg);
            // marcas de agua (detrás del contenido), centradas y contenidas en la página
            if let Some(m) = self.frame.eval_mark.clone() {
                self.watermark(p, &m, 44.0, (0.91, 0.91, 0.91), 0.5, 0.36);
            }
            if let Some((m, col)) = self.frame.watermark.clone() {
                let light = (1.0 - (1.0 - col.0) * 0.18, 1.0 - (1.0 - col.1) * 0.18, 1.0 - (1.0 - col.2) * 0.18);
                self.watermark(p, &m, 84.0, light, 0.5, 0.58);
            }
        }
        let replaced = self.pdf.replaced_chars;
        (self.pdf.finish(title, producer, subject, keywords), replaced)
    }

    /// Texto diagonal (35°) centrado en (`fx`,`fy`) como fracción de la página; reduce el cuerpo hasta
    /// que cabe en el 80 % del ancho y el 55 % del alto.
    fn watermark(&mut self, p: usize, text: &str, max_size: f64, color: Rgb, fx: f64, fy: f64) {
        let (w, h) = self.pdf.page_size(p);
        let ang = 35.0f64;
        let (c, s) = (ang.to_radians().cos(), ang.to_radians().sin());
        let mut size = max_size;
        loop {
            let tw = text_width(text, size, true);
            if (tw * c + size * s <= w * 0.8 && tw * s + size * c <= h * 0.55) || size <= 12.0 {
                break;
            }
            size -= 2.0;
        }
        let tw = text_width(text, size, true);
        // el punto de anclaje es el inicio de la línea base: se retrocede medio texto y se baja medio cuerpo
        let x = w * fx - (tw / 2.0) * c + (size * 0.33) * s;
        let y = h * fy - (tw / 2.0) * s - (size * 0.33) * c;
        self.pdf.prepend_text_rotated(p, x, y, size, ang, color, text);
    }
}

/// Recorta `s` con puntos suspensivos para que quepa en `max_w` puntos.
pub fn fit_text(s: &str, size: f64, max_w: f64, bold: bool) -> String {
    if text_width(s, size, bold) <= max_w {
        return s.to_string();
    }
    let mut out: Vec<char> = s.chars().collect();
    while !out.is_empty() {
        out.pop();
        let cand: String = out.iter().collect::<String>() + "...";
        if text_width(&cand, size, bold) <= max_w {
            return cand;
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codificacion_winansi_y_sustituciones() {
        let (b, r) = encode_text("Presión ñandú ¿qué? 5 m² 20 °C — “ok” €");
        assert_eq!(r, 0, "todo es representable");
        assert_eq!(b[8], 0xF1, "ñ = 0xF1");
        let (b2, r2) = encode_text("CO₂ → ≥ 5 ✓ 😀");
        assert_eq!(String::from_utf8_lossy(&b2), "CO2 -> >= 5 OK ?");
        assert_eq!(r2, 5);
    }

    #[test]
    fn anchos_conocidos_de_helvetica() {
        // "Hello" en Helvetica 10pt: H722 e556 l222 l222 o556 = 2278 -> 22.78
        assert!((text_width("Hello", 10.0, false) - 22.78).abs() < 1e-9);
        assert!((text_width("Hello", 10.0, true) - (722.0 + 556.0 + 278.0 + 278.0 + 611.0) * 0.01).abs() < 1e-9);
        assert!(text_width("ñ", 10.0, false) == text_width("n", 10.0, false));
        assert_eq!(text_width("", 12.0, false), 0.0);
    }

    #[test]
    fn ajuste_de_lineas() {
        let l = Layout::wrap("uno dos tres cuatro cinco seis siete ocho nueve diez", 10.0, false, 80.0);
        assert!(l.len() > 2);
        for s in &l {
            assert!(text_width(s, 10.0, false) <= 80.0 * 1.001, "{:?}", s);
        }
        // palabra larguísima (p. ej. una huella SHA-256) se parte por caracteres
        let h = "a".repeat(64);
        let l = Layout::wrap(&h, 8.0, false, 100.0);
        assert!(l.len() >= 3 && l.concat() == h);
        assert_eq!(Layout::wrap("a\nb", 10.0, false, 100.0), vec!["a", "b"]);
    }

    fn frame() -> Frame {
        Frame {
            banner: "BORRADOR / NO APTO PARA CONSTRUCCIÓN".into(),
            banner_color: RED,
            footer_left: "VIGIX | prueba".into(),
            page_word: "Página".into(),
            of_word: "de".into(),
            watermark: Some(("BORRADOR".into(), RED)),
            eval_mark: Some("EVALUACIÓN".into()),
        }
    }

    #[test]
    fn documento_multipagina_tiene_estructura_valida() {
        let mut l = Layout::new(frame());
        l.heading("Título principal", 1);
        l.paragraph("Texto de prueba con acentos: Presión, ñandú.", 10.0, false, BLACK);
        let headers = vec!["Id".to_string(), "Descripción".to_string(), "Valor".to_string()];
        let rows: Vec<Vec<String>> =
            (0..120).map(|i| vec![format!("R{}", i), "Descripción larga ".repeat(6), format!("{}", i * 3)]).collect();
        l.table(&headers, &rows, 8.0);
        let (pdf, replaced) = l.finish("Título ñ", "VIGIX 0.1.0", "asunto", "k1,k2");
        assert_eq!(replaced, 0);
        let txt = String::from_utf8_lossy(&pdf);
        assert!(txt.starts_with("%PDF-1.4"));
        assert!(txt.trim_end().ends_with("%%EOF"));
        let pages = txt.matches("/Type /Page ").count();
        assert!(pages >= 3, "120 filas deben ocupar varias páginas: {}", pages);
        assert!(txt.contains(&format!("/Count {}", pages)));
        // el banner y la numeración están en cada página
        assert!(txt.matches("Tj ET").count() > pages * 2);
        // xref coherente: el offset de startxref apunta a "xref"
        let sx = txt.rfind("startxref").unwrap();
        let off: usize = txt[sx + 10..].lines().next().unwrap().trim().parse().unwrap();
        assert_eq!(&pdf[off..off + 4], b"xref");
    }

    #[test]
    fn el_texto_largo_se_recorta_y_cabe() {
        let t = "VIGIX EXTINCIÓN FIJA | Edificio de ejemplo (DEMOSTRACIÓN — datos ficticios) | PERFIL DE DEMOSTRACIÓN — VALORES FICTICIOS, NO NORMATIVO";
        let r = fit_text(t, 7.5, 300.0, false);
        assert!(text_width(&r, 7.5, false) <= 300.0 && r.ends_with("..."), "{}", r);
        assert_eq!(fit_text("corto", 7.5, 300.0, false), "corto");
        assert_eq!(fit_text("x", 7.5, 0.5, false), "");
    }

    #[test]
    fn determinista() {
        let build = || {
            let mut l = Layout::new(frame());
            l.heading("X", 1);
            l.paragraph("hola", 10.0, false, BLACK);
            l.finish("t", "p", "s", "k").0
        };
        assert_eq!(build(), build());
    }
}
