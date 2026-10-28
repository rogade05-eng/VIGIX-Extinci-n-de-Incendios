//! Gases (contrato 5.6): agentes limpios halocarbonados, inertes y CO2 por inundación total.
//!
//! Métodos (los elige el perfil):
//!  * `gas.halocarbon_total_flooding.v1`:  S = k1 + k2*T;  W = (V/S) * C/(100-C) * Ca   [kg]
//!  * `gas.inert_total_flooding.v1`:       X = (Vs/S) * ln(100/(100-C)) * Ca;  V_gas = X * V [m3 de referencia]
//!  * `gas.co2_total_flooding.v1`:         W = V * FF * MCF   [kg]
//!
//! Seguridad humana: los límites NOAEL/LOAEL y la política para espacios ocupados son datos del
//! perfil; si el espacio está ocupado y falta cualquier confirmación crítica (alarma de
//! predescarga, retardo, evacuación, bloqueo/odorizante en CO2) el resultado se **bloquea**.
//! El motor NO evalúa alivio de presión ni estanqueidad: lo declara en cada resultado.

use super::{sys_scope, Calc};
use crate::calc::{fingerprint, trace_step, CalcOutput};
use crate::issues::Issue;
use crate::json::{fmt_num, Json};
use crate::profile::Profile;
use crate::reader::Scope;

pub const M_HALO: &str = "gas.halocarbon_total_flooding.v1";
pub const M_INERT: &str = "gas.inert_total_flooding.v1";
pub const M_CO2: &str = "gas.co2_total_flooding.v1";

/// Datos comunes ya validados.
struct Common<'a> {
    id: &'a str,
    hazard: &'a str,
    occupied: bool,
    volume_m3: f64,
    temp_c: f64,
    altitude_m: f64,
    cyl_scope: Scope<'a>,
}

/// Resultado de la cantidad de agente.
struct Qty {
    /// Masa de agente (kg) para clean_agent y co2.
    mass_kg: Option<f64>,
    /// Volumen de gas a condiciones de referencia (m3) para inertes.
    gas_m3: Option<f64>,
    concentration_pct: Option<f64>,
    details: Json,
}

pub fn run(sys: &Json, profile: &Profile, base: &str) -> CalcOutput {
    let fp = fingerprint("gas", &[sys], Some(profile));
    let mut c = Calc::new(profile, "gas");
    let s = sys_scope(sys, base);
    let Some(family) = c.rd.enum_str(&s, "family", &["clean_agent", "inert", "co2"]) else {
        return c.finish("gas", fp, Json::Null);
    };
    let (system_type, mkey, mid) = match family {
        "clean_agent" => ("gas_clean_agent", "gas.clean_agent", M_HALO),
        "inert" => ("gas_inert", "gas.inert", M_INERT),
        _ => ("gas_co2", "gas.co2", M_CO2),
    };
    c.require_scope(system_type);
    c.require_method(mkey, &[mid]);

    let hazard = c.rd.str(&s, "hazard_class");
    let occupied = c.rd.bool(&s, "occupied");
    let t_dis = c.rd.f64_pos(&s, "discharge_time_s");
    let enc = c.rd.obj(&s, "enclosure");
    let (vol, temp, alt) = match &enc {
        Some(e) => (c.rd.f64_pos(e, "volume_m3"), c.rd.f64(e, "min_temperature_c"), c.rd.f64(e, "altitude_m")),
        None => (None, None, None),
    };
    let cyl = c.rd.obj(&s, "cylinder");
    let hold = c.rd.opt_f64(&s, "hold_time_min");
    let max_dis = c.param(&format!("gas.{}.max_discharge_time", family), "s");
    let hold_req = c.param_opt(&format!("gas.{}.hold_time", family), "min");
    let (Some(hazard), Some(occupied), Some(t_dis), Some(vol), Some(temp), Some(alt), Some(cyl), Some(max_dis)) =
        (hazard, occupied, t_dis, vol, temp, alt, cyl, max_dis)
    else {
        return c.finish("gas", fp, Json::Null);
    };
    let id = sys.field("id").as_str().unwrap_or("");
    let cm = Common { id, hazard, occupied, volume_m3: vol, temp_c: temp, altitude_m: alt, cyl_scope: cyl };

    let qty = match family {
        "clean_agent" | "inert" => halo_or_inert(&mut c, &s, &cm, family),
        _ => co2(&mut c, &s, &cm),
    };
    let Some(qty) = qty else { return c.finish("gas", fp, Json::Null) };

    // ---- cilindros
    let mut cylinders = Json::Null;
    if let (Some(mass), true) = (qty.mass_kg, family != "inert") {
        let maxf = c.rd.f64_pos(&cm.cyl_scope, "max_fill_kg");
        let minf = c.rd.f64_nonneg(&cm.cyl_scope, "min_fill_kg");
        if let (Some(maxf), Some(minf)) = (maxf, minf) {
            let n = (mass / maxf - 1e-9).ceil().max(1.0);
            let each = mass / n;
            if each + 1e-9 < minf {
                c.push(
                    Issue::error("gas.cylinder_underfilled")
                        .module("gas")
                        .path(&cm.cyl_scope.path)
                        .object(id)
                        .param("fill_kg", fmt_num(each))
                        .param("min_kg", fmt_num(minf)),
                );
            }
            cylinders = Json::obj()
                .with("count", Json::num(n))
                .with("fill_kg_each", Json::num(each))
                .with("max_fill_kg", Json::num(maxf))
                .with("min_fill_kg", Json::num(minf));
        }
    }
    if let (Some(gas), true) = (qty.gas_m3, family == "inert") {
        if let Some(per) = c.rd.f64_pos(&cm.cyl_scope, "gas_volume_m3") {
            let n = (gas / per - 1e-9).ceil().max(1.0);
            cylinders = Json::obj().with("count", Json::num(n)).with("gas_volume_m3_each", Json::num(per));
        }
    }

    // ---- tiempo de descarga y retención
    if t_dis > max_dis.value * (1.0 + 1e-9) {
        c.push(
            Issue::error("gas.discharge_time_exceeds")
                .module("gas")
                .path(&s.key_path("discharge_time_s"))
                .object(id)
                .param("time_s", fmt_num(t_dis))
                .param("limit_s", fmt_num(max_dis.value))
                .reference(&max_dis.reference),
        );
    }
    if let Some(hr) = &hold_req {
        match hold {
            Some(h) if h + 1e-9 < hr.value => c.push(
                Issue::error("gas.hold_time_short")
                    .module("gas")
                    .path(&s.key_path("hold_time_min"))
                    .object(id)
                    .param("hold_min", fmt_num(h))
                    .param("required_min", fmt_num(hr.value))
                    .reference(&hr.reference),
            ),
            Some(_) => {}
            None => c.push(
                Issue::warning("gas.hold_time_untested")
                    .module("gas")
                    .path(&s.key_path("hold_time_min"))
                    .object(id)
                    .param("required_min", fmt_num(hr.value))
                    .reference(&hr.reference),
            ),
        }
    }

    // ---- seguridad de personas
    safety(&mut c, &s, &cm, family);

    // ---- limitaciones declaradas del software
    c.push(Issue::warning("gas.venting_not_evaluated").module("gas").path(&s.path).object(id));
    c.push(Issue::warning("gas.enclosure_integrity_not_evaluated").module("gas").path(&s.path).object(id));

    let values = values_json(family, mid, &qty, &cm, &cylinders, t_dis, max_dis.value, hold, hold_req.as_ref().map(|p| p.value));
    c.finish("gas", fp, values)
}

#[allow(clippy::too_many_arguments)]
fn values_json(
    family: &str,
    mid: &str,
    q: &Qty,
    cm: &Common,
    cylinders: &Json,
    t_dis: f64,
    max_dis: f64,
    hold: Option<f64>,
    hold_req: Option<f64>,
) -> Json {
    Json::obj()
        .with("family", Json::str(family))
        .with("method", Json::str(mid))
        .with("occupied", Json::Bool(cm.occupied))
        .with("enclosure_volume_m3", Json::num(cm.volume_m3))
        .with("agent_mass_kg", Json::opt_num(q.mass_kg))
        .with("gas_volume_reference_m3", Json::opt_num(q.gas_m3))
        .with("design_concentration_pct", Json::opt_num(q.concentration_pct))
        .with("details", q.details.clone())
        .with("cylinders", cylinders.clone())
        .with("discharge_time_s", Json::num(t_dis))
        .with("max_discharge_time_s", Json::num(max_dis))
        .with("hold_time_min", Json::opt_num(hold))
        .with("required_hold_time_min", Json::opt_num(hold_req))
        .with("not_evaluated", Json::Arr(vec![Json::str("pressure_relief_venting"), Json::str("enclosure_integrity_door_fan")]))
}

fn halo_or_inert(c: &mut Calc, s: &Scope, cm: &Common, family: &str) -> Option<Qty> {
    let agent = c.rd.str(s, "agent");
    let cc = c.rd.f64_range(s, "design_concentration_pct", 1e-9, 100.0 - 1e-9);
    let (agent, cc) = (agent?, cc?);
    let id = Some(cm.id);
    let k1 = c.lookup("gas.k1", &[("agent", agent)], "m3/kg", id);
    let k2 = c.lookup("gas.k2", &[("agent", agent)], "m3/kg/K", id);
    let applies = c.flag("gas.altitude_correction_applies");
    let ca = match applies {
        Some(true) => c.lookup_range("gas.altitude_correction", &[("family", family)], cm.altitude_m, "-", id).map(|p| p.value),
        Some(false) => Some(1.0),
        None => None,
    };
    let min_c = c.lookup("gas.min_design_concentration", &[("agent", agent), ("hazard_class", cm.hazard)], "%", id);
    let vs = if family == "inert" { c.lookup("gas.vs_reference", &[("agent", agent)], "m3/kg", id) } else { None };
    let noael = if cm.occupied {
        c.lookup("gas.noael", &[("agent", agent)], "%", id)
    } else {
        c.lookup_opt("gas.noael", &[("agent", agent)], "%", id)
    };
    let loael = if cm.occupied {
        c.lookup("gas.loael", &[("agent", agent)], "%", id)
    } else {
        c.lookup_opt("gas.loael", &[("agent", agent)], "%", id)
    };
    let policy = if cm.occupied { c.text("gas.occupied_limit") } else { None };
    if c.blocked() {
        return None;
    }
    let (k1, k2, ca, min_c) = (k1?, k2?, ca?, min_c?);
    let s_vol = k1.value + k2.value * cm.temp_c;
    if !(s_vol > 0.0 && s_vol.is_finite()) {
        c.push(Issue::block("gas.specific_volume_invalid").module("gas").path(&s.path).object(cm.id).param("s", fmt_num(s_vol)));
        return None;
    }
    // concentración mínima de diseño
    if cc + 1e-9 < min_c.value {
        c.push(
            Issue::error("gas.below_min_concentration")
                .module("gas")
                .path(&s.key_path("design_concentration_pct"))
                .object(cm.id)
                .param("design_pct", fmt_num(cc))
                .param("min_pct", fmt_num(min_c.value))
                .reference(&min_c.reference),
        );
    }
    // límites para personas
    if cm.occupied {
        let (Some(noael), Some(loael)) = (noael.as_ref(), loael.as_ref()) else { return None };
        match policy.as_deref() {
            Some("noael") => {
                if cc > noael.value + 1e-9 {
                    c.push(
                        Issue::error("gas.occupied_exceeds_noael")
                            .module("gas")
                            .path(&s.key_path("design_concentration_pct"))
                            .object(cm.id)
                            .param("design_pct", fmt_num(cc))
                            .param("noael_pct", fmt_num(noael.value))
                            .reference(&noael.reference),
                    );
                }
                if cc > loael.value + 1e-9 {
                    c.push(
                        Issue::block("gas.occupied_exceeds_loael")
                            .module("gas")
                            .path(&s.key_path("design_concentration_pct"))
                            .object(cm.id)
                            .param("design_pct", fmt_num(cc))
                            .param("loael_pct", fmt_num(loael.value))
                            .reference(&loael.reference),
                    );
                }
            }
            Some("loael") => {
                if cc > loael.value + 1e-9 {
                    c.push(
                        Issue::block("gas.occupied_exceeds_loael")
                            .module("gas")
                            .path(&s.key_path("design_concentration_pct"))
                            .object(cm.id)
                            .param("design_pct", fmt_num(cc))
                            .param("loael_pct", fmt_num(loael.value))
                            .reference(&loael.reference),
                    );
                }
            }
            Some("none") => {}
            other => {
                c.push(
                    Issue::block("profile.param_invalid")
                        .module("gas")
                        .path("parameters.gas.occupied_limit")
                        .param("param", other.unwrap_or("")),
                );
                return None;
            }
        }
    }
    let (mass, gas, eq) = if family == "clean_agent" {
        let w = (cm.volume_m3 / s_vol) * (cc / (100.0 - cc)) * ca;
        (Some(w), None, "W = (V/S) * C/(100-C) * Ca;  S = k1 + k2*T")
    } else {
        let vs = vs?;
        // Si el perfil fija la constante literal (p. ej. 2,303 con log10) se usa tal cual; si no,
        // el logaritmo natural exacto (ln 10 = 2,302585...; la diferencia es de 0,018 %).
        let log_term = match c.param_opt("gas.inert.log_constant", "-") {
            Some(k) => k.value * (100.0 / (100.0 - cc)).log10(),
            None => (100.0 / (100.0 - cc)).ln(),
        };
        let x = (vs.value / s_vol) * log_term * ca;
        (None, Some(x * cm.volume_m3), "V_gas = V * (Vs/S) * ln(100/(100-C)) * Ca;  S = k1 + k2*T")
    };
    c.trace.push(trace_step(
        "gas_quantity",
        eq,
        Json::obj()
            .with("agent", Json::str(agent))
            .with("V_m3", Json::num(cm.volume_m3))
            .with("T_c", Json::num(cm.temp_c))
            .with("k1", Json::num(k1.value))
            .with("k2", Json::num(k2.value))
            .with("S_m3_kg", Json::num(s_vol))
            .with("C_pct", Json::num(cc))
            .with("Ca", Json::num(ca))
            .with("k1_ref", Json::str(k1.reference.as_str())),
        Json::obj().with("mass_kg", Json::opt_num(mass)).with("gas_m3", Json::opt_num(gas)),
    ));
    let details = Json::obj()
        .with("agent", Json::str(agent))
        .with("specific_volume_m3_kg", Json::num(s_vol))
        .with("altitude_correction", Json::num(ca))
        .with("min_design_concentration_pct", Json::num(min_c.value))
        .with("noael_pct", Json::opt_num(noael.map(|p| p.value)))
        .with("loael_pct", Json::opt_num(loael.map(|p| p.value)))
        .with("occupied_limit_policy", Json::opt_str(policy.as_deref()));
    Some(Qty { mass_kg: mass, gas_m3: gas, concentration_pct: Some(cc), details })
}

fn co2(c: &mut Calc, s: &Scope, cm: &Common) -> Option<Qty> {
    let material = c.rd.str(s, "material");
    let material = material?;
    let id = Some(cm.id);
    let ff = c.lookup("gas.co2.flooding_factor", &[("hazard_class", cm.hazard)], "kg/m3", id);
    let mcf = c.lookup("gas.co2.material_conversion_factor", &[("material", material)], "-", id);
    if c.blocked() {
        return None;
    }
    let (ff, mcf) = (ff?, mcf?);
    let w = cm.volume_m3 * ff.value * mcf.value;
    c.trace.push(trace_step(
        "co2_quantity",
        "W = V * FF * MCF",
        Json::obj()
            .with("V_m3", Json::num(cm.volume_m3))
            .with("FF_kg_m3", Json::num(ff.value))
            .with("MCF", Json::num(mcf.value))
            .with("ff_ref", Json::str(ff.reference.as_str())),
        Json::obj().with("mass_kg", Json::num(w)),
    ));
    // CO2 en espacios ocupados: riesgo elevado -> advertencia reforzada permanente
    if cm.occupied {
        c.push(Issue::warning("safety.co2_high_risk").module("gas").path(&s.path).object(cm.id));
    }
    let details = Json::obj()
        .with("flooding_factor_kg_m3", Json::num(ff.value))
        .with("material", Json::str(material))
        .with("material_conversion_factor", Json::num(mcf.value));
    Some(Qty { mass_kg: Some(w), gas_m3: None, concentration_pct: None, details })
}

/// Confirmaciones críticas de seguridad: en espacios ocupados, cualquier ausencia es **bloqueo**.
fn safety(c: &mut Calc, s: &Scope, cm: &Common, family: &str) {
    let sf = s.child("safety");
    if !cm.occupied {
        c.push(Issue::info("gas.unoccupied_declared").module("gas").path(&s.key_path("occupied")).object(cm.id));
        return;
    }
    if sf.j.is_null() {
        c.push(Issue::block("data.missing").module("gas").path(&sf.path).object(cm.id).param("field", "safety"));
        return;
    }
    let alarm = c.rd.bool(&sf, "predischarge_alarm");
    let evac = c.rd.bool(&sf, "evacuation_verified");
    let delay = c.rd.f64_nonneg(&sf, "time_delay_s");
    let min_delay = c.param("gas.min_predischarge_delay", "s");
    if alarm == Some(false) {
        c.push(Issue::block("safety.predischarge_alarm_missing").module("gas").path(&sf.key_path("predischarge_alarm")).object(cm.id));
    }
    if evac == Some(false) {
        c.push(Issue::block("safety.evacuation_not_verified").module("gas").path(&sf.key_path("evacuation_verified")).object(cm.id));
    }
    if let (Some(d), Some(m)) = (delay, &min_delay) {
        if d + 1e-9 < m.value {
            c.push(
                Issue::block("safety.delay_too_short")
                    .module("gas")
                    .path(&sf.key_path("time_delay_s"))
                    .object(cm.id)
                    .param("delay_s", fmt_num(d))
                    .param("min_s", fmt_num(m.value))
                    .reference(&m.reference),
            );
        }
    }
    if family == "co2" {
        if c.rd.bool(&sf, "lockout_valve") == Some(false) {
            c.push(Issue::block("safety.co2_lockout_missing").module("gas").path(&sf.key_path("lockout_valve")).object(cm.id));
        }
        if c.rd.bool(&sf, "odorizer") == Some(false) {
            c.push(Issue::block("safety.co2_odorizer_missing").module("gas").path(&sf.key_path("odorizer")).object(cm.id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;
    use crate::calc::Status;
    use crate::issues::Severity;

    fn clean() -> Json {
        Json::parse(
            r#"{"id":"G1","family":"clean_agent","agent":"demo_halocarbon","hazard_class":"demo_medium","occupied":true,
            "enclosure":{"volume_m3":200,"min_temperature_c":20,"altitude_m":100},
            "design_concentration_pct":8.5,"discharge_time_s":10,"hold_time_min":12,
            "cylinder":{"max_fill_kg":60,"min_fill_kg":20},
            "safety":{"predischarge_alarm":true,"time_delay_s":30,"evacuation_verified":true}}"#,
        )
        .unwrap()
    }

    fn inert() -> Json {
        Json::parse(
            r#"{"id":"G2","family":"inert","agent":"demo_inert","hazard_class":"demo_medium","occupied":false,
            "enclosure":{"volume_m3":300,"min_temperature_c":15,"altitude_m":600},
            "design_concentration_pct":40,"discharge_time_s":50,
            "cylinder":{"gas_volume_m3":12}}"#,
        )
        .unwrap()
    }

    fn co2() -> Json {
        Json::parse(
            r#"{"id":"G3","family":"co2","hazard_class":"demo_high","occupied":true,"material":"demo_material_x",
            "enclosure":{"volume_m3":150,"min_temperature_c":20,"altitude_m":0},"discharge_time_s":45,
            "cylinder":{"max_fill_kg":45,"min_fill_kg":30},
            "safety":{"predischarge_alarm":true,"time_delay_s":25,"evacuation_verified":true,"lockout_valve":true,"odorizer":true}}"#,
        )
        .unwrap()
    }

    #[test]
    fn agente_limpio_coincide_con_la_formula_de_masa() {
        let o = run(&clean(), &demo(), "systems.gas.0");
        assert_eq!(o.status, Status::Ok, "{:?}", o.issues);
        // S = 0.15 + 0.0006*20 = 0.162; Ca (altitud 100 m) = 1.0; W = (200/0.162) * 8.5/91.5
        let s = 0.15 + 0.0006 * 20.0;
        let w = (200.0 / s) * (8.5 / 91.5);
        assert!((val(&o.values, "agent_mass_kg") - w).abs() < 1e-9, "{} vs {}", val(&o.values, "agent_mass_kg"), w);
        assert!((val(&o.values, "details.specific_volume_m3_kg") - s).abs() < 1e-12);
        // cilindros: ceil(W/60) con llenado uniforme
        let n = (w / 60.0).ceil();
        assert_eq!(val(&o.values, "cylinders.count"), n);
        assert!((val(&o.values, "cylinders.fill_kg_each") - w / n).abs() < 1e-9);
        // límites: demo_halocarbon NOAEL 9.0 > 8.5 => sin error; siempre se declara lo no evaluado
        assert!(!has_issue(&o, "gas.occupied_exceeds_noael"));
        assert!(has_issue(&o, "gas.venting_not_evaluated") && has_issue(&o, "gas.enclosure_integrity_not_evaluated"));
        assert_eq!(o.values.field("not_evaluated").items().len(), 2);
        assert!(!o.issues.iter().any(|i| i.severity >= Severity::Error), "{:?}", o.issues);
    }

    #[test]
    fn correccion_por_altitud_viene_de_la_tabla_del_perfil() {
        let mut s = clean();
        s.set_at("enclosure.altitude_m", Json::num(1000.0)); // tramo [500,1500) => 0.95
        let o = run(&s, &demo(), "x");
        let base = (200.0 / 0.162) * (8.5 / 91.5);
        assert!((val(&o.values, "agent_mass_kg") - base * 0.95).abs() < 1e-9);
        // altitud fuera de la tabla (5000 m) => bloqueo, no se extrapola
        s.set_at("enclosure.altitude_m", Json::num(5000.0));
        let o = run(&s, &demo(), "x");
        assert_eq!(o.status, Status::Blocked);
        assert!(has_issue(&o, "profile.out_of_table_range"));
    }

    #[test]
    fn concentracion_minima_y_noael_para_espacios_ocupados() {
        let mut s = clean();
        s.set_at("design_concentration_pct", Json::num(7.0)); // mínimo demo_medium = 8.0
        let o = run(&s, &demo(), "x");
        assert!(has_issue(&o, "gas.below_min_concentration"));
        // 9.5 % > NOAEL 9.0 % (política "noael") => error; < LOAEL 10.5 % => no bloquea
        s.set_at("design_concentration_pct", Json::num(9.5));
        let o = run(&s, &demo(), "x");
        assert_eq!(o.status, Status::Ok);
        assert!(has_issue(&o, "gas.occupied_exceeds_noael"));
        assert!(!has_issue(&o, "gas.occupied_exceeds_loael"));
        // 11 % > LOAEL => bloqueo
        s.set_at("design_concentration_pct", Json::num(11.0));
        let o = run(&s, &demo(), "x");
        assert_eq!(o.status, Status::Blocked);
        assert!(has_issue(&o, "gas.occupied_exceeds_loael"));
        // espacio NO ocupado: los límites de personas no se aplican
        s.set_at("occupied", Json::Bool(false));
        let o = run(&s, &demo(), "x");
        assert_eq!(o.status, Status::Ok, "{:?}", o.issues);
        assert!(!has_issue(&o, "gas.occupied_exceeds_noael"));
        assert!(has_issue(&o, "gas.unoccupied_declared"));
    }

    #[test]
    fn politica_de_ocupados_la_decide_el_perfil() {
        let mut p = demo_json();
        set_param(&mut p, &["parameters", "gas", "occupied_limit", "text"], Json::str("loael"));
        let prof = Profile::from_json(p).unwrap();
        let mut s = clean();
        s.set_at("design_concentration_pct", Json::num(9.5));
        let o = run(&s, &prof, "x");
        assert!(!has_issue(&o, "gas.occupied_exceeds_noael"), "con política loael 9.5 % no excede");
        let mut p = demo_json();
        set_param(&mut p, &["parameters", "gas", "occupied_limit", "text"], Json::str("otra"));
        let prof = Profile::from_json(p).unwrap();
        assert!(has_issue(&run(&clean(), &prof, "x"), "profile.param_invalid"));
    }

    #[test]
    fn inerte_coincide_con_la_formula_logaritmica() {
        let o = run(&inert(), &demo(), "systems.gas.1");
        assert_eq!(o.status, Status::Ok, "{:?}", o.issues);
        // S = 0.65 + 0.0024*15 = 0.686; Vs = 0.70; Ca (600 m) = 0.95
        let s = 0.65 + 0.0024 * 15.0;
        let x = (0.70 / s) * (100.0f64 / 60.0).ln() * 0.95;
        assert!((val(&o.values, "gas_volume_reference_m3") - x * 300.0).abs() < 1e-9);
        assert_eq!(val(&o.values, "cylinders.count"), (x * 300.0 / 12.0).ceil());
        assert!(o.values.field("agent_mass_kg").is_null());
        // La constante literal 2,303 difiere de ln 10 en 0,018 %: el perfil puede fijarla
        let alt = (0.70 / s) * 2.303 * (100.0f64 / 60.0).log10() * 0.95 * 300.0;
        assert!((alt - val(&o.values, "gas_volume_reference_m3")).abs() / alt < 3e-4);
        let mut p = demo_json();
        if let Json::Obj(root) = &mut p {
            if let Some(Json::Obj(params)) = root.get_mut("parameters") {
                if let Some(Json::Obj(g)) = params.get_mut("gas") {
                    g.insert("inert".to_string(), {
                        let mut inert = g.get("inert").cloned().unwrap();
                        inert.set(
                            "log_constant",
                            Json::obj()
                                .with("value", Json::num(2.303))
                                .with("unit", Json::str("-"))
                                .with("ref", Json::str("DEMO: constante literal")),
                        );
                        inert
                    });
                }
            }
        }
        let o2 = run(&inert(), &Profile::from_json(p).unwrap(), "x");
        assert!((val(&o2.values, "gas_volume_reference_m3") - alt).abs() < 1e-9, "con la constante del perfil se usa exactamente 2,303");
    }

    #[test]
    fn co2_por_factor_de_inundacion_y_advertencia_reforzada() {
        let o = run(&co2(), &demo(), "systems.gas.2");
        assert_eq!(o.status, Status::Ok, "{:?}", o.issues);
        // FF demo_high = 1.0 kg/m3; MCF = 1.3; W = 150*1.0*1.3 = 195 kg; cilindros 45 kg => 5
        assert!((val(&o.values, "agent_mass_kg") - 195.0).abs() < 1e-9);
        assert_eq!(val(&o.values, "cylinders.count"), 5.0);
        assert!(has_issue(&o, "safety.co2_high_risk"));
    }

    #[test]
    fn faltas_de_seguridad_en_espacio_ocupado_bloquean() {
        for (path, code) in [
            ("safety.predischarge_alarm", "safety.predischarge_alarm_missing"),
            ("safety.evacuation_verified", "safety.evacuation_not_verified"),
        ] {
            let mut s = clean();
            s.set_at(path, Json::Bool(false));
            let o = run(&s, &demo(), "x");
            assert_eq!(o.status, Status::Blocked, "{}", path);
            assert!(has_issue(&o, code), "{:?}", o.issues);
        }
        // retardo menor que el mínimo del perfil (20 s)
        let mut s = clean();
        s.set_at("safety.time_delay_s", Json::num(10.0));
        let o = run(&s, &demo(), "x");
        assert!(has_issue(&o, "safety.delay_too_short") && o.status == Status::Blocked);
        // dato ausente (null) NO se interpreta como "sí"
        let mut s = clean();
        s.set_at("safety.predischarge_alarm", Json::Null);
        let o = run(&s, &demo(), "x");
        assert_eq!(o.status, Status::Blocked);
        assert!(o.issues.iter().any(|i| i.code == "data.missing" && i.path.ends_with("predischarge_alarm")));
        // sin objeto safety en espacio ocupado
        let mut s = clean();
        if let Json::Obj(m) = &mut s {
            m.remove("safety");
        }
        assert_eq!(run(&s, &demo(), "x").status, Status::Blocked);
        // CO2: bloqueo y odorizante
        for (path, code) in [("safety.lockout_valve", "safety.co2_lockout_missing"), ("safety.odorizer", "safety.co2_odorizer_missing")] {
            let mut s = co2();
            s.set_at(path, Json::Bool(false));
            let o = run(&s, &demo(), "x");
            assert!(has_issue(&o, code) && o.status == Status::Blocked, "{}", path);
        }
    }

    #[test]
    fn descarga_retencion_y_llenado_de_cilindros() {
        let mut s = clean();
        s.set_at("discharge_time_s", Json::num(12.0)); // máximo 10 s
        s.set_at("hold_time_min", Json::num(5.0)); // requerido 10 min
        s.set_at("cylinder.min_fill_kg", Json::num(500.0));
        let o = run(&s, &demo(), "x");
        assert!(has_issue(&o, "gas.discharge_time_exceeds"));
        assert!(has_issue(&o, "gas.hold_time_short"));
        assert!(has_issue(&o, "gas.cylinder_underfilled"));
        // sin ensayo de retención => advertencia (no se asume cumplida)
        let mut s = clean();
        if let Json::Obj(m) = &mut s {
            m.remove("hold_time_min");
        }
        assert!(has_issue(&run(&s, &demo(), "x"), "gas.hold_time_untested"));
    }

    #[test]
    fn agente_inexistente_o_datos_faltantes_bloquean_sin_valores() {
        let mut s = clean();
        s.set_at("agent", Json::str("agente_inventado"));
        let o = run(&s, &demo(), "x");
        assert_eq!(o.status, Status::Blocked);
        assert!(o.values.is_null());
        assert!(has_issue(&o, "profile.table_no_row"));
        let mut s = clean();
        if let Json::Obj(m) = &mut s {
            m.remove("enclosure");
        }
        let o = run(&s, &demo(), "systems.gas.0");
        assert!(o.issues.iter().any(|i| i.code == "data.missing" && i.path == "systems.gas.0.enclosure"));
        // temperatura -273 °C o concentración 100 % no producen NaN/inf: se rechazan
        let mut s = clean();
        s.set_at("design_concentration_pct", Json::num(100.0));
        assert_eq!(run(&s, &demo(), "x").status, Status::Blocked);
        let mut s = clean();
        s.set_at("enclosure.min_temperature_c", Json::num(-1000.0));
        let o = run(&s, &demo(), "x");
        assert!(has_issue(&o, "gas.specific_volume_invalid"));
    }

    #[test]
    fn perfil_sin_cobertura_de_gases_bloquea() {
        let mut p = demo_json();
        p.set_at("scope.systems", Json::Arr(vec![Json::str("foam")]));
        let prof = Profile::from_json(p).unwrap();
        assert!(has_issue(&run(&clean(), &prof, "x"), "gate.profile_not_covering"));
    }
}
