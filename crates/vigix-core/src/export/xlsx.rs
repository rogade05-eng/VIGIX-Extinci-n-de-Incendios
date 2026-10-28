//! Libros XLSX (Office Open XML) sin dependencias: hojas con celdas numéricas, de texto y
//! booleanas, encabezados en negrita y primera fila inmovilizada. Los textos son `inlineStr`
//! (nunca fórmulas), de modo que no hay inyección de fórmulas. Los caracteres que XML no admite se
//! eliminan y se **informan** (contrato 9.2: contenido omitido o simplificado).

use super::zip::ZipWriter;
use crate::json::fmt_num;

#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Empty,
    Num(f64),
    Str(String),
    Bool(bool),
}

impl Cell {
    pub fn s<S: Into<String>>(v: S) -> Cell {
        Cell::Str(v.into())
    }
    pub fn n(v: f64) -> Cell {
        if v.is_finite() {
            Cell::Num(v)
        } else {
            Cell::Empty
        }
    }
}

#[derive(Clone, Debug)]
pub struct Sheet {
    pub name: String,
    pub rows: Vec<Vec<Cell>>,
    /// La primera fila es un encabezado (negrita y fila inmovilizada).
    pub header: bool,
}

/// Nombre de hoja válido: máx. 31 caracteres, sin `[]:*?/\`, único y no vacío.
pub fn safe_sheet_name(name: &str, used: &[String]) -> String {
    let mut n: String = name
        .chars()
        .map(|c| if "[]:*?/\\".contains(c) || c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .trim_matches('\'')
        .to_string();
    if n.is_empty() {
        n = "Hoja".to_string();
    }
    let mut base: String = n.chars().take(31).collect();
    let mut k = 2;
    while used.iter().any(|u| u.eq_ignore_ascii_case(&base)) {
        let suffix = format!(" ({})", k);
        let keep = 31usize.saturating_sub(suffix.chars().count());
        base = format!("{}{}", n.chars().take(keep).collect::<String>(), suffix);
        k += 1;
    }
    base
}

fn xml_escape(s: &str, removed: &mut usize) -> String {
    let mut o = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\t' | '\n' | '\r' => o.push(c),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' || (0xD800..0xE000).contains(&(c as u32)) => *removed += 1,
            c => o.push(c),
        }
    }
    o
}

/// Referencia de columna A, B, ..., Z, AA...
pub fn col_name(mut i: usize) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (i % 26) as u8);
        if i < 26 {
            break;
        }
        i = i / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

/// Construye el libro. Devuelve los bytes y el número de caracteres no representables eliminados.
pub fn build(sheets: &[Sheet]) -> (Vec<u8>, usize) {
    let mut removed = 0usize;
    let mut z = ZipWriter::new();
    let mut ct = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/><Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>",
    );
    for i in 1..=sheets.len() {
        ct.push_str(&format!("<Override PartName=\"/xl/worksheets/sheet{}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>", i));
    }
    ct.push_str("</Types>");
    z.add("[Content_Types].xml", ct.as_bytes());
    z.add(
        "_rels/.rels",
        b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>",
    );
    let mut names: Vec<String> = Vec::new();
    let mut wb = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets>");
    let mut rels = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">");
    for (i, sh) in sheets.iter().enumerate() {
        let name = safe_sheet_name(&sh.name, &names);
        names.push(name.clone());
        wb.push_str(&format!("<sheet name=\"{}\" sheetId=\"{}\" r:id=\"rId{}\"/>", xml_escape(&name, &mut removed), i + 1, i + 1));
        rels.push_str(&format!("<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{}.xml\"/>", i + 1, i + 1));
        // anchos de columna aproximados por el contenido (máx. 60)
        let ncols = sh.rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let mut widths = vec![8.0f64; ncols];
        for r in &sh.rows {
            for (c, cell) in r.iter().enumerate() {
                let l = match cell {
                    Cell::Str(s) => s.chars().count() as f64,
                    Cell::Num(n) => fmt_num(*n).len() as f64,
                    _ => 0.0,
                };
                widths[c] = widths[c].max((l + 2.0).min(60.0));
            }
        }
        let mut x = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">");
        if sh.header && !sh.rows.is_empty() {
            x.push_str("<sheetViews><sheetView workbookViewId=\"0\"><pane ySplit=\"1\" topLeftCell=\"A2\" activePane=\"bottomLeft\" state=\"frozen\"/></sheetView></sheetViews>");
        }
        if ncols > 0 {
            x.push_str("<cols>");
            for (c, w) in widths.iter().enumerate() {
                x.push_str(&format!("<col min=\"{}\" max=\"{}\" width=\"{}\" customWidth=\"1\"/>", c + 1, c + 1, fmt_num(*w)));
            }
            x.push_str("</cols>");
        }
        x.push_str("<sheetData>");
        for (r, row) in sh.rows.iter().enumerate() {
            x.push_str(&format!("<row r=\"{}\">", r + 1));
            for (c, cell) in row.iter().enumerate() {
                let rf = format!("{}{}", col_name(c), r + 1);
                let style = if sh.header && r == 0 { " s=\"1\"" } else { "" };
                match cell {
                    Cell::Empty => {}
                    Cell::Num(n) => x.push_str(&format!("<c r=\"{}\"{}><v>{}</v></c>", rf, style, fmt_num(*n))),
                    Cell::Bool(b) => x.push_str(&format!("<c r=\"{}\"{} t=\"b\"><v>{}</v></c>", rf, style, if *b { 1 } else { 0 })),
                    Cell::Str(s) => x.push_str(&format!(
                        "<c r=\"{}\"{} t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                        rf,
                        style,
                        xml_escape(s, &mut removed)
                    )),
                }
            }
            x.push_str("</row>");
        }
        x.push_str("</sheetData></worksheet>");
        z.add(&format!("xl/worksheets/sheet{}.xml", i + 1), x.as_bytes());
    }
    wb.push_str("</sheets></workbook>");
    rels.push_str(&format!("<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/></Relationships>", sheets.len() + 1));
    z.add("xl/workbook.xml", wb.as_bytes());
    z.add("xl/_rels/workbook.xml.rels", rels.as_bytes());
    z.add(
        "xl/styles.xml",
        b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><fonts count=\"2\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font><font><b/><sz val=\"11\"/><name val=\"Calibri\"/></font></fonts><fills count=\"3\"><fill><patternFill patternType=\"none\"/></fill><fill><patternFill patternType=\"gray125\"/></fill><fill><patternFill patternType=\"solid\"><fgColor rgb=\"FFDDE3EA\"/><bgColor indexed=\"64\"/></patternFill></fill></fills><borders count=\"1\"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs><cellXfs count=\"2\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/><xf numFmtId=\"0\" fontId=\"1\" fillId=\"2\" borderId=\"0\" xfId=\"0\" applyFont=\"1\" applyFill=\"1\"/></cellXfs><cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles></styleSheet>",
    );
    (z.finish(), removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::zip::read_stored;

    #[test]
    fn referencias_de_columna() {
        assert_eq!(col_name(0), "A");
        assert_eq!(col_name(25), "Z");
        assert_eq!(col_name(26), "AA");
        assert_eq!(col_name(27), "AB");
        assert_eq!(col_name(701), "ZZ");
        assert_eq!(col_name(702), "AAA");
    }

    #[test]
    fn nombres_de_hoja_validos_y_unicos() {
        let used = vec!["Datos".to_string()];
        assert_eq!(safe_sheet_name("Datos", &used), "Datos (2)");
        assert_eq!(safe_sheet_name("a/b:c*d?[e]", &[]), "a b c d  e");
        assert_eq!(safe_sheet_name("", &[]), "Hoja");
        let long = "x".repeat(50);
        assert_eq!(safe_sheet_name(&long, &[]).chars().count(), 31);
        assert!(safe_sheet_name(&long, &[long.chars().take(31).collect()]).chars().count() <= 31);
    }

    #[test]
    fn libro_bien_formado_con_tipos_y_escape() {
        let sheets = vec![
            Sheet {
                name: "Resumen".into(),
                header: true,
                rows: vec![
                    vec![Cell::s("Campo"), Cell::s("Valor")],
                    vec![Cell::s("a<b & \"c\""), Cell::n(1.5)],
                    vec![Cell::s("=1+1"), Cell::Bool(true)],
                    vec![Cell::Empty, Cell::n(f64::NAN)],
                ],
            },
            Sheet { name: "Otra".into(), header: false, rows: vec![vec![Cell::n(-0.0000001), Cell::s("ñandú ✓")]] },
        ];
        let (bytes, removed) = build(&sheets);
        assert_eq!(removed, 0);
        let files = read_stored(&bytes).unwrap();
        let names: Vec<&str> = files.iter().map(|f| f.0.as_str()).collect();
        assert!(
            names.contains(&"[Content_Types].xml")
                && names.contains(&"xl/workbook.xml")
                && names.contains(&"xl/worksheets/sheet2.xml")
                && names.contains(&"xl/styles.xml")
        );
        let s1 = String::from_utf8(files.iter().find(|f| f.0 == "xl/worksheets/sheet1.xml").unwrap().1.clone()).unwrap();
        assert!(s1.contains("a&lt;b &amp; &quot;c&quot;"));
        assert!(s1.contains("<v>1.5</v>") && s1.contains("t=\"b\"") && s1.contains("state=\"frozen\""));
        assert!(s1.contains("=1+1"), "el texto con = se guarda como texto (inlineStr), no como fórmula");
        assert!(!s1.contains("<f>"));
        // el número no finito no se escribe como valor
        assert!(!s1.contains("NaN"));
        // determinista
        assert_eq!(build(&sheets).0, bytes);
    }

    #[test]
    fn caracteres_ilegales_se_eliminan_y_se_informan() {
        let sheets = vec![Sheet { name: "H".into(), header: false, rows: vec![vec![Cell::s("a\u{0}b\u{1}c")]] }];
        let (bytes, removed) = build(&sheets);
        assert_eq!(removed, 2);
        let files = read_stored(&bytes).unwrap();
        let s = String::from_utf8(files.iter().find(|f| f.0 == "xl/worksheets/sheet1.xml").unwrap().1.clone()).unwrap();
        assert!(s.contains(">abc<"));
    }
}
