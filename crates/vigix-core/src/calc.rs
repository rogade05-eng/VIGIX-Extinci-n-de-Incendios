//! Tipos comunes de los módulos de cálculo.
//!
//! Todo resultado lleva: versión del motor, perfil (código, versión, huella, estado de
//! verificación), huella de los datos de entrada, incidencias, parámetros normativos usados
//! (con referencia) y el rastro de ecuaciones (contrato 4.3/9.1/11.2).

use crate::hash::sha256_hex;
use crate::issues::{Issue, Severity};
use crate::json::Json;
use crate::profile::{Param, Profile};
use crate::{ENGINE_VERSION, PRODUCT_NAME};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Cálculo completado (puede traer advertencias o errores de verificación).
    Ok,
    /// Faltan datos o hay una condición que impide calcular (nunca se rellena con ceros).
    Blocked,
    /// El método numérico no convergió.
    NotConverged,
    /// Cancelado por el usuario.
    Cancelled,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Blocked => "blocked",
            Status::NotConverged => "not_converged",
            Status::Cancelled => "cancelled",
        }
    }
}

/// Resultado de un módulo de cálculo.
#[derive(Clone, Debug)]
pub struct CalcOutput {
    pub kind: String,
    pub status: Status,
    /// Valores resultantes (estructura propia de cada módulo). `Null` si está bloqueado.
    pub values: Json,
    /// Rastro: ecuaciones aplicadas con entradas, unidades y resultado intermedio.
    pub trace: Vec<Json>,
    /// Parámetros normativos del perfil efectivamente usados (con referencia).
    pub params_used: Vec<Param>,
    pub issues: Vec<Issue>,
    pub fingerprint: String,
}

impl CalcOutput {
    pub fn blocked(kind: &str, issues: Vec<Issue>, fingerprint: String) -> CalcOutput {
        CalcOutput {
            kind: kind.to_string(),
            status: Status::Blocked,
            values: Json::Null,
            trace: vec![],
            params_used: vec![],
            issues,
            fingerprint,
        }
    }

    pub fn worst(&self) -> Option<Severity> {
        self.issues.iter().map(|i| i.severity).max()
    }

    /// Sobre JSON que se guarda en `project.results[kind]`.
    pub fn to_json(&self, profile: Option<&Profile>, calc_time_iso: &str) -> Json {
        let mut o = Json::obj()
            .with("kind", Json::str(self.kind.as_str()))
            .with("status", Json::str(self.status.as_str()))
            .with("engine", Json::obj().with("name", Json::str(PRODUCT_NAME)).with("version", Json::str(ENGINE_VERSION)))
            .with("input_fingerprint", Json::str(self.fingerprint.as_str()))
            .with("calc_at", Json::str(calc_time_iso))
            .with("values", self.values.clone())
            .with("trace", Json::Arr(self.trace.clone()))
            .with("params_used", Json::Arr(self.params_used.iter().map(Param::to_json).collect()))
            .with("issues", Json::Arr(self.issues.iter().map(Issue::to_json).collect()));
        o.set(
            "profile",
            match profile {
                Some(p) => Json::obj()
                    .with("code", Json::str(p.code()))
                    .with("version", Json::str(p.version()))
                    .with("sha256", Json::str(p.sha256.as_str()))
                    .with("verified", Json::Bool(p.is_verified()))
                    .with("demo", Json::Bool(p.is_demo())),
                None => Json::Null,
            },
        );
        o
    }
}

/// Huella SHA-256 de un conjunto de datos de entrada + perfil + versión del motor.
/// Mismas entradas => misma huella (contrato 11.2 "repetibilidad").
pub fn fingerprint(kind: &str, parts: &[&Json], profile: Option<&Profile>) -> String {
    let mut s = String::new();
    s.push_str(kind);
    s.push('|');
    s.push_str(ENGINE_VERSION);
    s.push('|');
    s.push_str(profile.map(|p| p.sha256.as_str()).unwrap_or("sin-perfil"));
    for p in parts {
        s.push('|');
        s.push_str(&p.canonical());
    }
    sha256_hex(s.as_bytes())
}

/// Paso del rastro de cálculo.
pub fn trace_step(step: &str, equation: &str, inputs: Json, output: Json) -> Json {
    Json::obj().with("step", Json::str(step)).with("equation", Json::str(equation)).with("inputs", inputs).with("output", output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huella_estable_y_sensible() {
        let a = Json::parse(r#"{"x":1,"y":[1,2]}"#).unwrap();
        let b = Json::parse(r#"{"y":[1,2],"x":1}"#).unwrap();
        let c = Json::parse(r#"{"x":2,"y":[1,2]}"#).unwrap();
        assert_eq!(fingerprint("k", &[&a], None), fingerprint("k", &[&b], None));
        assert_ne!(fingerprint("k", &[&a], None), fingerprint("k", &[&c], None));
        assert_ne!(fingerprint("k", &[&a], None), fingerprint("otro", &[&a], None));
    }

    #[test]
    fn sobre_json_lleva_metadatos_de_trazabilidad() {
        let out = CalcOutput::blocked("hydraulic", vec![Issue::block("data.missing")], "abc".into());
        let j = out.to_json(None, "2026-10-28T00:00:00Z");
        assert_eq!(j.field("status").as_str(), Some("blocked"));
        assert_eq!(j.at("engine.version").and_then(Json::as_str), Some(ENGINE_VERSION));
        assert_eq!(j.field("input_fingerprint").as_str(), Some("abc"));
        assert!(j.field("profile").is_null());
        assert_eq!(j.field("issues").items().len(), 1);
    }
}
