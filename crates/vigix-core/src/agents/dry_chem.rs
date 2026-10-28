//! Polvo químico (contrato 5.7).
//!  * `dry_chemical.total_flooding.v1`:     W = V * tasa(agente, riesgo) + A_abertura * factor(agente)
//!  * `dry_chemical.local_application.v1`:  W = tasa[kg/min/m2] * A * t[min]
//!
//! Verifica además que las boquillas puedan descargar la masa dentro del tiempo de descarga.

use super::{occupied_safety, sys_scope, Calc};
use crate::calc::{fingerprint, trace_step, CalcOutput};
use crate::issues::Issue;
use crate::json::{fmt_num, Json};
use crate::profile::Profile;

pub const M_TOTAL: &str = "dry_chemical.total_flooding.v1";
pub const M_LOCAL: &str = "dry_chemical.local_application.v1";

pub fn run(sys: &Json, profile: &Profile, base: &str) -> CalcOutput {
    let fp = fingerprint("dry_chemical", &[sys], Some(profile));
    let mut c = Calc::new(profile, "dry_chemical");
    c.require_scope("dry_chemical");
    let s = sys_scope(sys, base);
    let id = sys.field("id").as_str().unwrap_or("");
    let mode = c.rd.enum_str(&s, "mode", &["total_flooding", "local_application"]);
    let agent = c.rd.str(&s, "agent");
    let hazard = c.rd.str(&s, "hazard_class");
    let occupied = c.rd.bool(&s, "occupied");
    let t_dis = c.rd.f64_pos(&s, "discharge_time_s");
    let noz = c.rd.obj(&s, "nozzles");
    let (n_noz, q_noz) = match &noz {
        Some(n) => (c.rd.f64_pos(n, "count"), c.rd.f64_pos(n, "flow_kg_s")),
        None => (None, None),
    };
    let cyl = c.rd.obj(&s, "cylinder");
    let cap = cyl.as_ref().and_then(|cs| c.rd.f64_pos(cs, "capacity_kg"));
    let (Some(mode), Some(agent), Some(hazard), Some(occupied), Some(t_dis), Some(n_noz), Some(q_noz), Some(cap)) =
        (mode, agent, hazard, occupied, t_dis, n_noz, q_noz, cap)
    else {
        return c.finish("dry_chemical", fp, Json::Null);
    };
    let mass;
    let mut details = Json::obj();
    match mode {
        "total_flooding" => {
            c.require_method("dry_chemical.total_flooding", &[M_TOTAL]);
            let enc = c.rd.obj(&s, "enclosure");
            let (vol, open) = match &enc {
                Some(e) => (c.rd.f64_pos(e, "volume_m3"), c.rd.f64_nonneg(e, "open_area_m2")),
                None => (None, None),
            };
            let rate = c.lookup("dry_chemical.flooding_rate", &[("agent", agent), ("hazard_class", hazard)], "kg/m3", Some(id));
            let of = c.lookup("dry_chemical.opening_factor", &[("agent", agent)], "kg/m2", Some(id));
            let maxt = c.param("dry_chemical.max_discharge_time", "s");
            let (Some(vol), Some(open), Some(rate), Some(of), Some(maxt)) = (vol, open, rate, of, maxt) else {
                return c.finish("dry_chemical", fp, Json::Null);
            };
            mass = vol * rate.value + open * of.value;
            if t_dis > maxt.value * (1.0 + 1e-9) {
                c.push(
                    Issue::error("drychem.discharge_time_exceeds")
                        .module("dry_chemical")
                        .path(&s.key_path("discharge_time_s"))
                        .object(id)
                        .param("time_s", fmt_num(t_dis))
                        .param("limit_s", fmt_num(maxt.value))
                        .reference(&maxt.reference),
                );
            }
            c.trace.push(trace_step(
                "drychem_total",
                "W = V*tasa + A_abertura*factor_abertura",
                Json::obj()
                    .with("V_m3", Json::num(vol))
                    .with("rate_kg_m3", Json::num(rate.value))
                    .with("open_m2", Json::num(open))
                    .with("opening_factor_kg_m2", Json::num(of.value))
                    .with("ref", Json::str(rate.reference.as_str())),
                Json::obj().with("mass_kg", Json::num(mass)),
            ));
            details.set("rate_kg_m3", Json::num(rate.value));
            details.set("opening_factor_kg_m2", Json::num(of.value));
            // espacio ocupado: el polvo reduce la visibilidad y es un riesgo respiratorio
            if occupied {
                c.push(Issue::warning("drychem.occupied_total_flooding").module("dry_chemical").path(&s.key_path("occupied")).object(id));
            }
        }
        _ => {
            c.require_method("dry_chemical.local_application", &[M_LOCAL]);
            let loc = c.rd.obj(&s, "local");
            let area = loc.as_ref().and_then(|l| c.rd.f64_pos(l, "area_m2"));
            let rate = c.lookup("dry_chemical.local_rate", &[("agent", agent), ("hazard_class", hazard)], "kg/min/m2", Some(id));
            let mint = c.param("dry_chemical.local_min_time", "s");
            let (Some(area), Some(rate), Some(mint)) = (area, rate, mint) else { return c.finish("dry_chemical", fp, Json::Null) };
            mass = rate.value * area * (t_dis / 60.0);
            if t_dis + 1e-9 < mint.value {
                c.push(
                    Issue::error("drychem.discharge_time_short")
                        .module("dry_chemical")
                        .path(&s.key_path("discharge_time_s"))
                        .object(id)
                        .param("time_s", fmt_num(t_dis))
                        .param("min_s", fmt_num(mint.value))
                        .reference(&mint.reference),
                );
            }
            c.trace.push(trace_step(
                "drychem_local",
                "W = tasa[kg/min/m2] * A * t[min]",
                Json::obj()
                    .with("rate_kg_min_m2", Json::num(rate.value))
                    .with("area_m2", Json::num(area))
                    .with("t_s", Json::num(t_dis))
                    .with("ref", Json::str(rate.reference.as_str())),
                Json::obj().with("mass_kg", Json::num(mass)),
            ));
            details.set("rate_kg_min_m2", Json::num(rate.value));
        }
    }
    // capacidad de descarga de las boquillas
    let total_rate = n_noz * q_noz;
    let t_needed = mass / total_rate;
    if t_needed > t_dis * (1.0 + 1e-9) {
        c.push(
            Issue::error("drychem.nozzle_capacity_insufficient")
                .module("dry_chemical")
                .path(&s.key_path("nozzles"))
                .object(id)
                .param("time_needed_s", fmt_num(t_needed))
                .param("discharge_time_s", fmt_num(t_dis)),
        );
    }
    let cylinders = (mass / cap - 1e-9).ceil().max(1.0);
    occupied_safety(&mut c, &s, occupied, id, "safety");
    let values = Json::obj()
        .with("mode", Json::str(mode))
        .with("agent", Json::str(agent))
        .with("agent_mass_kg", Json::num(mass))
        .with("cylinders_required", Json::num(cylinders))
        .with("nozzle_total_rate_kg_s", Json::num(total_rate))
        .with("time_needed_s", Json::num(t_needed))
        .with("discharge_time_s", Json::num(t_dis))
        .with("details", details);
    c.finish("dry_chemical", fp, values)
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;
    use crate::calc::Status;

    fn total() -> Json {
        Json::parse(
            r#"{"id":"DC1","mode":"total_flooding","agent":"demo_powder","hazard_class":"demo_medium","occupied":false,
            "discharge_time_s":25,"enclosure":{"volume_m3":120,"open_area_m2":2},
            "nozzles":{"count":4,"flow_kg_s":1.5},"cylinder":{"capacity_kg":45}}"#,
        )
        .unwrap()
    }
    fn local() -> Json {
        Json::parse(
            r#"{"id":"DC2","mode":"local_application","agent":"demo_powder","hazard_class":"demo_high","occupied":false,
            "discharge_time_s":60,"local":{"area_m2":6},"nozzles":{"count":2,"flow_kg_s":0.5},"cylinder":{"capacity_kg":20}}"#,
        )
        .unwrap()
    }

    #[test]
    fn inundacion_total_con_compensacion_de_aberturas() {
        let o = run(&total(), &demo(), "systems.dry_chemical.0");
        assert_eq!(o.status, Status::Ok, "{:?}", o.issues);
        // 120*0.65 + 2*5.0 = 88 kg; cilindros ceil(88/45) = 2; capacidad 4*1.5 = 6 kg/s => 14.67 s
        assert!((val(&o.values, "agent_mass_kg") - 88.0).abs() < 1e-9);
        assert_eq!(val(&o.values, "cylinders_required"), 2.0);
        assert!((val(&o.values, "time_needed_s") - 88.0 / 6.0).abs() < 1e-9);
        assert!(!has_issue(&o, "drychem.nozzle_capacity_insufficient"));
    }

    #[test]
    fn aplicacion_local_usa_tasa_por_area_y_tiempo() {
        let o = run(&local(), &demo(), "systems.dry_chemical.1");
        assert_eq!(o.status, Status::Ok, "{:?}", o.issues);
        // 3.0 kg/min/m2 * 6 m2 * 1 min = 18 kg
        assert!((val(&o.values, "agent_mass_kg") - 18.0).abs() < 1e-9);
        assert_eq!(val(&o.values, "cylinders_required"), 1.0);
        // capacidad 2*0.5 = 1 kg/s => 18 s <= 60 s
        assert!((val(&o.values, "time_needed_s") - 18.0).abs() < 1e-9);
    }

    #[test]
    fn limites_de_tiempo_y_capacidad_de_boquillas() {
        let mut s = total();
        s.set_at("discharge_time_s", Json::num(40.0)); // máximo 30 s
        assert!(has_issue(&run(&s, &demo(), "x"), "drychem.discharge_time_exceeds"));
        let mut s = total();
        s.set_at("nozzles.flow_kg_s", Json::num(0.1)); // 0.4 kg/s => 220 s > 25 s
        assert!(has_issue(&run(&s, &demo(), "x"), "drychem.nozzle_capacity_insufficient"));
        let mut s = local();
        s.set_at("discharge_time_s", Json::num(10.0)); // mínimo 30 s
        assert!(has_issue(&run(&s, &demo(), "x"), "drychem.discharge_time_short"));
    }

    #[test]
    fn ocupado_exige_confirmaciones_y_advierte() {
        let mut s = total();
        s.set_at("occupied", Json::Bool(true));
        let o = run(&s, &demo(), "x");
        assert_eq!(o.status, Status::Blocked); // sin objeto safety
        s.set_at("safety", Json::parse(r#"{"predischarge_alarm":true,"evacuation_verified":true}"#).unwrap());
        let o = run(&s, &demo(), "x");
        assert_eq!(o.status, Status::Ok, "{:?}", o.issues);
        assert!(has_issue(&o, "drychem.occupied_total_flooding"));
        s.set_at("safety.evacuation_verified", Json::Bool(false));
        assert!(has_issue(&run(&s, &demo(), "x"), "safety.evacuation_not_verified"));
    }

    #[test]
    fn datos_y_tablas_faltantes_bloquean() {
        let mut s = total();
        s.set_at("agent", Json::str("polvo_x"));
        let o = run(&s, &demo(), "x");
        assert_eq!(o.status, Status::Blocked);
        assert!(has_issue(&o, "profile.table_no_row"));
        let mut s = total();
        if let Json::Obj(m) = &mut s {
            m.remove("nozzles");
        }
        assert!(run(&s, &demo(), "x").issues.iter().any(|i| i.code == "data.missing" && i.path.ends_with("nozzles")));
    }
}
