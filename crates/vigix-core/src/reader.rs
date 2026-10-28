//! Lector tipado de datos de entrada.
//!
//! Regla del contrato (3.5): **ningún valor faltante se sustituye por cero, valor medio o
//! supuesto oculto**. Cada método devuelve `Option`: si falta o es inválido, registra una
//! incidencia de bloqueo con la ruta exacta del dato y devuelve `None`. El módulo de cálculo
//! comprueba `has_block()` antes de calcular.

use crate::issues::{Issue, Severity};
use crate::json::Json;

/// Ámbito de lectura: objeto JSON + ruta (para localizar el dato) + id del objeto de modelo.
#[derive(Clone)]
pub struct Scope<'a> {
    pub j: &'a Json,
    pub path: String,
    pub id: Option<String>,
}

impl<'a> Scope<'a> {
    pub fn new(j: &'a Json, path: &str) -> Scope<'a> {
        Scope { j, path: path.to_string(), id: None }
    }
    pub fn with_id(mut self, id: &str) -> Scope<'a> {
        self.id = Some(id.to_string());
        self
    }
    /// Desciende a un campo (si no existe usa `Null`).
    pub fn child(&self, key: &str) -> Scope<'a> {
        Scope { j: self.j.field(key), path: join(&self.path, key), id: self.id.clone() }
    }
    pub fn item(&self, idx: usize, item: &'a Json) -> Scope<'a> {
        Scope { j: item, path: join(&self.path, &idx.to_string()), id: self.id.clone() }
    }
    pub fn key_path(&self, key: &str) -> String {
        join(&self.path, key)
    }
}

fn join(base: &str, key: &str) -> String {
    if base.is_empty() {
        key.to_string()
    } else {
        format!("{}.{}", base, key)
    }
}

pub struct Reader {
    pub module: String,
    pub issues: Vec<Issue>,
}

impl Reader {
    pub fn new(module: &str) -> Reader {
        Reader { module: module.to_string(), issues: Vec::new() }
    }

    pub fn has_block(&self) -> bool {
        self.issues.iter().any(|i| i.severity == Severity::Block)
    }

    pub fn push(&mut self, i: Issue) {
        self.issues.push(i);
    }

    fn raise(&mut self, code: &str, s: &Scope, key: &str) -> Issue {
        Issue::block(code).module(&self.module).path(&s.key_path(key)).opt_object(s.id.as_deref()).param("field", key)
    }

    fn fail(&mut self, code: &str, s: &Scope, key: &str) {
        let i = self.raise(code, s, key);
        self.issues.push(i);
    }

    /// Número finito obligatorio.
    pub fn f64(&mut self, s: &Scope, key: &str) -> Option<f64> {
        match s.j.field(key) {
            Json::Null => {
                self.fail("data.missing", s, key);
                None
            }
            Json::Num(x) if x.is_finite() => Some(*x),
            _ => {
                self.fail("data.type", s, key);
                None
            }
        }
    }

    /// Número estrictamente positivo (diámetro 0, volumen 0, K = 0... son bloqueos).
    pub fn f64_pos(&mut self, s: &Scope, key: &str) -> Option<f64> {
        let v = self.f64(s, key)?;
        if v > 0.0 {
            Some(v)
        } else {
            let i = self.raise("data.not_positive", s, key).param("value", crate::json::fmt_num(v));
            self.issues.push(i);
            None
        }
    }

    pub fn f64_nonneg(&mut self, s: &Scope, key: &str) -> Option<f64> {
        let v = self.f64(s, key)?;
        if v >= 0.0 {
            Some(v)
        } else {
            let i = self.raise("data.negative", s, key).param("value", crate::json::fmt_num(v));
            self.issues.push(i);
            None
        }
    }

    pub fn f64_range(&mut self, s: &Scope, key: &str, lo: f64, hi: f64) -> Option<f64> {
        let v = self.f64(s, key)?;
        if v >= lo && v <= hi {
            Some(v)
        } else {
            let i = self
                .raise("data.out_of_range", s, key)
                .param("value", crate::json::fmt_num(v))
                .param("min", crate::json::fmt_num(lo))
                .param("max", crate::json::fmt_num(hi));
            self.issues.push(i);
            None
        }
    }

    /// Número opcional: ausente o `null` => `None` sin incidencia (opcionalidad explícita);
    /// un tipo equivocado sí es incidencia.
    pub fn opt_f64(&mut self, s: &Scope, key: &str) -> Option<f64> {
        match s.j.field(key) {
            Json::Null => None,
            Json::Num(x) if x.is_finite() => Some(*x),
            _ => {
                self.fail("data.type", s, key);
                None
            }
        }
    }

    /// Texto obligatorio no vacío.
    pub fn str<'a>(&mut self, s: &Scope<'a>, key: &str) -> Option<&'a str> {
        match s.j.field(key) {
            Json::Null => {
                self.fail("data.missing", s, key);
                None
            }
            Json::Str(t) if !t.trim().is_empty() => Some(t.as_str()),
            Json::Str(_) => {
                self.fail("data.empty", s, key);
                None
            }
            _ => {
                self.fail("data.type", s, key);
                None
            }
        }
    }

    pub fn opt_str<'a>(&mut self, s: &Scope<'a>, key: &str) -> Option<&'a str> {
        match s.j.field(key) {
            Json::Null => None,
            Json::Str(t) if !t.trim().is_empty() => Some(t.as_str()),
            Json::Str(_) => None,
            _ => {
                self.fail("data.type", s, key);
                None
            }
        }
    }

    /// Booleano **explícito** (un `null` es "no confirmado" => dato faltante).
    pub fn bool(&mut self, s: &Scope, key: &str) -> Option<bool> {
        match s.j.field(key) {
            Json::Null => {
                self.fail("data.missing", s, key);
                None
            }
            Json::Bool(b) => Some(*b),
            _ => {
                self.fail("data.type", s, key);
                None
            }
        }
    }

    pub fn arr<'a>(&mut self, s: &Scope<'a>, key: &str) -> Option<&'a [Json]> {
        match s.j.field(key) {
            Json::Null => {
                self.fail("data.missing", s, key);
                None
            }
            Json::Arr(a) => Some(a.as_slice()),
            _ => {
                self.fail("data.type", s, key);
                None
            }
        }
    }

    pub fn obj<'a>(&mut self, s: &Scope<'a>, key: &str) -> Option<Scope<'a>> {
        match s.j.field(key) {
            Json::Null => {
                self.fail("data.missing", s, key);
                None
            }
            Json::Obj(_) => Some(s.child(key)),
            _ => {
                self.fail("data.type", s, key);
                None
            }
        }
    }

    /// Objeto opcional: ausente o `null` => `None` sin incidencia; un tipo equivocado sí es incidencia.
    pub fn opt_obj<'a>(&mut self, s: &Scope<'a>, key: &str) -> Option<Scope<'a>> {
        match s.j.field(key) {
            Json::Null => None,
            Json::Obj(_) => Some(s.child(key)),
            _ => {
                self.fail("data.type", s, key);
                None
            }
        }
    }

    /// Texto que debe pertenecer a un conjunto cerrado.
    pub fn enum_str<'a>(&mut self, s: &Scope<'a>, key: &str, allowed: &[&str]) -> Option<&'a str> {
        let v = self.str(s, key)?;
        if allowed.contains(&v) {
            Some(v)
        } else {
            let i = self.raise("data.bad_enum", s, key).param("value", v).param("allowed", allowed.join("|"));
            self.issues.push(i);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faltante_nunca_es_cero() {
        let j = Json::parse(r#"{"d":null,"e":0,"f":-2,"g":"x","h":5}"#).unwrap();
        let s = Scope::new(&j, "pipe").with_id("P1");
        let mut r = Reader::new("water");
        assert_eq!(r.f64(&s, "d"), None); // null
        assert_eq!(r.f64(&s, "zz"), None); // ausente
        assert_eq!(r.f64_pos(&s, "e"), None); // cero no es positivo
        assert_eq!(r.f64_nonneg(&s, "f"), None);
        assert_eq!(r.f64(&s, "g"), None); // tipo
        assert_eq!(r.f64_range(&s, "h", 6.0, 9.0), None);
        assert_eq!(r.f64_range(&s, "h", 1.0, 9.0), Some(5.0));
        let codes: Vec<&str> = r.issues.iter().map(|i| i.code.as_str()).collect();
        assert_eq!(codes, ["data.missing", "data.missing", "data.not_positive", "data.negative", "data.type", "data.out_of_range"]);
        assert!(r.has_block());
        assert_eq!(r.issues[0].path, "pipe.d");
        assert_eq!(r.issues[0].object.as_deref(), Some("P1"));
        assert_eq!(r.issues[0].params.get("field").map(String::as_str), Some("d"));
    }

    #[test]
    fn opcional_y_booleano_explicito() {
        let j = Json::parse(r#"{"a":null,"b":true,"c":"hola","d":""}"#).unwrap();
        let s = Scope::new(&j, "");
        let mut r = Reader::new("x");
        assert_eq!(r.opt_f64(&s, "a"), None);
        assert_eq!(r.opt_f64(&s, "nada"), None);
        assert!(r.issues.is_empty());
        assert_eq!(r.bool(&s, "b"), Some(true));
        assert_eq!(r.bool(&s, "a"), None); // null => "no confirmado" => faltante
        assert_eq!(r.str(&s, "c"), Some("hola"));
        assert_eq!(r.str(&s, "d"), None);
        assert_eq!(r.issues.len(), 2);
        assert_eq!(r.issues[0].path, "a");
    }

    #[test]
    fn enumeraciones() {
        let j = Json::parse(r#"{"k":"pipe","z":"otro"}"#).unwrap();
        let s = Scope::new(&j, "link");
        let mut r = Reader::new("water");
        assert_eq!(r.enum_str(&s, "k", &["pipe", "pump"]), Some("pipe"));
        assert_eq!(r.enum_str(&s, "z", &["pipe", "pump"]), None);
        assert_eq!(r.issues[0].code, "data.bad_enum");
        assert_eq!(r.issues[0].params.get("allowed").map(String::as_str), Some("pipe|pump"));
    }

    #[test]
    fn objeto_opcional_no_bloquea_si_falta() {
        let j = Json::parse(r#"{"a":{"x":1},"n":null,"mal":5}"#).unwrap();
        let s = Scope::new(&j, "");
        let mut r = Reader::new("x");
        assert!(r.opt_obj(&s, "a").is_some());
        assert!(r.opt_obj(&s, "n").is_none());
        assert!(r.opt_obj(&s, "ausente").is_none());
        assert!(r.issues.is_empty(), "un objeto opcional ausente no es un dato faltante");
        assert!(r.opt_obj(&s, "mal").is_none());
        assert_eq!(r.issues[0].code, "data.type");
        // el accesor obligatorio SÍ bloquea
        assert!(r.obj(&s, "ausente").is_none());
        assert!(r.issues.iter().any(|i| i.code == "data.missing"));
    }

    #[test]
    fn ambitos_anidados() {
        let j = Json::parse(r#"{"nodes":[{"id":"N1","z":3}],"x":{"y":{"v":1}}}"#).unwrap();
        let root = Scope::new(&j, "net");
        let mut r = Reader::new("water");
        let nodes = r.arr(&root, "nodes").unwrap();
        let sc = root.child("nodes").item(0, &nodes[0]).with_id("N1");
        assert_eq!(r.f64(&sc, "z"), Some(3.0));
        assert_eq!(r.f64(&sc, "w"), None);
        assert_eq!(r.issues[0].path, "net.nodes.0.w");
        assert_eq!(r.issues[0].object.as_deref(), Some("N1"));
        let y = root.child("x").child("y");
        assert_eq!(y.path, "net.x.y");
    }
}
