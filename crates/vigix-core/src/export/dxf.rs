//! Escritor DXF ASCII formato R12 (AC1009), el más interoperable (AutoCAD, BricsCAD, LibreCAD...).
//!
//! * Unidades: metros (se declara en comentarios `999` porque R12 no tiene `$INSUNITS`).
//! * Capas con prefijo `VIGIX-`; símbolos genéricos como bloques (contrato 5.2: símbolos genéricos
//!   y editables).
//! * Texto no ASCII se codifica `\U+XXXX` y se **informa** el recuento (contenido simplificado).

use crate::geometry::Pt;
use std::collections::{BTreeMap, BTreeSet};

pub struct Dxf {
    layers: BTreeMap<String, u8>,
    ents: Vec<String>,
    blocks: BTreeSet<&'static str>,
    comments: Vec<String>,
    min: Pt,
    max: Pt,
    pub encoded_chars: usize,
}

impl Default for Dxf {
    fn default() -> Self {
        Self::new()
    }
}

fn fnum(x: f64) -> String {
    let s = format!("{:.6}", if x.is_finite() { x } else { 0.0 });
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s.is_empty() || s == "-" || s == "-0" {
        "0".to_string()
    } else {
        s
    }
}

/// Nombre de capa válido para DXF (sin `<>/\":;?*|=\``), en mayúsculas.
pub fn safe_layer(name: &str) -> String {
    let n: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '$' { c.to_ascii_uppercase() } else { '_' })
        .collect();
    let n: String = n.chars().take(60).collect();
    if n.is_empty() {
        "_".to_string()
    } else {
        n
    }
}

impl Dxf {
    pub fn new() -> Dxf {
        Dxf {
            layers: BTreeMap::new(),
            ents: Vec::new(),
            blocks: BTreeSet::new(),
            comments: Vec::new(),
            min: [f64::MAX, f64::MAX],
            max: [f64::MIN, f64::MIN],
            encoded_chars: 0,
        }
    }

    fn touch(&mut self, p: Pt) {
        self.min[0] = self.min[0].min(p[0]);
        self.min[1] = self.min[1].min(p[1]);
        self.max[0] = self.max[0].max(p[0]);
        self.max[1] = self.max[1].max(p[1]);
    }

    pub fn comment(&mut self, text: &str) {
        let t = self.ascii(text).replace(['\r', '\n'], " ");
        self.comments.push(t);
    }

    /// Codifica los caracteres no ASCII como `\U+XXXX` (admitido por el texto de AutoCAD).
    pub fn ascii(&mut self, s: &str) -> String {
        let mut o = String::with_capacity(s.len());
        for c in s.chars() {
            if c.is_ascii() && !c.is_ascii_control() {
                o.push(c);
            } else if c.is_ascii_control() {
                o.push(' ');
            } else {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    o.push_str(&format!("\\U+{:04X}", u));
                }
                self.encoded_chars += 1;
            }
        }
        o
    }

    pub fn layer(&mut self, name: &str, color: u8) -> String {
        let n = safe_layer(name);
        self.layers.entry(n.clone()).or_insert(color);
        n
    }

    pub fn line(&mut self, layer: &str, a: Pt, b: Pt) {
        let l = self.layer(layer, 7);
        self.touch(a);
        self.touch(b);
        self.ents.push(format!(
            "0\nLINE\n8\n{}\n10\n{}\n20\n{}\n30\n0.0\n11\n{}\n21\n{}\n31\n0.0\n",
            l,
            fnum(a[0]),
            fnum(a[1]),
            fnum(b[0]),
            fnum(b[1])
        ));
    }

    /// Polilínea R12 (POLYLINE/VERTEX/SEQEND). `width` > 0 la dibuja con ese ancho (muros).
    pub fn polyline(&mut self, layer: &str, pts: &[Pt], closed: bool, width: f64) {
        if pts.len() < 2 {
            return;
        }
        let l = self.layer(layer, 7);
        let mut s = format!("0\nPOLYLINE\n8\n{}\n66\n1\n70\n{}\n", l, if closed { 1 } else { 0 });
        if width > 0.0 {
            s.push_str(&format!("40\n{}\n41\n{}\n", fnum(width), fnum(width)));
        }
        for p in pts {
            self.touch(*p);
            s.push_str(&format!("0\nVERTEX\n8\n{}\n10\n{}\n20\n{}\n30\n0.0\n", l, fnum(p[0]), fnum(p[1])));
        }
        s.push_str(&format!("0\nSEQEND\n8\n{}\n", l));
        self.ents.push(s);
    }

    pub fn circle(&mut self, layer: &str, c: Pt, r: f64) {
        let l = self.layer(layer, 7);
        self.touch([c[0] - r, c[1] - r]);
        self.touch([c[0] + r, c[1] + r]);
        self.ents.push(format!("0\nCIRCLE\n8\n{}\n10\n{}\n20\n{}\n30\n0.0\n40\n{}\n", l, fnum(c[0]), fnum(c[1]), fnum(r)));
    }

    pub fn text(&mut self, layer: &str, at: Pt, height: f64, text: &str) {
        let l = self.layer(layer, 7);
        let t = self.ascii(text);
        self.touch(at);
        self.ents.push(format!(
            "0\nTEXT\n8\n{}\n10\n{}\n20\n{}\n30\n0.0\n40\n{}\n1\n{}\n7\nSTANDARD\n",
            l,
            fnum(at[0]),
            fnum(at[1]),
            fnum(height),
            t
        ));
    }

    /// Inserta un símbolo genérico (SPR, NOZ, HYD, EXT, SRC, TNK, PMP) en `at`.
    pub fn insert(&mut self, layer: &str, block: &'static str, at: Pt, scale: f64) {
        let l = self.layer(layer, 7);
        self.blocks.insert(block);
        self.touch(at);
        self.ents.push(format!(
            "0\nINSERT\n8\n{}\n2\n{}\n10\n{}\n20\n{}\n30\n0.0\n41\n{}\n42\n{}\n43\n{}\n50\n0.0\n",
            l,
            block,
            fnum(at[0]),
            fnum(at[1]),
            fnum(scale),
            fnum(scale),
            fnum(scale)
        ));
    }

    fn block_def(name: &str) -> String {
        let e = |s: &str| s.to_string();
        let body = match name {
            "VIGIX_SPR" => vec![e("0\nCIRCLE\n8\n0\n10\n0\n20\n0\n30\n0\n40\n0.15\n"), e("0\nLINE\n8\n0\n10\n-0.15\n20\n0\n30\n0\n11\n0.15\n21\n0\n31\n0\n"), e("0\nLINE\n8\n0\n10\n0\n20\n-0.15\n30\n0\n11\n0\n21\n0.15\n31\n0\n")],
            "VIGIX_NOZ" => vec![e("0\nCIRCLE\n8\n0\n10\n0\n20\n0\n30\n0\n40\n0.15\n"), e("0\nCIRCLE\n8\n0\n10\n0\n20\n0\n30\n0\n40\n0.05\n")],
            "VIGIX_HYD" => vec![e("0\nPOLYLINE\n8\n0\n66\n1\n70\n1\n0\nVERTEX\n8\n0\n10\n-0.2\n20\n-0.17\n30\n0\n0\nVERTEX\n8\n0\n10\n0.2\n20\n-0.17\n30\n0\n0\nVERTEX\n8\n0\n10\n0\n20\n0.2\n30\n0\n0\nSEQEND\n8\n0\n")],
            "VIGIX_EXT" => vec![e("0\nPOLYLINE\n8\n0\n66\n1\n70\n1\n0\nVERTEX\n8\n0\n10\n-0.15\n20\n-0.15\n30\n0\n0\nVERTEX\n8\n0\n10\n0.15\n20\n-0.15\n30\n0\n0\nVERTEX\n8\n0\n10\n0.15\n20\n0.15\n30\n0\n0\nVERTEX\n8\n0\n10\n-0.15\n20\n0.15\n30\n0\n0\nSEQEND\n8\n0\n"), e("0\nLINE\n8\n0\n10\n-0.15\n20\n-0.15\n30\n0\n11\n0.15\n21\n0.15\n31\n0\n"), e("0\nLINE\n8\n0\n10\n-0.15\n20\n0.15\n30\n0\n11\n0.15\n21\n-0.15\n31\n0\n")],
            "VIGIX_SRC" => vec![e("0\nCIRCLE\n8\n0\n10\n0\n20\n0\n30\n0\n40\n0.4\n"), e("0\nCIRCLE\n8\n0\n10\n0\n20\n0\n30\n0\n40\n0.25\n")],
            "VIGIX_TNK" => vec![e("0\nPOLYLINE\n8\n0\n66\n1\n70\n1\n0\nVERTEX\n8\n0\n10\n-0.4\n20\n-0.4\n30\n0\n0\nVERTEX\n8\n0\n10\n0.4\n20\n-0.4\n30\n0\n0\nVERTEX\n8\n0\n10\n0.4\n20\n0.4\n30\n0\n0\nVERTEX\n8\n0\n10\n-0.4\n20\n0.4\n30\n0\n0\nSEQEND\n8\n0\n"), e("0\nLINE\n8\n0\n10\n-0.4\n20\n0\n30\n0\n11\n0.4\n21\n0\n31\n0\n")],
            "VIGIX_PMP" => vec![e("0\nCIRCLE\n8\n0\n10\n0\n20\n0\n30\n0\n40\n0.3\n"), e("0\nLINE\n8\n0\n10\n0\n20\n0\n30\n0\n11\n0.3\n21\n0\n31\n0\n"), e("0\nLINE\n8\n0\n10\n0\n20\n0\n30\n0\n11\n-0.15\n21\n0.26\n31\n0\n"), e("0\nLINE\n8\n0\n10\n0\n20\n0\n30\n0\n11\n-0.15\n21\n-0.26\n31\n0\n")],
            _ => vec![],
        };
        format!("0\nBLOCK\n8\n0\n2\n{n}\n70\n0\n10\n0\n20\n0\n30\n0\n3\n{n}\n{b}0\nENDBLK\n8\n0\n", n = name, b = body.concat())
    }

    /// Devuelve el DXF y el número de caracteres codificados como `\U+XXXX`.
    pub fn finish(mut self) -> (String, usize) {
        let (mn, mx) = if self.min[0] > self.max[0] { ([0.0, 0.0], [1.0, 1.0]) } else { (self.min, self.max) };
        let mut o = String::new();
        for c in &self.comments {
            o.push_str(&format!("999\n{}\n", c));
        }
        o.push_str("0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1009\n");
        o.push_str(&format!(
            "9\n$EXTMIN\n10\n{}\n20\n{}\n30\n0.0\n9\n$EXTMAX\n10\n{}\n20\n{}\n30\n0.0\n0\nENDSEC\n",
            fnum(mn[0]),
            fnum(mn[1]),
            fnum(mx[0]),
            fnum(mx[1])
        ));
        o.push_str("0\nSECTION\n2\nTABLES\n0\nTABLE\n2\nLTYPE\n70\n1\n0\nLTYPE\n2\nCONTINUOUS\n70\n0\n3\nSolid line\n72\n65\n73\n0\n40\n0.0\n0\nENDTAB\n");
        self.layers.entry("0".to_string()).or_insert(7);
        o.push_str(&format!("0\nTABLE\n2\nLAYER\n70\n{}\n", self.layers.len()));
        for (name, color) in &self.layers {
            o.push_str(&format!("0\nLAYER\n2\n{}\n70\n0\n62\n{}\n6\nCONTINUOUS\n", name, color));
        }
        o.push_str("0\nENDTAB\n0\nTABLE\n2\nSTYLE\n70\n1\n0\nSTYLE\n2\nSTANDARD\n70\n0\n40\n0.0\n41\n1.0\n50\n0.0\n71\n0\n42\n0.2\n3\ntxt\n4\n\n0\nENDTAB\n0\nENDSEC\n");
        o.push_str("0\nSECTION\n2\nBLOCKS\n");
        for b in &self.blocks {
            o.push_str(&Dxf::block_def(b));
        }
        o.push_str("0\nENDSEC\n0\nSECTION\n2\nENTITIES\n");
        for e in &self.ents {
            o.push_str(e);
        }
        o.push_str("0\nENDSEC\n0\nEOF\n");
        (o, self.encoded_chars)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estructura_basica_y_capas_seguras() {
        let mut d = Dxf::new();
        d.comment("Proyecto: Prueba ñandú");
        d.polyline("vigix-arq muros", &[[0.0, 0.0], [5.0, 0.0], [5.0, 3.0]], false, 0.2);
        d.polyline("zonas", &[[0.0, 0.0], [5.0, 0.0], [5.0, 3.0], [0.0, 3.0]], true, 0.0);
        d.line("l", [0.0, 0.0], [1.0, 1.0]);
        d.circle("c", [1.0, 1.0], 0.5);
        d.text("t", [0.0, 0.0], 0.25, "Zona «A» 5 m²");
        d.insert("dev", "VIGIX_SPR", [2.0, 2.0], 1.0);
        let (s, enc) = d.finish();
        assert!(enc > 0);
        assert!(s.starts_with("999\n"));
        assert!(s.contains("0\nSECTION\n2\nBLOCKS\n0\nBLOCK\n8\n0\n2\nVIGIX_SPR"));
        assert!(s.contains("2\nVIGIX-ARQ_MUROS\n") && s.contains("\\U+00AB"), "capa saneada y texto codificado");
        assert!(s.ends_with("0\nEOF\n"));
        assert!(s.is_ascii(), "el DXF R12 debe ser ASCII puro");
        assert_eq!(safe_layer("a<b>/c:d"), "A_B__C_D");
        assert_eq!(safe_layer(""), "_");
    }

    #[test]
    fn determinista() {
        let build = || {
            let mut d = Dxf::new();
            d.line("a", [0.0, 0.0], [1.0, 2.0]);
            d.finish().0
        };
        assert_eq!(build(), build());
    }
}
