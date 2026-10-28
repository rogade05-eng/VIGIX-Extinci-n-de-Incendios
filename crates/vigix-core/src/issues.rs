//! Incidencias de validación (contrato 5.10).
//!
//! Severidades: bloqueo, error, advertencia, información y comprobación superada.
//! Cada incidencia lleva: causa (`code`, que se traduce con el catálogo `issue.<code>`),
//! dato implicado (`path` + `params`), referencia normativa del perfil (`reference`),
//! acción sugerida (`issue.<code>.action`) y el objeto que se puede localizar (`object`).
//! El motor **no** genera texto visible: solo códigos y parámetros (contrato 7).

use crate::json::Json;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Comprobación superada.
    Pass = 0,
    Info = 1,
    Warning = 2,
    Error = 3,
    /// Impide emitir memoria de conformidad / obtener un resultado válido.
    Block = 4,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Pass => "pass",
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
            Severity::Block => "block",
        }
    }
    pub fn parse(s: &str) -> Option<Severity> {
        Some(match s {
            "pass" => Severity::Pass,
            "info" => Severity::Info,
            "warning" => Severity::Warning,
            "error" => Severity::Error,
            "block" => Severity::Block,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub severity: Severity,
    /// Código estable, p. ej. `data.missing`, `hyd.no_source`.
    pub code: String,
    /// Módulo: `project`, `profile`, `extinguishers`, `water`, `foam`, `gas`, ...
    pub module: String,
    /// Ruta con puntos del dato implicado dentro del proyecto o del perfil.
    pub path: String,
    /// Identificador del objeto del modelo (para localizarlo en el plano).
    pub object: Option<String>,
    pub params: BTreeMap<String, String>,
    /// Referencia normativa del perfil (capítulo/sección/tabla), si aplica.
    pub reference: Option<String>,
}

impl Issue {
    pub fn new(severity: Severity, code: &str) -> Issue {
        Issue {
            severity,
            code: code.to_string(),
            module: String::new(),
            path: String::new(),
            object: None,
            params: BTreeMap::new(),
            reference: None,
        }
    }
    pub fn block(code: &str) -> Issue {
        Issue::new(Severity::Block, code)
    }
    pub fn error(code: &str) -> Issue {
        Issue::new(Severity::Error, code)
    }
    pub fn warning(code: &str) -> Issue {
        Issue::new(Severity::Warning, code)
    }
    pub fn info(code: &str) -> Issue {
        Issue::new(Severity::Info, code)
    }
    pub fn pass(code: &str) -> Issue {
        Issue::new(Severity::Pass, code)
    }

    pub fn module(mut self, m: &str) -> Issue {
        self.module = m.to_string();
        self
    }
    pub fn path(mut self, p: &str) -> Issue {
        self.path = p.to_string();
        self
    }
    pub fn object(mut self, id: &str) -> Issue {
        self.object = Some(id.to_string());
        self
    }
    pub fn opt_object(mut self, id: Option<&str>) -> Issue {
        self.object = id.map(|s| s.to_string());
        self
    }
    pub fn param<V: ToString>(mut self, k: &str, v: V) -> Issue {
        self.params.insert(k.to_string(), v.to_string());
        self
    }
    pub fn reference(mut self, r: &str) -> Issue {
        if !r.is_empty() {
            self.reference = Some(r.to_string());
        }
        self
    }

    pub fn to_json(&self) -> Json {
        let mut params = Json::obj();
        for (k, v) in &self.params {
            params.set(k.as_str(), Json::str(v.as_str()));
        }
        Json::obj()
            .with("severity", Json::str(self.severity.as_str()))
            .with("code", Json::str(self.code.as_str()))
            .with("module", Json::str(self.module.as_str()))
            .with("path", Json::str(self.path.as_str()))
            .with("object", Json::opt_str(self.object.as_deref()))
            .with("params", params)
            .with("reference", Json::opt_str(self.reference.as_deref()))
    }

    pub fn from_json(j: &Json) -> Option<Issue> {
        let mut params = BTreeMap::new();
        if let Some(o) = j.field("params").as_obj() {
            for (k, v) in o {
                params.insert(k.clone(), v.as_str().unwrap_or("").to_string());
            }
        }
        Some(Issue {
            severity: Severity::parse(j.field("severity").as_str()?)?,
            code: j.field("code").as_str()?.to_string(),
            module: j.field("module").as_str().unwrap_or("").to_string(),
            path: j.field("path").as_str().unwrap_or("").to_string(),
            object: j.field("object").as_str().map(|s| s.to_string()),
            params,
            reference: j.field("reference").as_str().map(|s| s.to_string()),
        })
    }
}

/// Lista de incidencias con utilidades de agregación.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IssueList(pub Vec<Issue>);

impl IssueList {
    pub fn new() -> Self {
        IssueList(Vec::new())
    }
    pub fn push(&mut self, i: Issue) {
        self.0.push(i);
    }
    pub fn extend<I: IntoIterator<Item = Issue>>(&mut self, it: I) {
        self.0.extend(it);
    }
    pub fn has_block(&self) -> bool {
        self.0.iter().any(|i| i.severity == Severity::Block)
    }
    pub fn has_error_or_block(&self) -> bool {
        self.0.iter().any(|i| i.severity >= Severity::Error)
    }
    pub fn worst(&self) -> Option<Severity> {
        self.0.iter().map(|i| i.severity).max()
    }
    pub fn count(&self, s: Severity) -> usize {
        self.0.iter().filter(|i| i.severity == s).count()
    }
    /// Orden determinista: más grave primero, luego módulo, ruta y código.
    pub fn sort(&mut self) {
        self.0.sort_by(|a, b| {
            b.severity
                .cmp(&a.severity)
                .then_with(|| a.module.cmp(&b.module))
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.code.cmp(&b.code))
                .then_with(|| a.object.cmp(&b.object))
        });
    }
    pub fn to_json(&self) -> Json {
        Json::Arr(self.0.iter().map(Issue::to_json).collect())
    }
    pub fn summary_json(&self) -> Json {
        Json::obj()
            .with("block", Json::num(self.count(Severity::Block) as f64))
            .with("error", Json::num(self.count(Severity::Error) as f64))
            .with("warning", Json::num(self.count(Severity::Warning) as f64))
            .with("info", Json::num(self.count(Severity::Info) as f64))
            .with("pass", Json::num(self.count(Severity::Pass) as f64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orden_y_agregacion() {
        let mut l = IssueList::new();
        l.push(Issue::info("a.info").module("water"));
        l.push(Issue::block("b.block").module("gas").path("x.y"));
        l.push(Issue::warning("c.warn").module("foam"));
        l.push(Issue::pass("d.pass"));
        l.push(Issue::error("e.err").module("water"));
        assert!(l.has_block());
        assert_eq!(l.worst(), Some(Severity::Block));
        l.sort();
        let sev: Vec<Severity> = l.0.iter().map(|i| i.severity).collect();
        assert_eq!(sev, vec![Severity::Block, Severity::Error, Severity::Warning, Severity::Info, Severity::Pass]);
        assert_eq!(l.count(Severity::Warning), 1);
    }

    #[test]
    fn json_ida_y_vuelta() {
        let i = Issue::block("data.missing").module("water").path("a.b").object("P1").param("field", "diameter_mm").reference("S1:Tabla 3");
        let j = i.to_json();
        let back = Issue::from_json(&j).unwrap();
        assert_eq!(i, back);
        assert_eq!(j.field("severity").as_str(), Some("block"));
    }

    #[test]
    fn severidad_texto() {
        for s in [Severity::Pass, Severity::Info, Severity::Warning, Severity::Error, Severity::Block] {
            assert_eq!(Severity::parse(s.as_str()), Some(s));
        }
        assert_eq!(Severity::parse("x"), None);
        assert!(Severity::Block > Severity::Error && Severity::Error > Severity::Warning);
    }
}
