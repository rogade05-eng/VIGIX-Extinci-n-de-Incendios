//! Motores de agentes extintores: espuma, gases (CO2, agentes limpios, inertes), polvo químico,
//! químico húmedo de cocinas y aerosol condensado (contrato 5.5 a 5.8).
//!
//! Estructura común: cada motor lee **todo** valor normativo del perfil (con referencia y
//! unidad) y los datos de fabricante del propio sistema (catálogo); si falta algo, bloquea con
//! la ruta exacta. Nunca hay valores por defecto ocultos.

pub mod aerosol;
pub mod dry_chem;
pub mod foam;
pub mod gas;
pub mod wet_chem;

use crate::calc::{CalcOutput, Status};
use crate::issues::Issue;
use crate::json::Json;
use crate::profile::{Param, Profile};
use crate::reader::{Reader, Scope};

/// Contexto de un cálculo: perfil, lector con incidencias, parámetros usados y rastro.
pub struct Calc<'a> {
    pub profile: &'a Profile,
    pub module: &'static str,
    pub rd: Reader,
    pub used: Vec<Param>,
    pub trace: Vec<Json>,
}

impl<'a> Calc<'a> {
    pub fn new(profile: &'a Profile, module: &'static str) -> Calc<'a> {
        Calc { profile, module, rd: Reader::new(module), used: Vec::new(), trace: Vec::new() }
    }

    fn note(&mut self, p: &Param) {
        if !self.used.iter().any(|u| u.path == p.path && u.value == p.value) {
            self.used.push(p.clone());
        }
    }

    pub fn push(&mut self, i: Issue) {
        self.rd.push(i);
    }

    pub fn blocked(&self) -> bool {
        self.rd.has_block()
    }

    /// El perfil debe cubrir el tipo de sistema (contrato 3.5).
    pub fn require_scope(&mut self, system: &str) -> bool {
        if self.profile.covers(system) {
            true
        } else {
            self.rd.push(Issue::block("gate.profile_not_covering").module(self.module).path("scope.systems").param("system", system));
            false
        }
    }

    /// Método declarado por el perfil; debe estar entre los admitidos por el motor.
    pub fn require_method(&mut self, key: &str, allowed: &[&str]) -> Option<String> {
        match self.profile.method(key) {
            Some(m) if allowed.contains(&m) => Some(m.to_string()),
            other => {
                self.rd.push(
                    Issue::block("profile.method_unsupported")
                        .module(self.module)
                        .path(&format!("methods.{}", key))
                        .param("method", other.unwrap_or(""))
                        .param("module", key),
                );
                None
            }
        }
    }

    pub fn param(&mut self, path: &str, unit: &str) -> Option<Param> {
        match self.profile.param_in(path, unit) {
            Ok(p) => {
                self.note(&p);
                Some(p)
            }
            Err(i) => {
                self.rd.push(i);
                None
            }
        }
    }

    /// Parámetro opcional: ausente => `None` sin incidencia; unidad incompatible => bloqueo.
    pub fn param_opt(&mut self, path: &str, unit: &str) -> Option<Param> {
        match self.profile.param_in(path, unit) {
            Ok(p) => {
                self.note(&p);
                Some(p)
            }
            Err(i) if i.code == "profile.param_missing" => None,
            Err(i) => {
                self.rd.push(i);
                None
            }
        }
    }

    pub fn flag(&mut self, path: &str) -> Option<bool> {
        match self.profile.flag(path) {
            Ok((b, _)) => Some(b),
            Err(i) => {
                self.rd.push(i);
                None
            }
        }
    }

    pub fn flag_opt(&mut self, path: &str) -> Option<bool> {
        match self.profile.flag(path) {
            Ok((b, _)) => Some(b),
            Err(i) if i.code == "profile.param_missing" => None,
            Err(i) => {
                self.rd.push(i);
                None
            }
        }
    }

    pub fn text(&mut self, path: &str) -> Option<String> {
        match self.profile.text(path) {
            Ok((t, _)) => Some(t),
            Err(i) => {
                self.rd.push(i);
                None
            }
        }
    }

    pub fn lookup(&mut self, table: &str, keys: &[(&str, &str)], unit: &str, object: Option<&str>) -> Option<Param> {
        match self.profile.lookup_in(table, keys, unit) {
            Ok(p) => {
                self.note(&p);
                Some(p)
            }
            Err(i) => {
                self.rd.push(i.opt_object(object));
                None
            }
        }
    }

    pub fn lookup_opt(&mut self, table: &str, keys: &[(&str, &str)], unit: &str, object: Option<&str>) -> Option<Param> {
        match self.profile.lookup_in(table, keys, unit) {
            Ok(p) => {
                self.note(&p);
                Some(p)
            }
            Err(i) if i.code == "profile.table_missing" => None,
            Err(i) => {
                self.rd.push(i.opt_object(object));
                None
            }
        }
    }

    pub fn lookup_range(&mut self, table: &str, keys: &[(&str, &str)], x: f64, unit: &str, object: Option<&str>) -> Option<Param> {
        match self.profile.lookup_range_in(table, keys, x, unit) {
            Ok(p) => {
                self.note(&p);
                Some(p)
            }
            Err(i) => {
                self.rd.push(i.opt_object(object));
                None
            }
        }
    }

    /// Cierra el cálculo: bloqueado (sin valores) o completado con incidencias ordenadas.
    pub fn finish(mut self, kind: &str, fp: String, values: Json) -> CalcOutput {
        self.rd.issues.sort_by(|a, b| {
            b.severity
                .cmp(&a.severity)
                .then_with(|| a.code.cmp(&b.code))
                .then_with(|| a.object.cmp(&b.object))
                .then_with(|| a.path.cmp(&b.path))
        });
        if self.rd.has_block() {
            return CalcOutput {
                kind: kind.to_string(),
                status: Status::Blocked,
                values: Json::Null,
                trace: self.trace,
                params_used: self.used,
                issues: self.rd.issues,
                fingerprint: fp,
            };
        }
        CalcOutput {
            kind: kind.to_string(),
            status: Status::Ok,
            values,
            trace: self.trace,
            params_used: self.used,
            issues: self.rd.issues,
            fingerprint: fp,
        }
    }
}

/// Confirmaciones críticas para espacios ocupados (alarma de predescarga y evacuación
/// verificadas). Un `false` o un dato ausente **bloquea**; no se asume nada.
pub fn occupied_safety(c: &mut Calc, s: &Scope, occupied: bool, id: &str, code_prefix: &str) {
    if !occupied {
        return;
    }
    let sf = s.child("safety");
    if sf.j.is_null() {
        c.push(Issue::block("data.missing").module(c.module).path(&sf.path).object(id).param("field", "safety"));
        return;
    }
    if c.rd.bool(&sf, "predischarge_alarm") == Some(false) {
        c.push(
            Issue::block(&format!("{}.predischarge_alarm_missing", code_prefix))
                .module(c.module)
                .path(&sf.key_path("predischarge_alarm"))
                .object(id),
        );
    }
    if c.rd.bool(&sf, "evacuation_verified") == Some(false) {
        c.push(
            Issue::block(&format!("{}.evacuation_not_verified", code_prefix))
                .module(c.module)
                .path(&sf.key_path("evacuation_verified"))
                .object(id),
        );
    }
}

/// Raíz de lectura de un sistema con su id y su ruta dentro del proyecto.
pub fn sys_scope<'a>(sys: &'a Json, base: &str) -> Scope<'a> {
    let s = Scope::new(sys, base);
    match sys.field("id").as_str() {
        Some(id) => s.with_id(id),
        None => s,
    }
}

#[cfg(test)]
pub mod testutil {
    use crate::json::Json;
    use crate::profile::Profile;
    use std::fs;

    pub fn demo_json() -> Json {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../profiles/demo/DEMO-NO-NORMATIVO_0.1.0.json");
        Json::parse(&fs::read_to_string(p).unwrap()).unwrap()
    }
    pub fn demo() -> Profile {
        Profile::from_json(demo_json()).unwrap()
    }
    pub fn val(j: &Json, path: &str) -> f64 {
        j.at(path).and_then(Json::as_f64).unwrap_or_else(|| panic!("falta {} en {}", path, j.to_compact()))
    }
    pub fn has_issue(o: &crate::calc::CalcOutput, code: &str) -> bool {
        o.issues.iter().any(|i| i.code == code)
    }
    /// Cambia (o borra con `None`) una clave literal de `methods` (las claves llevan puntos).
    pub fn set_method(p: &mut Json, key: &str, value: Option<&str>) {
        if let Some(Json::Obj(root)) = Some(&mut *p) {
            if let Some(Json::Obj(m)) = root.get_mut("methods") {
                match value {
                    Some(v) => {
                        m.insert(key.to_string(), Json::str(v));
                    }
                    None => {
                        m.remove(key);
                    }
                }
            }
        }
    }
    /// Cambia un parámetro del perfil dado por su ruta de claves literales.
    pub fn set_param(p: &mut Json, keys: &[&str], value: Json) {
        let mut cur = p;
        for k in keys {
            cur = match cur {
                Json::Obj(m) => m.get_mut(*k).unwrap_or_else(|| panic!("no existe la clave {}", k)),
                _ => panic!("ruta inválida"),
            };
        }
        *cur = value;
    }
}
