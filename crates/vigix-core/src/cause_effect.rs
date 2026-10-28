//! Matriz causa-efecto (contrato 5.9).
//!
//! La matriz es **documental/informativa**: el software no acciona equipos reales ni sustituye
//! la programación del panel (límite del alcance, contrato 2.3). Aquí se valida la coherencia
//! (referencias, disparos, retardos) y se genera la tabla entradas × salidas.

use crate::calc::{fingerprint, trace_step, CalcOutput, Status};
use crate::issues::Issue;
use crate::json::{fmt_num, Json};
use crate::profile::Profile;
use std::collections::BTreeMap;

pub const MODULE: &str = "cause_effect";
const OUT_KINDS: &[&str] = &[
    "agent_release",
    "alarm_horn",
    "strobe",
    "hvac_stop",
    "damper_close",
    "fuel_shutoff",
    "door_release",
    "pump_start",
    "valve_open",
    "notification",
    "other",
];
const IN_KINDS: &[&str] = &["detector", "manual_station", "flow_switch", "pressure_switch", "supervisory", "other"];
const LOGICS: &[&str] = &["any", "all", "cross_zone"];

/// Huella de entrada (matriz + sistemas de extinción + perfil + versión del motor).
pub fn input_fingerprint(project: &Json, profile: Option<&Profile>) -> String {
    fingerprint("cause_effect", &[project.field("cause_effect"), project.field("systems")], profile)
}

pub fn run(project: &Json, profile: Option<&Profile>) -> CalcOutput {
    let ce = project.field("cause_effect");
    let fp = input_fingerprint(project, profile);
    let mut issues: Vec<Issue> = Vec::new();
    let inputs = ce.field("inputs").items();
    let outputs = ce.field("outputs").items();
    let rules = ce.field("rules").items();
    let mut in_ids: BTreeMap<&str, usize> = BTreeMap::new();
    let mut out_ids: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, x) in inputs.iter().enumerate() {
        let id = x.field("id").as_str().unwrap_or("");
        if id.is_empty() {
            issues.push(Issue::block("data.missing").module(MODULE).path(&format!("cause_effect.inputs.{}.id", i)).param("field", "id"));
        } else if in_ids.insert(id, i).is_some() {
            issues.push(
                Issue::block("geo.duplicate_id").module(MODULE).path(&format!("cause_effect.inputs.{}.id", i)).object(id).param("id", id),
            );
        }
        if !IN_KINDS.contains(&x.field("kind").as_str().unwrap_or("")) {
            issues.push(
                Issue::block("data.bad_enum")
                    .module(MODULE)
                    .path(&format!("cause_effect.inputs.{}.kind", i))
                    .opt_object(Some(id))
                    .param("field", "kind")
                    .param("allowed", IN_KINDS.join("|")),
            );
        }
    }
    // sistemas de extinción existentes (para validar destinos)
    let mut systems: BTreeMap<String, &Json> = BTreeMap::new();
    for k in ["foam", "gas", "dry_chemical", "wet_chemical", "aerosol"] {
        for s in project.field("systems").field(k).items() {
            if let Some(id) = s.field("id").as_str() {
                systems.insert(id.to_string(), s);
            }
        }
    }
    for (i, x) in outputs.iter().enumerate() {
        let id = x.field("id").as_str().unwrap_or("");
        let p = format!("cause_effect.outputs.{}", i);
        if id.is_empty() {
            issues.push(Issue::block("data.missing").module(MODULE).path(&format!("{}.id", p)).param("field", "id"));
        } else if out_ids.insert(id, i).is_some() {
            issues.push(Issue::block("geo.duplicate_id").module(MODULE).path(&format!("{}.id", p)).object(id).param("id", id));
        }
        let kind = x.field("kind").as_str().unwrap_or("");
        if !OUT_KINDS.contains(&kind) {
            issues.push(
                Issue::block("data.bad_enum")
                    .module(MODULE)
                    .path(&format!("{}.kind", p))
                    .opt_object(Some(id))
                    .param("field", "kind")
                    .param("allowed", OUT_KINDS.join("|")),
            );
        }
        match x.field("delay_s") {
            Json::Num(d) if *d >= 0.0 => {}
            Json::Null => issues
                .push(Issue::block("data.missing").module(MODULE).path(&format!("{}.delay_s", p)).object(id).param("field", "delay_s")),
            _ => issues
                .push(Issue::block("data.negative").module(MODULE).path(&format!("{}.delay_s", p)).object(id).param("field", "delay_s")),
        }
        if kind == "agent_release" {
            match x.field("target_system").as_str() {
                Some(t) if systems.contains_key(t) => {}
                Some(t) => issues.push(
                    Issue::error("ce.target_unknown").module(MODULE).path(&format!("{}.target_system", p)).object(id).param("target", t),
                ),
                None => issues.push(
                    Issue::error("ce.target_unknown").module(MODULE).path(&format!("{}.target_system", p)).object(id).param("target", ""),
                ),
            }
        }
    }
    let mut triggered: BTreeMap<&str, Vec<&str>> = BTreeMap::new(); // salida -> reglas
    for (i, r) in rules.iter().enumerate() {
        let rid = r.field("id").as_str().unwrap_or("");
        let p = format!("cause_effect.rules.{}", i);
        if rid.is_empty() {
            issues.push(Issue::block("data.missing").module(MODULE).path(&format!("{}.id", p)).param("field", "id"));
        }
        let logic = r.field("logic").as_str().unwrap_or("");
        if !LOGICS.contains(&logic) {
            issues.push(
                Issue::block("data.bad_enum")
                    .module(MODULE)
                    .path(&format!("{}.logic", p))
                    .object(rid)
                    .param("field", "logic")
                    .param("allowed", LOGICS.join("|")),
            );
        }
        let rin = r.field("inputs").items();
        let rout = r.field("outputs").items();
        if rin.is_empty() || rout.is_empty() {
            issues.push(Issue::error("ce.rule_incomplete").module(MODULE).path(&p).object(rid));
        }
        if logic == "cross_zone" && rin.len() < 2 {
            issues.push(Issue::error("ce.cross_zone_needs_two").module(MODULE).path(&format!("{}.inputs", p)).object(rid));
        }
        for x in rin {
            let id = x.as_str().unwrap_or("");
            if !in_ids.contains_key(id) {
                issues.push(
                    Issue::block("ce.unknown_reference")
                        .module(MODULE)
                        .path(&format!("{}.inputs", p))
                        .object(rid)
                        .param("ref", id)
                        .param("kind", "input"),
                );
            }
        }
        for x in rout {
            let id = x.as_str().unwrap_or("");
            if !out_ids.contains_key(id) {
                issues.push(
                    Issue::block("ce.unknown_reference")
                        .module(MODULE)
                        .path(&format!("{}.outputs", p))
                        .object(rid)
                        .param("ref", id)
                        .param("kind", "output"),
                );
            } else {
                triggered.entry(id).or_default().push(rid);
            }
        }
    }
    // salidas de liberación: deben tener disparo y, si el espacio está ocupado, alarma previa y retardo
    let min_delay = profile.and_then(|p| p.param_in("gas.min_predischarge_delay", "s").ok());
    for (i, x) in outputs.iter().enumerate() {
        if x.field("kind").as_str() != Some("agent_release") {
            continue;
        }
        let id = x.field("id").as_str().unwrap_or("");
        let p = format!("cause_effect.outputs.{}", i);
        let Some(rules_of) = triggered.get(id) else {
            issues.push(Issue::error("ce.release_without_trigger").module(MODULE).path(&p).object(id));
            continue;
        };
        let target = x.field("target_system").as_str().and_then(|t| systems.get(t));
        let occupied = target.map(|t| t.field("occupied").as_bool() == Some(true)).unwrap_or(false);
        if occupied {
            // alguna regla que dispare la descarga debe disparar también una alarma
            let has_alarm = rules.iter().any(|r| {
                r.field("outputs").items().iter().any(|o| o.as_str() == Some(id))
                    && r.field("outputs").items().iter().any(|o| {
                        o.as_str()
                            .and_then(|oid| out_ids.get(oid))
                            .map(|&oi| matches!(outputs[oi].field("kind").as_str(), Some("alarm_horn") | Some("strobe")))
                            .unwrap_or(false)
                    })
            });
            if !has_alarm {
                issues.push(Issue::block("ce.release_without_predischarge_alarm").module(MODULE).path(&p).object(id));
            }
            if let (Some(d), Some(m)) = (x.field("delay_s").as_f64(), &min_delay) {
                if d + 1e-9 < m.value {
                    issues.push(
                        Issue::block("ce.release_delay_too_short")
                            .module(MODULE)
                            .path(&format!("{}.delay_s", p))
                            .object(id)
                            .param("delay_s", fmt_num(d))
                            .param("min_s", fmt_num(m.value))
                            .reference(&m.reference),
                    );
                }
            }
        }
        let _ = rules_of;
    }
    // entradas que no activan nada
    for (id, &i) in &in_ids {
        let used = rules.iter().any(|r| r.field("inputs").items().iter().any(|x| x.as_str() == Some(*id)));
        if !used {
            issues.push(Issue::warning("ce.input_unused").module(MODULE).path(&format!("cause_effect.inputs.{}", i)).object(id));
        }
    }
    // matriz: filas = entradas, columnas = salidas; celda = retardo en s (o vacío)
    let mut cells: Vec<Json> = Vec::new();
    for x in inputs {
        let xid = x.field("id").as_str().unwrap_or("");
        let mut row = Json::obj().with("input", Json::str(xid)).with("name", x.field("name").clone());
        let mut effects = Json::obj();
        for r in rules {
            if r.field("inputs").items().iter().any(|a| a.as_str() == Some(xid)) {
                for o in r.field("outputs").items() {
                    if let Some(oid) = o.as_str() {
                        let d = out_ids.get(oid).and_then(|&oi| outputs[oi].field("delay_s").as_f64()).unwrap_or(0.0);
                        let rd = r.field("delay_s").as_f64().unwrap_or(0.0);
                        effects.set(
                            oid,
                            Json::obj()
                                .with("delay_s", Json::num(d + rd))
                                .with("logic", r.field("logic").clone())
                                .with("rule", r.field("id").clone()),
                        );
                    }
                }
            }
        }
        row.set("effects", effects);
        cells.push(row);
    }
    let blocked = issues.iter().any(|i| i.severity == crate::issues::Severity::Block);
    issues.sort_by(|a, b| b.severity.cmp(&a.severity).then_with(|| a.code.cmp(&b.code)).then_with(|| a.object.cmp(&b.object)));
    let trace = vec![trace_step(
        "cause_effect",
        "matriz entradas x salidas; retardo efectivo = retardo de salida + retardo de regla",
        Json::obj().with("rules", Json::num(rules.len() as f64)),
        Json::Null,
    )];
    if blocked {
        return CalcOutput {
            kind: "cause_effect".into(),
            status: Status::Blocked,
            values: Json::Null,
            trace,
            params_used: vec![],
            issues,
            fingerprint: fp,
        };
    }
    let values = Json::obj()
        .with("inputs", ce.field("inputs").clone())
        .with("outputs", ce.field("outputs").clone())
        .with("rows", Json::Arr(cells))
        .with("informative_interface", Json::Bool(true));
    CalcOutput { kind: "cause_effect".into(), status: Status::Ok, values, trace, params_used: vec![], issues, fingerprint: fp }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::testutil::demo;
    use crate::issues::Severity;

    fn project() -> Json {
        Json::parse(
            r#"{"systems":{"gas":[{"id":"G1","occupied":true},{"id":"G2","occupied":false}],"foam":[],"dry_chemical":[],"wet_chemical":[],"aerosol":[]},
            "cause_effect":{
              "inputs":[{"id":"D1","name":"Detector A","kind":"detector"},{"id":"D2","name":"Detector B","kind":"detector"},{"id":"MS","name":"Pulsador","kind":"manual_station"}],
              "outputs":[
                {"id":"HORN","kind":"alarm_horn","delay_s":0},
                {"id":"REL1","kind":"agent_release","delay_s":30,"target_system":"G1"},
                {"id":"REL2","kind":"agent_release","delay_s":0,"target_system":"G2"},
                {"id":"HVAC","kind":"hvac_stop","delay_s":0}],
              "rules":[
                {"id":"R1","logic":"cross_zone","inputs":["D1","D2"],"outputs":["HORN","REL1","HVAC"],"delay_s":0},
                {"id":"R2","logic":"any","inputs":["MS"],"outputs":["REL2"],"delay_s":0}]}}"#,
        )
        .unwrap()
    }

    #[test]
    fn matriz_coherente_genera_filas_con_retardos() {
        let o = run(&project(), Some(&demo()));
        assert_eq!(o.status, Status::Ok, "{:?}", o.issues);
        let rows = o.values.field("rows").items();
        assert_eq!(rows.len(), 3);
        let d1 = &rows[0];
        assert_eq!(d1.at("effects.REL1.delay_s").and_then(Json::as_f64), Some(30.0));
        assert_eq!(d1.at("effects.HORN.delay_s").and_then(Json::as_f64), Some(0.0));
        assert!(d1.at("effects.REL2").is_none());
        assert_eq!(o.values.field("informative_interface").as_bool(), Some(true));
        assert!(!o.issues.iter().any(|i| i.severity >= Severity::Error), "{:?}", o.issues);
    }

    #[test]
    fn descarga_sin_disparo_o_con_destino_inexistente() {
        let mut p = project();
        p.set_at("cause_effect.rules", Json::arr());
        let o = run(&p, Some(&demo()));
        assert!(o.issues.iter().any(|i| i.code == "ce.release_without_trigger" && i.object.as_deref() == Some("REL1")));
        let mut p = project();
        p.set_at("cause_effect.outputs.1.target_system", Json::str("G99"));
        assert!(run(&p, Some(&demo())).issues.iter().any(|i| i.code == "ce.target_unknown" && i.object.as_deref() == Some("REL1")));
    }

    #[test]
    fn espacio_ocupado_exige_alarma_previa_y_retardo_minimo() {
        // quitar la bocina de la regla R1 => la descarga ocupada queda sin alarma
        let mut p = project();
        p.set_at("cause_effect.rules.0.outputs", Json::parse(r#"["REL1","HVAC"]"#).unwrap());
        let o = run(&p, Some(&demo()));
        assert_eq!(o.status, Status::Blocked);
        assert!(o.issues.iter().any(|i| i.code == "ce.release_without_predischarge_alarm" && i.object.as_deref() == Some("REL1")));
        // retardo menor que el mínimo del perfil (20 s)
        let mut p = project();
        p.set_at("cause_effect.outputs.1.delay_s", Json::num(5.0));
        let o = run(&p, Some(&demo()));
        assert!(o.issues.iter().any(|i| i.code == "ce.release_delay_too_short" && i.reference.is_some()));
        // la salida G2 (no ocupado) sin alarma ni retardo es admisible
        assert!(!run(&project(), Some(&demo()))
            .issues
            .iter()
            .any(|i| i.object.as_deref() == Some("REL2") && i.severity >= Severity::Error));
    }

    #[test]
    fn referencias_inexistentes_duplicados_y_tipos_invalidos_bloquean() {
        let mut p = project();
        p.set_at("cause_effect.rules.0.inputs", Json::parse(r#"["D1","D9"]"#).unwrap());
        let o = run(&p, None);
        assert_eq!(o.status, Status::Blocked);
        assert!(o.issues.iter().any(|i| i.code == "ce.unknown_reference" && i.params.get("ref").map(String::as_str) == Some("D9")));
        let mut p = project();
        p.set_at("cause_effect.inputs.1.id", Json::str("D1"));
        assert!(run(&p, None).issues.iter().any(|i| i.code == "geo.duplicate_id"));
        let mut p = project();
        p.set_at("cause_effect.outputs.0.kind", Json::str("explotar"));
        assert!(run(&p, None).issues.iter().any(|i| i.code == "data.bad_enum"));
        let mut p = project();
        p.set_at("cause_effect.outputs.0.delay_s", Json::Null);
        assert!(run(&p, None).issues.iter().any(|i| i.code == "data.missing" && i.path.ends_with("delay_s")));
    }

    #[test]
    fn reglas_incompletas_y_entradas_sin_uso() {
        let mut p = project();
        p.set_at("cause_effect.rules.1.outputs", Json::arr());
        p.set_at("cause_effect.rules.0.inputs", Json::parse(r#"["D1"]"#).unwrap());
        let o = run(&p, Some(&demo()));
        assert!(o.issues.iter().any(|i| i.code == "ce.rule_incomplete"));
        assert!(o.issues.iter().any(|i| i.code == "ce.cross_zone_needs_two"));
        assert!(o.issues.iter().any(|i| i.code == "ce.input_unused" && i.object.as_deref() == Some("D2")));
    }
}
