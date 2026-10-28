//! JSON mínimo, determinista y sin dependencias.
//!
//! * Los objetos usan `BTreeMap`: el orden de claves es siempre el mismo, de modo que
//!   la forma compacta es a la vez la forma **canónica** que se usa para huellas SHA-256
//!   (contrato 11.2: mismas entradas => misma huella).
//! * Los números son `f64`; los enteros exactos se escriben sin parte decimal.
//! * El analizador impone límites de tamaño y de profundidad (contrato 8.4: archivos
//!   corruptos y límites de memoria).

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Valor JSON.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

/// Error de análisis JSON con desplazamiento en bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct JsonError {
    pub msg: String,
    pub offset: usize,
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JSON inválido en el byte {}: {}", self.offset, self.msg)
    }
}

impl std::error::Error for JsonError {}

/// Límites del analizador.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_bytes: usize,
    pub max_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_bytes: 256 * 1024 * 1024,
            max_depth: 128,
        }
    }
}

static NULL: Json = Json::Null;

impl Json {
    // ----------------------------------------------------------------- análisis
    pub fn parse(s: &str) -> Result<Json, JsonError> {
        Json::parse_with_limits(s, Limits::default())
    }

    pub fn parse_with_limits(s: &str, limits: Limits) -> Result<Json, JsonError> {
        if s.len() > limits.max_bytes {
            return Err(JsonError {
                msg: format!(
                    "el documento excede el límite de {} bytes",
                    limits.max_bytes
                ),
                offset: 0,
            });
        }
        let mut p = Parser {
            b: s.as_bytes(),
            i: 0,
            max_depth: limits.max_depth,
        };
        // BOM UTF-8 tolerado.
        if p.b.starts_with(&[0xEF, 0xBB, 0xBF]) {
            p.i = 3;
        }
        p.ws();
        let v = p.value(0)?;
        p.ws();
        if p.i != p.b.len() {
            return Err(p.err("texto sobrante después del valor"));
        }
        Ok(v)
    }

    // ------------------------------------------------------------- constructores
    pub fn obj() -> Json {
        Json::Obj(BTreeMap::new())
    }
    pub fn arr() -> Json {
        Json::Arr(Vec::new())
    }
    /// Número; un valor no finito se convierte en `null` (JSON no puede representarlo).
    pub fn num(x: f64) -> Json {
        if x.is_finite() {
            Json::Num(x)
        } else {
            Json::Null
        }
    }
    pub fn str<S: Into<String>>(s: S) -> Json {
        Json::Str(s.into())
    }
    pub fn opt_num(x: Option<f64>) -> Json {
        match x {
            Some(v) => Json::num(v),
            None => Json::Null,
        }
    }
    pub fn opt_str(x: Option<&str>) -> Json {
        match x {
            Some(v) => Json::Str(v.to_string()),
            None => Json::Null,
        }
    }

    /// Inserta/reemplaza una clave (solo si `self` es objeto) y devuelve `self` para encadenar.
    pub fn with<K: Into<String>>(mut self, k: K, v: Json) -> Json {
        if let Json::Obj(m) = &mut self {
            m.insert(k.into(), v);
        }
        self
    }

    pub fn set<K: Into<String>>(&mut self, k: K, v: Json) {
        if let Json::Obj(m) = self {
            m.insert(k.into(), v);
        }
    }

    pub fn push(&mut self, v: Json) {
        if let Json::Arr(a) = self {
            a.push(v);
        }
    }

    // ----------------------------------------------------------------- acceso
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.get(key),
            _ => None,
        }
    }

    /// Como `get` pero devuelve `Null` si no existe (cómodo para encadenar).
    pub fn field(&self, key: &str) -> &Json {
        self.get(key).unwrap_or(&NULL)
    }

    pub fn idx(&self, i: usize) -> Option<&Json> {
        match self {
            Json::Arr(a) => a.get(i),
            _ => None,
        }
    }

    /// Ruta con puntos: `a.b.0.c` (los segmentos numéricos indexan arreglos).
    pub fn at(&self, path: &str) -> Option<&Json> {
        if path.is_empty() {
            return Some(self);
        }
        let mut cur = self;
        for seg in path.split('.') {
            cur = match cur {
                Json::Obj(m) => m.get(seg)?,
                Json::Arr(a) => a.get(seg.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
        Some(cur)
    }

    /// Asigna en una ruta con puntos, creando objetos intermedios. Devuelve false si el
    /// camino choca con un valor que no es objeto.
    pub fn set_at(&mut self, path: &str, v: Json) -> bool {
        let segs: Vec<&str> = path.split('.').collect();
        let mut cur = self;
        for (n, seg) in segs.iter().enumerate() {
            let last = n + 1 == segs.len();
            match cur {
                Json::Obj(m) => {
                    if last {
                        m.insert((*seg).to_string(), v);
                        return true;
                    }
                    cur = m.entry((*seg).to_string()).or_insert_with(Json::obj);
                }
                Json::Arr(a) => {
                    let Ok(i) = seg.parse::<usize>() else {
                        return false;
                    };
                    if i >= a.len() {
                        return false;
                    }
                    if last {
                        a[i] = v;
                        return true;
                    }
                    cur = &mut a[i];
                }
                _ => return false,
            }
        }
        false
    }

    pub fn as_str(&self) -> Option<&str> {
        if let Json::Str(s) = self {
            Some(s)
        } else {
            None
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        if let Json::Num(x) = self {
            Some(*x)
        } else {
            None
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Num(x) if x.fract() == 0.0 && x.abs() < 9.0e15 => Some(*x as i64),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        if let Json::Bool(b) = self {
            Some(*b)
        } else {
            None
        }
    }
    pub fn as_arr(&self) -> Option<&Vec<Json>> {
        if let Json::Arr(a) = self {
            Some(a)
        } else {
            None
        }
    }
    pub fn as_obj(&self) -> Option<&BTreeMap<String, Json>> {
        if let Json::Obj(m) = self {
            Some(m)
        } else {
            None
        }
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }
    /// Elementos de un arreglo o lista vacía (nunca falla).
    pub fn items(&self) -> &[Json] {
        match self {
            Json::Arr(a) => a.as_slice(),
            _ => &[],
        }
    }

    // ---------------------------------------------------------- serialización
    /// Forma compacta == forma canónica (claves ordenadas, sin espacios).
    pub fn to_compact(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, None, 0);
        s
    }
    pub fn canonical(&self) -> String {
        self.to_compact()
    }
    pub fn to_pretty(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, Some(2), 0);
        s.push('\n');
        s
    }

    fn write(&self, out: &mut String, indent: Option<usize>, level: usize) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(x) => out.push_str(&fmt_num(*x)),
            Json::Str(s) => write_str(out, s),
            Json::Arr(a) => {
                if a.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    newline(out, indent, level + 1);
                    v.write(out, indent, level + 1);
                }
                newline(out, indent, level);
                out.push(']');
            }
            Json::Obj(m) => {
                if m.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push('{');
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    newline(out, indent, level + 1);
                    write_str(out, k);
                    out.push(':');
                    if indent.is_some() {
                        out.push(' ');
                    }
                    v.write(out, indent, level + 1);
                }
                newline(out, indent, level);
                out.push('}');
            }
        }
    }
}

fn newline(out: &mut String, indent: Option<usize>, level: usize) {
    if let Some(n) = indent {
        out.push('\n');
        for _ in 0..(n * level) {
            out.push(' ');
        }
    }
}

/// Formato numérico determinista: enteros exactos sin decimales; el resto con la
/// representación más corta que reproduce el mismo `f64`.
pub fn fmt_num(x: f64) -> String {
    if !x.is_finite() {
        return "null".to_string();
    }
    if x == x.trunc() && x.abs() < 1.0e15 {
        return format!("{}", x as i64);
    }
    let a = x.abs();
    if !(1.0e-9..1.0e21).contains(&a) {
        return format!("{:e}", x);
    }
    format!("{}", x)
}

fn write_str(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

// ---------------------------------------------------------------- analizador
struct Parser<'a> {
    b: &'a [u8],
    i: usize,
    max_depth: usize,
}

impl<'a> Parser<'a> {
    fn err(&self, msg: &str) -> JsonError {
        JsonError {
            msg: msg.to_string(),
            offset: self.i,
        }
    }

    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn value(&mut self, depth: usize) -> Result<Json, JsonError> {
        if depth > self.max_depth {
            return Err(self.err("profundidad máxima excedida"));
        }
        match self.peek() {
            None => Err(self.err("fin de texto inesperado")),
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'n') => self.literal("null", Json::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            Some(_) => Err(self.err("carácter inesperado")),
        }
    }

    fn literal(&mut self, word: &str, v: Json) -> Result<Json, JsonError> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(self.err("literal inválido"))
        }
    }

    fn number(&mut self) -> Result<Json, JsonError> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        match self.peek() {
            Some(b'0') => self.i += 1,
            Some(c) if c.is_ascii_digit() => {
                while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                    self.i += 1;
                }
            }
            _ => return Err(self.err("número inválido")),
        }
        if self.peek() == Some(b'.') {
            self.i += 1;
            if !matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                return Err(self.err("se esperaba un dígito tras el punto decimal"));
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.i += 1;
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            self.i += 1;
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.i += 1;
            }
            if !matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                return Err(self.err("exponente inválido"));
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.i += 1;
            }
        }
        let txt = std::str::from_utf8(&self.b[start..self.i]).map_err(|_| self.err("utf8"))?;
        let x: f64 = txt.parse().map_err(|_| self.err("número inválido"))?;
        if !x.is_finite() {
            return Err(self.err("número fuera de rango"));
        }
        Ok(Json::Num(x))
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        if self.i + 4 > self.b.len() {
            return Err(self.err("secuencia \\u incompleta"));
        }
        let mut v = 0u32;
        for _ in 0..4 {
            let d = (self.b[self.i] as char)
                .to_digit(16)
                .ok_or_else(|| self.err("dígito hexadecimal inválido"))?;
            v = v * 16 + d;
            self.i += 1;
        }
        Ok(v)
    }

    fn string(&mut self) -> Result<String, JsonError> {
        // self.b[self.i] == '"'
        self.i += 1;
        let mut out = String::new();
        loop {
            // Copia en bloque los bytes que no necesitan tratamiento.
            let start = self.i;
            while self.i < self.b.len() {
                let c = self.b[self.i];
                if c == b'"' || c == b'\\' || c < 0x20 {
                    break;
                }
                self.i += 1;
            }
            if self.i > start {
                // El origen es &str válido y cortamos en bytes ASCII, así que es UTF-8 válido.
                out.push_str(
                    std::str::from_utf8(&self.b[start..self.i]).map_err(|_| self.err("utf8"))?,
                );
            }
            match self.peek() {
                None => return Err(self.err("cadena sin cerrar")),
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.i += 1;
                    let Some(e) = self.peek() else {
                        return Err(self.err("escape incompleto"));
                    };
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{08}'),
                        b'f' => out.push('\u{0C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let cp = if (0xD800..0xDC00).contains(&hi) {
                                if self.b[self.i..].starts_with(b"\\u") {
                                    self.i += 2;
                                    let lo = self.hex4()?;
                                    if !(0xDC00..0xE000).contains(&lo) {
                                        return Err(self.err("sustituto bajo inválido"));
                                    }
                                    0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                                } else {
                                    return Err(self.err("sustituto alto sin pareja"));
                                }
                            } else if (0xDC00..0xE000).contains(&hi) {
                                return Err(self.err("sustituto bajo sin pareja"));
                            } else {
                                hi
                            };
                            out.push(
                                char::from_u32(cp)
                                    .ok_or_else(|| self.err("punto de código inválido"))?,
                            );
                        }
                        _ => return Err(self.err("escape inválido")),
                    }
                }
                Some(_) => return Err(self.err("carácter de control sin escapar en cadena")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Json, JsonError> {
        self.i += 1;
        let mut v = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(Json::Arr(v));
        }
        loop {
            self.ws();
            v.push(self.value(depth + 1)?);
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Json::Arr(v));
                }
                _ => return Err(self.err("se esperaba ',' o ']'")),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, JsonError> {
        self.i += 1;
        let mut m = BTreeMap::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Json::Obj(m));
        }
        loop {
            self.ws();
            if self.peek() != Some(b'"') {
                return Err(self.err("se esperaba una clave de texto"));
            }
            let k = self.string()?;
            self.ws();
            if self.peek() != Some(b':') {
                return Err(self.err("se esperaba ':'"));
            }
            self.i += 1;
            self.ws();
            let v = self.value(depth + 1)?;
            // Claves duplicadas: se rechazan (ambigüedad en datos de seguridad).
            if m.insert(k.clone(), v).is_some() {
                return Err(self.err(&format!("clave duplicada '{}'", k)));
            }
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Json::Obj(m));
                }
                _ => return Err(self.err("se esperaba ',' o '}'")),
            }
        }
    }
}

/// Atajo para construir objetos: `jobj! { "a" => Json::num(1.0), "b" => Json::str("x") }`.
#[macro_export]
macro_rules! jobj {
    ( $( $k:expr => $v:expr ),* $(,)? ) => {{
        #[allow(unused_mut)]
        let mut o = $crate::json::Json::obj();
        $( o.set($k, $v); )*
        o
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_basico() {
        let src = r#"{"b":[1,2.5,-3e2,true,false,null],"a":"hola \"mundo\" \u00e1\n","c":{}}"#;
        let v = Json::parse(src).unwrap();
        // claves ordenadas en la forma canónica
        assert_eq!(
            v.to_compact(),
            r#"{"a":"hola \"mundo\" á\n","b":[1,2.5,-300,true,false,null],"c":{}}"#
        );
        let again = Json::parse(&v.to_compact()).unwrap();
        assert_eq!(v, again);
    }

    #[test]
    fn numeros_deterministas() {
        assert_eq!(fmt_num(1.0), "1");
        assert_eq!(fmt_num(-0.0), "0");
        assert_eq!(fmt_num(0.1), "0.1");
        assert_eq!(fmt_num(1.0e-7), "0.0000001");
        assert_eq!(fmt_num(1.0e-12), "1e-12");
        assert_eq!(fmt_num(1.5e25), "1.5e25");
        assert_eq!(fmt_num(123456789012345680.0), "123456789012345680");
        for x in [
            0.1,
            1.0 / 3.0,
            6.05e5,
            1.85,
            4.87,
            1.0e-300,
            2.5e-9,
            98765.4321,
        ] {
            let t = fmt_num(x);
            let y: f64 = t.parse().unwrap();
            assert_eq!(x, y, "roundtrip de {}", t);
        }
    }

    #[test]
    fn no_finitos_se_vuelven_null() {
        assert_eq!(Json::num(f64::NAN), Json::Null);
        assert_eq!(Json::num(f64::INFINITY), Json::Null);
    }

    #[test]
    fn rechaza_basura() {
        for bad in [
            "",
            "{",
            "[1,]",
            "{\"a\":1,}",
            "01",
            "1.",
            "-",
            "\"\\x\"",
            "nul",
            "[1 2]",
            "{\"a\" 1}",
            "{\"a\":1,\"a\":2}",
            "1e999",
            "\"\\ud800\"",
        ] {
            assert!(Json::parse(bad).is_err(), "debía fallar: {:?}", bad);
        }
    }

    #[test]
    fn limite_de_profundidad_y_tamano() {
        let deep = format!("{}{}", "[".repeat(300), "]".repeat(300));
        assert!(Json::parse(&deep).is_err());
        let ok = format!("{}{}", "[".repeat(100), "]".repeat(100));
        assert!(Json::parse(&ok).is_ok());
        let lim = Limits {
            max_bytes: 10,
            max_depth: 8,
        };
        assert!(Json::parse_with_limits("[1,2,3,4,5,6,7,8,9]", lim).is_err());
    }

    #[test]
    fn rutas() {
        let mut v = Json::parse(r#"{"a":{"b":[{"c":5},{"c":6}]}}"#).unwrap();
        assert_eq!(v.at("a.b.1.c").and_then(Json::as_f64), Some(6.0));
        assert!(v.at("a.b.2.c").is_none());
        assert!(v.set_at("a.x.y", Json::num(1.0)));
        assert_eq!(v.at("a.x.y").and_then(Json::as_f64), Some(1.0));
        assert!(v.set_at("a.b.0.c", Json::num(9.0)));
        assert_eq!(v.at("a.b.0.c").and_then(Json::as_f64), Some(9.0));
        assert!(!v.set_at("a.b.7.c", Json::num(9.0)));
    }

    #[test]
    fn unicode_y_sustitutos() {
        let v = Json::parse(r#""\ud83d\ude00 ñ""#).unwrap();
        assert_eq!(v.as_str(), Some("😀 ñ"));
        assert_eq!(
            Json::str("tab\tctl\u{1}").to_compact(),
            r#""tab\tctl\u0001""#
        );
    }

    #[test]
    fn macro_jobj() {
        let o = jobj! { "z" => Json::num(1.0), "a" => Json::str("x") };
        assert_eq!(o.to_compact(), r#"{"a":"x","z":1}"#);
    }
}
