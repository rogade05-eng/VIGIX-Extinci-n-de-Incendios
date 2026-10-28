//! CSV según RFC 4180 (lectura y escritura) con mitigación de inyección de fórmulas al exportar
//! (OWASP "CSV injection"): una celda de texto que empiece por `=`, `+`, `-` o `@` (y que no sea
//! un número) se escribe con un apóstrofo inicial.

/// Analiza un CSV. `delimiter` suele ser `,` o `;`. Tolera BOM y fin de línea CRLF/LF.
/// Devuelve error si hay comillas sin cerrar.
pub fn parse(text: &str, delimiter: char) -> Result<Vec<Vec<String>>, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    let mut any = false;
    while let Some(ch) = chars.next() {
        any = true;
        if in_quotes {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(ch);
            }
        } else if ch == '"' && field.is_empty() {
            in_quotes = true;
        } else if ch == delimiter {
            row.push(std::mem::take(&mut field));
        } else if ch == '\r' {
            // se ignora; el salto lo procesa '\n' (o un '\r' suelto)
            if chars.peek() != Some(&'\n') {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
        } else if ch == '\n' {
            row.push(std::mem::take(&mut field));
            rows.push(std::mem::take(&mut row));
        } else {
            field.push(ch);
        }
    }
    if in_quotes {
        return Err("comillas sin cerrar".to_string());
    }
    if any && (!field.is_empty() || !row.is_empty()) {
        row.push(field);
        rows.push(row);
    }
    // fila vacía final por saltos redundantes
    while rows.last().map(|r| r.len() == 1 && r[0].is_empty()).unwrap_or(false) {
        rows.pop();
    }
    Ok(rows)
}

fn looks_numeric(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty() && t.parse::<f64>().is_ok()
}

/// Escapa una celda de texto para CSV (comillas, delimitador, saltos de línea) y neutraliza fórmulas.
pub fn escape_cell(value: &str, delimiter: char, neutralize_formulas: bool) -> String {
    let mut v = value.to_string();
    if neutralize_formulas {
        if let Some(c) = v.chars().next() {
            if matches!(c, '=' | '+' | '-' | '@' | '\t' | '\r') && !looks_numeric(&v) {
                v.insert(0, '\'');
            }
        }
    }
    if v.contains(delimiter) || v.contains('"') || v.contains('\n') || v.contains('\r') {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v
    }
}

pub fn write(rows: &[Vec<String>], delimiter: char, neutralize_formulas: bool) -> String {
    let mut out = String::new();
    for r in rows {
        let cells: Vec<String> = r.iter().map(|c| escape_cell(c, delimiter, neutralize_formulas)).collect();
        out.push_str(&cells.join(&delimiter.to_string()));
        out.push_str("\r\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lectura_rfc4180() {
        let t = "a,b,c\r\n1,\"x,y\",\"dos \"\"comillas\"\"\"\r\n\"linea1\nlinea2\",,fin";
        let r = parse(t, ',').unwrap();
        assert_eq!(r.len(), 3);
        assert_eq!(r[1], vec!["1", "x,y", "dos \"comillas\""]);
        assert_eq!(r[2], vec!["linea1\nlinea2", "", "fin"]);
    }

    #[test]
    fn bom_punto_y_coma_y_filas_finales() {
        let r = parse("\u{feff}id;nombre\n1;Ñandú\n\n", ';').unwrap();
        assert_eq!(r, vec![vec!["id", "nombre"], vec!["1", "Ñandú"]]);
        assert!(parse("a,\"b", ',').is_err());
        assert_eq!(parse("", ',').unwrap().len(), 0);
    }

    #[test]
    fn escritura_y_ida_y_vuelta() {
        let rows = vec![vec!["id".to_string(), "desc".to_string()], vec!["1".to_string(), "con,coma \"y\" comillas\ny salto".to_string()]];
        let s = write(&rows, ',', true);
        assert_eq!(parse(&s, ',').unwrap(), rows);
        assert!(s.ends_with("\r\n"));
    }

    #[test]
    fn neutraliza_inyeccion_de_formulas_pero_no_numeros() {
        assert_eq!(escape_cell("=HYPERLINK(\"http://x\")", ',', true), "\"'=HYPERLINK(\"\"http://x\"\")\"");
        assert_eq!(escape_cell("+cmd|' /C calc'!A0", ',', true), "'+cmd|' /C calc'!A0");
        assert_eq!(escape_cell("@SUM(1)", ',', true), "'@SUM(1)");
        assert_eq!(escape_cell("-5.25", ',', true), "-5.25"); // número negativo legítimo
        assert_eq!(escape_cell("-texto", ',', true), "'-texto");
        assert_eq!(escape_cell("=1+1", ',', false), "=1+1");
        assert_eq!(escape_cell("normal", ',', true), "normal");
    }
}
