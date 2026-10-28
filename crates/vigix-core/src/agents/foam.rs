//! Espuma (contrato 5.5): caudal de solución, concentrado, agua, volúmenes y verificación de
//! dosificador y tanque. Métodos admitidos (los elige el perfil):
//!  * `foam.surface_application.v1`:  Q_sol = tasa * A;  Q_conc = Q_sol * p/100;  V = Q * t
//!  * `foam.high_expansion_flooding.v1`:  R = (V/T + Rs) * CN * CL  [m3/min de espuma]

use super::{sys_scope, Calc};
use crate::calc::{fingerprint, trace_step, CalcOutput};
use crate::issues::Issue;
use crate::json::Json;
use crate::profile::Profile;

pub const SYSTEM: &str = "foam";
pub const M_SURFACE: &str = "foam.surface_application.v1";
pub const M_HE: &str = "foam.high_expansion_flooding.v1";

pub fn run(sys: &Json, profile: &Profile, base: &str) -> CalcOutput {
    let fp = fingerprint("foam", &[sys], Some(profile));
    let mut c = Calc::new(profile, "foam");
    c.require_scope(SYSTEM);
    let s = sys_scope(sys, base);
    let id = sys.field("id").as_str().unwrap_or("");
    let method = c.rd.enum_str(&s, "method", &["surface_application", "high_expansion_flooding"]);
    let hazard = c.rd.str(&s, "hazard_class");
    let conc = c.rd.obj(&s, "concentrate");
    let (ctype, prop) = match &conc {
        Some(cs) => (c.rd.str(cs, "type"), c.rd.f64_range(cs, "proportion_pct", 1e-9, 100.0 - 1e-9)),
        None => (None, None),
    };
    let reserve = c.param("foam.reserve_factor", "-");
    let (Some(method), Some(hazard), Some(ctype), Some(prop), Some(reserve)) = (method, hazard, ctype, prop, reserve) else {
        return c.finish("foam", fp, Json::Null);
    };
    if reserve.value < 1.0 {
        c.push(Issue::block("profile.param_invalid").module("foam").path(&reserve.path).param("param", "foam.reserve_factor"));
    }
    let values = match method {
        "surface_application" => surface(&mut c, &s, sys, hazard, ctype, prop, reserve.value, id),
        _ => high_expansion(&mut c, &s, hazard, prop, reserve.value, id),
    };
    let Some(values) = values else { return c.finish("foam", fp, Json::Null) };
    // tanque de concentrado
    if let Some(tank) = c.rd.obj(&s, "tank") {
        if let Some(cap) = c.rd.f64_pos(&tank, "capacity_l") {
            let stored = values.at("volumes.concentrate_stored_l").and_then(Json::as_f64).unwrap_or(0.0);
            if cap + 1e-9 < stored {
                c.push(
                    Issue::error("foam.tank_insufficient")
                        .module("foam")
                        .path(&tank.key_path("capacity_l"))
                        .opt_object(s.id.as_deref())
                        .param("capacity_l", crate::json::fmt_num(cap))
                        .param("required_l", crate::json::fmt_num(stored)),
                );
            }
        }
    }
    c.finish("foam", fp, values)
}

#[allow(clippy::too_many_arguments)]
fn surface(
    c: &mut Calc,
    s: &crate::reader::Scope,
    sys: &Json,
    hazard: &str,
    ctype: &str,
    prop: f64,
    reserve: f64,
    id: &str,
) -> Option<Json> {
    c.require_method("foam.surface", &[M_SURFACE])?;
    let application = c.rd.str(s, "application");
    let area = c.rd.f64_pos(s, "area_m2");
    let (application, area) = (application?, area?);
    let rate = c.lookup("foam.application_rate", &[("application", application), ("concentrate_type", ctype)], "L/min/m2", Some(id))?;
    let dur = c.lookup("foam.discharge_duration", &[("application", application), ("hazard_class", hazard)], "min", Some(id))?;
    let q_sol = rate.value * area;
    let q_conc = q_sol * prop / 100.0;
    let q_water = q_sol - q_conc;
    let v_sol = q_sol * dur.value;
    let v_conc = q_conc * dur.value;
    let v_water = q_water * dur.value;
    // mangueras manuales declaradas por el proyectista
    let mut hose_flow = 0.0;
    let mut hose_conc = 0.0;
    let mut hose_json = Vec::new();
    for (i, h) in sys.field("hose_streams").items().iter().enumerate() {
        let hs = s.child("hose_streams").item(i, h);
        let (f, d) = (c.rd.f64_pos(&hs, "flow_lpm"), c.rd.f64_pos(&hs, "duration_min"));
        if let (Some(f), Some(d)) = (f, d) {
            hose_flow += f;
            hose_conc += f * prop / 100.0 * d;
            hose_json.push(Json::obj().with("flow_lpm", Json::num(f)).with("duration_min", Json::num(d)));
        }
    }
    let total_conc = v_conc + hose_conc;
    let stored = total_conc * reserve;
    c.trace.push(trace_step(
        "foam_surface",
        "Q_sol = tasa*A; Q_conc = Q_sol*p/100; V_conc = Q_conc*t (+ mangueras); V_almacenado = V_conc_total * factor_reserva",
        Json::obj()
            .with("rate_L_min_m2", Json::num(rate.value))
            .with("area_m2", Json::num(area))
            .with("proportion_pct", Json::num(prop))
            .with("duration_min", Json::num(dur.value))
            .with("reserve_factor", Json::num(reserve))
            .with("rate_ref", Json::str(rate.reference.as_str()))
            .with("duration_ref", Json::str(dur.reference.as_str())),
        Json::obj().with("solution_flow_lpm", Json::num(q_sol)).with("concentrate_stored_l", Json::num(stored)),
    ));
    // dosificador
    let mut proportioner = Json::Null;
    let mut inlet_pressure = Json::Null;
    if let Some(p) = c.rd.obj(s, "proportioner") {
        let (lo, hi, pin) =
            (c.rd.f64_pos(&p, "min_flow_lpm"), c.rd.f64_pos(&p, "max_flow_lpm"), c.rd.f64_nonneg(&p, "min_inlet_pressure_bar"));
        if let (Some(lo), Some(hi), Some(pin)) = (lo, hi, pin) {
            let total = q_sol + hose_flow;
            let ok = total >= lo - 1e-9 && total <= hi + 1e-9;
            if !ok {
                c.push(
                    Issue::error("foam.proportioner_out_of_range")
                        .module("foam")
                        .path(&p.path)
                        .opt_object(s.id.as_deref())
                        .param("flow_lpm", crate::json::fmt_num(total))
                        .param("min_lpm", crate::json::fmt_num(lo))
                        .param("max_lpm", crate::json::fmt_num(hi)),
                );
            }
            proportioner = Json::obj()
                .with("flow_lpm", Json::num(total))
                .with("min_flow_lpm", Json::num(lo))
                .with("max_flow_lpm", Json::num(hi))
                .with("within_range", Json::Bool(ok));
            inlet_pressure = Json::num(pin);
        }
    }
    Some(
        Json::obj()
            .with("method", Json::str(M_SURFACE))
            .with(
                "flows",
                Json::obj()
                    .with("solution_lpm", Json::num(q_sol))
                    .with("concentrate_lpm", Json::num(q_conc))
                    .with("water_lpm", Json::num(q_water))
                    .with("hose_solution_lpm", Json::num(hose_flow)),
            )
            .with(
                "volumes",
                Json::obj()
                    .with("solution_l", Json::num(v_sol))
                    .with("concentrate_l", Json::num(v_conc))
                    .with("water_l", Json::num(v_water))
                    .with("hose_concentrate_l", Json::num(hose_conc))
                    .with("concentrate_total_l", Json::num(total_conc))
                    .with("concentrate_stored_l", Json::num(stored)),
            )
            .with("duration_min", Json::num(dur.value))
            .with("hose_streams", Json::Arr(hose_json))
            .with("proportioner", proportioner)
            .with("hydraulic_demand", Json::obj().with("flow_lpm", Json::num(q_sol + hose_flow)).with("pressure_bar", inlet_pressure)),
    )
}

fn high_expansion(c: &mut Calc, s: &crate::reader::Scope, hazard: &str, prop: f64, reserve: f64, id: &str) -> Option<Json> {
    c.require_method("foam.high_expansion", &[M_HE])?;
    let vol = c.rd.f64_pos(s, "enclosure_volume_m3");
    let sprinklers = c.rd.bool(s, "sprinklers_present");
    let gen = c.rd.obj(s, "generator");
    let (out, er) = match &gen {
        Some(g) => (c.rd.f64_pos(g, "foam_output_m3_min"), c.rd.f64_pos(g, "expansion_ratio")),
        None => (None, None),
    };
    let (vol, sprinklers, out, er) = (vol?, sprinklers?, out?, er?);
    let rs = if sprinklers { c.rd.f64_nonneg(s, "sprinkler_breakdown_m3_min")? } else { 0.0 };
    let t_sub = c.lookup("foam.he_submergence_time", &[("hazard_class", hazard)], "min", Some(id))?;
    let t_op = c.lookup("foam.he_operating_time", &[("hazard_class", hazard)], "min", Some(id))?;
    let cn = c.param("foam.he_shrinkage_factor", "-")?;
    let cl = c.param("foam.he_leakage_factor", "-")?;
    if t_sub.value <= 0.0 {
        c.push(Issue::block("profile.param_invalid").module("foam").path(&t_sub.path).param("param", "foam.he_submergence_time"));
        return None;
    }
    let r = (vol / t_sub.value + rs) * cn.value * cl.value;
    let n_gen = (r / out - 1e-9).ceil().max(1.0);
    let q_sol = r * 1000.0 / er; // L/min de solución (1 m3 de espuma = 1000/ER L de solución)
    let q_conc = q_sol * prop / 100.0;
    let v_conc = q_conc * t_op.value;
    let stored = v_conc * reserve;
    c.trace.push(trace_step(
        "foam_high_expansion",
        "R = (V/T + Rs) * CN * CL [m3/min]; N = ceil(R/Q_gen); Q_sol = R*1000/ER; V_conc = Q_sol*p/100*t_op",
        Json::obj()
            .with("volume_m3", Json::num(vol))
            .with("submergence_min", Json::num(t_sub.value))
            .with("rs_m3_min", Json::num(rs))
            .with("cn", Json::num(cn.value))
            .with("cl", Json::num(cl.value))
            .with("expansion_ratio", Json::num(er)),
        Json::obj().with("r_m3_min", Json::num(r)).with("generators", Json::num(n_gen)),
    ));
    Some(
        Json::obj()
            .with("method", Json::str(M_HE))
            .with("foam_discharge_m3_min", Json::num(r))
            .with("generators_required", Json::num(n_gen))
            .with(
                "flows",
                Json::obj()
                    .with("solution_lpm", Json::num(q_sol))
                    .with("concentrate_lpm", Json::num(q_conc))
                    .with("water_lpm", Json::num(q_sol - q_conc)),
            )
            .with("volumes", Json::obj().with("concentrate_l", Json::num(v_conc)).with("concentrate_stored_l", Json::num(stored)))
            .with("duration_min", Json::num(t_op.value))
            .with("hydraulic_demand", Json::obj().with("flow_lpm", Json::num(q_sol)).with("pressure_bar", Json::Null)),
    )
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;
    use crate::calc::Status;
    use crate::issues::Severity;

    fn surface_sys() -> Json {
        Json::parse(
            r#"{"id":"FM1","method":"surface_application","hazard_class":"demo_high","application":"bund",
            "concentrate":{"type":"demo_afff","proportion_pct":3},"area_m2":50,
            "hose_streams":[{"flow_lpm":100,"duration_min":20}],
            "proportioner":{"min_flow_lpm":100,"max_flow_lpm":600,"min_inlet_pressure_bar":7.0},
            "tank":{"capacity_l":300}}"#,
        )
        .unwrap()
    }

    #[test]
    fn aplicacion_superficial_coincide_con_el_calculo_manual() {
        let o = run(&surface_sys(), &demo(), "systems.foam.0");
        assert_eq!(o.status, Status::Ok, "{:?}", o.issues);
        let v = &o.values;
        // tasa 4.0 L/min/m2 (bund/demo_afff), 50 m2 => 200 L/min de solución
        assert!((val(v, "flows.solution_lpm") - 200.0).abs() < 1e-12);
        assert!((val(v, "flows.concentrate_lpm") - 6.0).abs() < 1e-12);
        assert!((val(v, "flows.water_lpm") - 194.0).abs() < 1e-12);
        // duración 15 min (bund/demo_high): 200*15 = 3000 L de solución; concentrado 90 L
        assert!((val(v, "volumes.solution_l") - 3000.0).abs() < 1e-9);
        assert!((val(v, "volumes.concentrate_l") - 90.0).abs() < 1e-9);
        // manguera: 100 L/min * 3 % * 20 min = 60 L de concentrado; total 150 L; reserva 1.2 => 180 L
        assert!((val(v, "volumes.concentrate_total_l") - 150.0).abs() < 1e-9);
        assert!((val(v, "volumes.concentrate_stored_l") - 180.0).abs() < 1e-9);
        // demanda para hidráulica: 200 + 100 = 300 L/min a 7 bar
        assert!((val(v, "hydraulic_demand.flow_lpm") - 300.0).abs() < 1e-12);
        assert!((val(v, "hydraulic_demand.pressure_bar") - 7.0).abs() < 1e-12);
        assert!(o.params_used.iter().all(|p| p.reference.starts_with("DEMO")));
        assert!(!o.issues.iter().any(|i| i.severity >= Severity::Error));
    }

    #[test]
    fn dosificador_y_tanque_fuera_de_rango_son_errores() {
        let mut s = surface_sys();
        s.set_at("proportioner.max_flow_lpm", Json::num(250.0)); // total 300 > 250
        s.set_at("tank.capacity_l", Json::num(100.0)); // 180 > 100
        let o = run(&s, &demo(), "systems.foam.0");
        assert_eq!(o.status, Status::Ok);
        assert!(has_issue(&o, "foam.proportioner_out_of_range"));
        assert!(has_issue(&o, "foam.tank_insufficient"));
    }

    #[test]
    fn combinacion_sin_fila_en_el_perfil_bloquea() {
        let mut s = surface_sys();
        s.set_at("concentrate.type", Json::str("concentrado_inventado"));
        let o = run(&s, &demo(), "systems.foam.0");
        assert_eq!(o.status, Status::Blocked);
        assert!(o.values.is_null());
        let i = o.issues.iter().find(|i| i.code == "profile.table_no_row").unwrap();
        assert_eq!(i.object.as_deref(), Some("FM1"));
        assert_eq!(i.params.get("concentrate_type").map(String::as_str), Some("concentrado_inventado"));
    }

    #[test]
    fn faltantes_y_proporcion_invalida_bloquean() {
        let mut s = surface_sys();
        if let Json::Obj(m) = &mut s {
            m.remove("area_m2");
        }
        let o = run(&s, &demo(), "systems.foam.0");
        assert!(o.issues.iter().any(|i| i.code == "data.missing" && i.path == "systems.foam.0.area_m2"));
        let mut s = surface_sys();
        s.set_at("concentrate.proportion_pct", Json::num(0.0));
        let o = run(&s, &demo(), "systems.foam.0");
        assert!(has_issue(&o, "data.out_of_range"));
        let mut s = surface_sys();
        s.set_at("concentrate.proportion_pct", Json::num(100.0));
        assert!(has_issue(&run(&s, &demo(), "x"), "data.out_of_range"));
    }

    #[test]
    fn alta_expansion_coincide_con_la_formula() {
        let s = Json::parse(
            r#"{"id":"FM2","method":"high_expansion_flooding","hazard_class":"demo_high","enclosure_volume_m3":600,
            "sprinklers_present":true,"sprinkler_breakdown_m3_min":10,
            "generator":{"foam_output_m3_min":120,"expansion_ratio":500},
            "concentrate":{"type":"demo_afff","proportion_pct":2},"tank":{"capacity_l":1000}}"#,
        )
        .unwrap();
        let o = run(&s, &demo(), "systems.foam.1");
        assert_eq!(o.status, Status::Ok, "{:?}", o.issues);
        // T = 3 min, CN = 1.15, CL = 1.2 => R = (600/3 + 10) * 1.15 * 1.2 = 289.8 m3/min
        let r = (600.0f64 / 3.0 + 10.0) * 1.15 * 1.2;
        assert!((val(&o.values, "foam_discharge_m3_min") - r).abs() < 1e-9);
        assert_eq!(val(&o.values, "generators_required"), (r / 120.0).ceil());
        // solución = R*1000/500 = 579.6 L/min; concentrado 2 % = 11.592 L/min; t_op 12 min; reserva 1.2
        assert!((val(&o.values, "flows.solution_lpm") - r * 2.0).abs() < 1e-9);
        assert!((val(&o.values, "volumes.concentrate_stored_l") - r * 2.0 * 0.02 * 12.0 * 1.2).abs() < 1e-9);
        // sin rociadores presentes no se pide Rs
        let mut s2 = s.clone();
        s2.set_at("sprinklers_present", Json::Bool(false));
        if let Json::Obj(m) = &mut s2 {
            m.remove("sprinkler_breakdown_m3_min");
        }
        let o2 = run(&s2, &demo(), "x");
        assert_eq!(o2.status, Status::Ok, "{:?}", o2.issues);
        assert!((val(&o2.values, "foam_discharge_m3_min") - (200.0 * 1.15 * 1.2)).abs() < 1e-9);
        // con rociadores y sin Rs => bloqueo (no se asume 0)
        let mut s3 = s.clone();
        if let Json::Obj(m) = &mut s3 {
            m.remove("sprinkler_breakdown_m3_min");
        }
        assert_eq!(run(&s3, &demo(), "x").status, Status::Blocked);
    }

    #[test]
    fn perfil_que_no_cubre_espuma_bloquea() {
        let mut p = demo_json();
        p.set_at("scope.systems", Json::Arr(vec![Json::str("portable_extinguishers")]));
        let prof = Profile::from_json(p).unwrap();
        let o = run(&surface_sys(), &prof, "x");
        assert!(has_issue(&o, "gate.profile_not_covering"));
        assert_eq!(o.status, Status::Blocked);
    }

    #[test]
    fn metodo_no_declarado_o_desconocido_bloquea() {
        let mut p = demo_json();
        set_method(&mut p, "foam.surface", Some("foam.otro.v9"));
        let prof = Profile::from_json(p).unwrap();
        let o = run(&surface_sys(), &prof, "x");
        assert!(has_issue(&o, "profile.method_unsupported"));
    }
}
