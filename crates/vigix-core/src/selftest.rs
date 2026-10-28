//! Autotest de perfiles normativos (contrato 11.2 "pruebas de perfil normativo").
//!
//! Un perfil incluye casos de prueba independientes con su resultado esperado. Este módulo los
//! ejecuta contra el motor **actual**: si alguno falla, el perfil deja de considerarse verificado
//! (el motor y el perfil divergen) y se informa qué caso y qué valor difieren.
//!
//! Formato de un caso:
//! ```text
//! { "id": "C1", "system": "foam", "module": "foam", "input": { "system": {...} },
//!   "expected": [ { "path": "values.flows.solution_lpm", "value": 200, "tolerance_rel": 1e-9 },
//!                 { "path": "values.zones.0.ok", "equals": true } ],
//!   "expect_status": "ok", "expect_codes": ["ext.no_serving_unit"], "source": "..." }
//! ```

use crate::agents;
use crate::calc::CalcOutput;
use crate::cause_effect;
use crate::extinguishers;
use crate::hydraulics::model as hyd;
use crate::json::{fmt_num, Json};
use crate::profile::Profile;

#[derive(Clone, Debug)]
pub struct CaseResult {
    pub id: String,
    pub system: String,
    pub passed: bool,
    pub messages: Vec<String>,
}

impl CaseResult {
    pub fn to_json(&self) -> Json {
        Json::obj()
            .with("id", Json::str(self.id.as_str()))
            .with("system", Json::str(self.system.as_str()))
            .with("passed", Json::Bool(self.passed))
            .with("messages", Json::Arr(self.messages.iter().map(|m| Json::str(m.as_str())).collect()))
    }
}

fn execute(profile: &Profile, module: &str, input: &Json) -> Option<CallResult> {
    let out = match module {
        "extinguishers" => extinguishers::run(input.field("project"), profile, None),
        "hydraulic" => {
            let mode = if input.field("mode").as_str() == Some("required_pressure") {
                hyd::Mode::RequiredPressure
            } else {
                hyd::Mode::OperatingPoint
            };
            hyd::run(input.field("network"), input.field("scenario").as_str().unwrap_or(""), mode, profile, None)
        }
        "foam" => agents::foam::run(input.field("system"), profile, "input.system"),
        "gas" => agents::gas::run(input.field("system"), profile, "input.system"),
        "dry_chemical" => agents::dry_chem::run(input.field("system"), profile, "input.system"),
        "wet_chemical" => agents::wet_chem::run(input.field("system"), profile, "input.system"),
        "aerosol" => agents::aerosol::run(input.field("system"), profile, "input.system"),
        "cause_effect" => cause_effect::run(input.field("project"), Some(profile)),
        _ => return None,
    };
    Some(CallResult { json: out.to_json(Some(profile), ""), out })
}

struct CallResult {
    json: Json,
    out: CalcOutput,
}

pub fn run_case(profile: &Profile, case: &Json) -> CaseResult {
    let id = case.field("id").as_str().unwrap_or("?").to_string();
    let system = case.field("system").as_str().unwrap_or("").to_string();
    let mut msgs = Vec::new();
    let module = case.field("module").as_str().unwrap_or("");
    let Some(res) = execute(profile, module, case.field("input")) else {
        return CaseResult { id, system, passed: false, messages: vec![format!("módulo de prueba desconocido: '{}'", module)] };
    };
    let want_status = case.field("expect_status").as_str().unwrap_or("ok");
    if res.out.status.as_str() != want_status {
        msgs.push(format!(
            "estado '{}' (se esperaba '{}'): {}",
            res.out.status.as_str(),
            want_status,
            res.out.issues.iter().map(|i| i.code.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    for c in case.field("expect_codes").items().iter().filter_map(Json::as_str) {
        if !res.out.issues.iter().any(|i| i.code == c) {
            msgs.push(format!("no se emitió la incidencia esperada '{}'", c));
        }
    }
    let exp = case.field("expected").items();
    if exp.is_empty() && want_status == "ok" {
        msgs.push("el caso no define resultados esperados".to_string());
    }
    for e in exp {
        let path = e.field("path").as_str().unwrap_or("");
        match (res.json.at(path), e.get("value"), e.get("equals")) {
            (None, _, _) => msgs.push(format!("'{}' no existe en el resultado", path)),
            (Some(Json::Num(got)), Some(Json::Num(want)), _) => {
                let rel = e.field("tolerance_rel").as_f64();
                let abs = e.field("tolerance_abs").as_f64();
                let tol = match (rel, abs) {
                    (None, None) => 0.01 * want.abs(),
                    (r, a) => (r.unwrap_or(0.0) * want.abs()).max(a.unwrap_or(0.0)),
                };
                if (got - want).abs() > tol {
                    msgs.push(format!("{}: obtenido {} vs esperado {} (tolerancia {})", path, fmt_num(*got), fmt_num(*want), fmt_num(tol)));
                }
            }
            (Some(got), _, Some(eq)) => {
                if got != eq {
                    msgs.push(format!("{}: obtenido {} vs esperado {}", path, got.to_compact(), eq.to_compact()));
                }
            }
            _ => msgs.push(format!("'{}': expectativa mal formada", path)),
        }
    }
    CaseResult { id, system, passed: msgs.is_empty(), messages: msgs }
}

/// Ejecuta todos los casos del perfil.
pub fn run(profile: &Profile) -> Vec<CaseResult> {
    profile.raw.field("test_cases").items().iter().map(|c| run_case(profile, c)).collect()
}

pub fn summary_json(results: &[CaseResult]) -> Json {
    Json::obj()
        .with("total", Json::num(results.len() as f64))
        .with("passed", Json::num(results.iter().filter(|r| r.passed).count() as f64))
        .with("failed", Json::num(results.iter().filter(|r| !r.passed).count() as f64))
        .with("cases", Json::Arr(results.iter().map(CaseResult::to_json).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::testutil::demo_json;

    fn with_case(case: Json) -> Profile {
        let mut p = demo_json();
        p.set_at("test_cases", Json::Arr(vec![case]));
        Profile::from_json(p).unwrap()
    }

    fn foam_case(expected: f64) -> Json {
        Json::parse(&format!(
            r#"{{"id":"F1","system":"foam","module":"foam","input":{{"system":{{"id":"FM1","method":"surface_application","hazard_class":"demo_high","application":"bund",
              "concentrate":{{"type":"demo_afff","proportion_pct":3}},"area_m2":50}}}},
              "expected":[{{"path":"values.flows.solution_lpm","value":{},"tolerance_rel":1e-9}}]}}"#,
            expected
        ))
        .unwrap()
    }

    #[test]
    fn un_caso_correcto_pasa_y_uno_incorrecto_se_detecta_con_detalle() {
        let r = run(&with_case(foam_case(200.0)));
        assert!(r[0].passed, "{:?}", r[0].messages);
        let r = run(&with_case(foam_case(250.0)));
        assert!(!r[0].passed);
        assert!(r[0].messages[0].contains("solution_lpm") && r[0].messages[0].contains("200"), "{:?}", r[0].messages);
    }

    #[test]
    fn casos_de_bloqueo_y_igualdad() {
        let case = Json::parse(
            r#"{"id":"B1","system":"foam","module":"foam","expect_status":"blocked","expect_codes":["profile.table_no_row"],
            "input":{"system":{"id":"X","method":"surface_application","hazard_class":"demo_high","application":"bund","concentrate":{"type":"no_existe","proportion_pct":3},"area_m2":10}}}"#,
        )
        .unwrap();
        assert!(run(&with_case(case))[0].passed);
        let eq = Json::parse(r#"{"id":"E1","system":"foam","module":"foam","input":{"system":{"id":"FM1","method":"surface_application","hazard_class":"demo_high","application":"bund","concentrate":{"type":"demo_afff","proportion_pct":3},"area_m2":50}},"expected":[{"path":"status","equals":"ok"}]}"#).unwrap();
        assert!(run(&with_case(eq))[0].passed);
    }

    #[test]
    fn modulo_desconocido_o_sin_expectativas_falla() {
        let r = run(&with_case(Json::parse(r#"{"id":"Z","system":"x","module":"inexistente","input":{}}"#).unwrap()));
        assert!(!r[0].passed);
        let r = run(&with_case(Json::parse(r#"{"id":"Z","system":"foam","module":"foam","input":{"system":{"id":"FM1","method":"surface_application","hazard_class":"demo_high","application":"bund","concentrate":{"type":"demo_afff","proportion_pct":3},"area_m2":50}}}"#).unwrap()));
        assert!(!r[0].passed && r[0].messages[0].contains("no define resultados esperados"));
        let s = summary_json(&r);
        assert_eq!(s.field("failed").as_f64(), Some(1.0));
    }
}
