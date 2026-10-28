//! Aerosol condensado (contrato 5.8). Datos de producto desde el listado del fabricante.
//!  * `aerosol.total_flooding.v1`:  m = V * densidad_listada * factor_de_riesgo(perfil)
//!  * unidades = max( ceil(m / masa_unidad), ceil(V / volumen_máx_por_unidad) )
//!  * distancia libre real >= distancia mínima listada

use super::{occupied_safety, sys_scope, Calc};
use crate::calc::{fingerprint, trace_step, CalcOutput};
use crate::issues::Issue;
use crate::json::{fmt_num, Json};
use crate::profile::Profile;

pub const M_TOTAL: &str = "aerosol.total_flooding.v1";

pub fn run(sys: &Json, profile: &Profile, base: &str) -> CalcOutput {
    let fp = fingerprint("aerosol", &[sys], Some(profile));
    let mut c = Calc::new(profile, "aerosol");
    c.require_scope("condensed_aerosol");
    c.require_method("aerosol.total_flooding", &[M_TOTAL]);
    let s = sys_scope(sys, base);
    let id = sys.field("id").as_str().unwrap_or("");
    let hazard = c.rd.str(&s, "hazard_class");
    let occupied = c.rd.bool(&s, "occupied");
    let enc = c.rd.obj(&s, "enclosure");
    let vol = enc.as_ref().and_then(|e| c.rd.f64_pos(e, "volume_m3"));
    let prod = c.rd.obj(&s, "product");
    let (dens, unit_g, maxv, minclr, lref) = match &prod {
        Some(p) => (
            c.rd.f64_pos(p, "design_density_g_m3"),
            c.rd.f64_pos(p, "unit_mass_g"),
            c.rd.f64_pos(p, "max_volume_per_unit_m3"),
            c.rd.f64_nonneg(p, "min_clearance_m"),
            c.rd.str(p, "listing_ref"),
        ),
        None => (None, None, None, None, None),
    };
    let actual = c.rd.f64_nonneg(&s, "actual_clearance_m");
    let (Some(hazard), Some(occupied), Some(vol), Some(dens), Some(unit_g), Some(maxv), Some(minclr), Some(lref), Some(actual)) =
        (hazard, occupied, vol, dens, unit_g, maxv, minclr, lref, actual)
    else {
        return c.finish("aerosol", fp, Json::Null);
    };
    let factor = c.lookup("aerosol.design_factor", &[("hazard_class", hazard)], "-", Some(id));
    let Some(factor) = factor else { return c.finish("aerosol", fp, Json::Null) };
    let mass_g = vol * dens * factor.value;
    let by_mass = (mass_g / unit_g - 1e-9).ceil().max(1.0);
    let by_volume = (vol / maxv - 1e-9).ceil().max(1.0);
    let units = by_mass.max(by_volume);
    if actual + 1e-9 < minclr {
        c.push(
            Issue::error("aerosol.clearance_insufficient")
                .module("aerosol")
                .path(&s.key_path("actual_clearance_m"))
                .object(id)
                .param("actual_m", fmt_num(actual))
                .param("min_m", fmt_num(minclr)),
        );
    }
    occupied_safety(&mut c, &s, occupied, id, "safety");
    if occupied {
        c.push(Issue::warning("aerosol.occupied_review").module("aerosol").path(&s.key_path("occupied")).object(id));
    }
    c.trace.push(trace_step(
        "aerosol_quantity",
        "m = V * densidad_listada * factor; n = max(ceil(m/m_unidad), ceil(V/V_max_unidad))",
        Json::obj()
            .with("V_m3", Json::num(vol))
            .with("density_g_m3", Json::num(dens))
            .with("factor", Json::num(factor.value))
            .with("factor_ref", Json::str(factor.reference.as_str()))
            .with("listing_ref", Json::str(lref)),
        Json::obj().with("mass_g", Json::num(mass_g)).with("units", Json::num(units)),
    ));
    let values = Json::obj()
        .with("agent_mass_g", Json::num(mass_g))
        .with("units_by_mass", Json::num(by_mass))
        .with("units_by_volume", Json::num(by_volume))
        .with("units_required", Json::num(units))
        .with("design_factor", Json::num(factor.value))
        .with("clearance_m", Json::obj().with("actual", Json::num(actual)).with("min_listed", Json::num(minclr)))
        .with("listing_ref", Json::str(lref));
    c.finish("aerosol", fp, values)
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;
    use crate::calc::Status;

    fn sys() -> Json {
        Json::parse(
            r#"{"id":"AE1","hazard_class":"demo_high","occupied":false,"actual_clearance_m":1.5,
            "enclosure":{"volume_m3":90},
            "product":{"design_density_g_m3":100,"unit_mass_g":2500,"max_volume_per_unit_m3":40,"min_clearance_m":1.0,"listing_ref":"Listado Y rev.1"}}"#,
        )
        .unwrap()
    }

    #[test]
    fn masa_y_numero_de_unidades_por_masa_y_por_volumen() {
        let o = run(&sys(), &demo(), "systems.aerosol.0");
        assert_eq!(o.status, Status::Ok, "{:?}", o.issues);
        // factor demo_high = 1.2: m = 90*100*1.2 = 10800 g; por masa ceil(10800/2500) = 5; por volumen ceil(90/40) = 3
        assert!((val(&o.values, "agent_mass_g") - 10800.0).abs() < 1e-9);
        assert_eq!(val(&o.values, "units_by_mass"), 5.0);
        assert_eq!(val(&o.values, "units_by_volume"), 3.0);
        assert_eq!(val(&o.values, "units_required"), 5.0);
        // si el volumen manda
        let mut s = sys();
        s.set_at("product.max_volume_per_unit_m3", Json::num(10.0));
        assert_eq!(val(&run(&s, &demo(), "x").values, "units_required"), 9.0);
    }

    #[test]
    fn distancia_libre_y_espacios_ocupados() {
        let mut s = sys();
        s.set_at("actual_clearance_m", Json::num(0.5));
        assert!(has_issue(&run(&s, &demo(), "x"), "aerosol.clearance_insufficient"));
        let mut s = sys();
        s.set_at("occupied", Json::Bool(true));
        assert_eq!(run(&s, &demo(), "x").status, Status::Blocked);
        s.set_at("safety", Json::parse(r#"{"predischarge_alarm":true,"evacuation_verified":true}"#).unwrap());
        let o = run(&s, &demo(), "x");
        assert_eq!(o.status, Status::Ok);
        assert!(has_issue(&o, "aerosol.occupied_review"));
    }

    #[test]
    fn faltantes_bloquean() {
        let mut s = sys();
        if let Json::Obj(m) = &mut s {
            m.remove("product");
        }
        let o = run(&s, &demo(), "systems.aerosol.0");
        assert_eq!(o.status, Status::Blocked);
        assert!(o.issues.iter().any(|i| i.code == "data.missing" && i.path == "systems.aerosol.0.product"));
        let mut s = sys();
        s.set_at("hazard_class", Json::str("inexistente"));
        assert!(has_issue(&run(&s, &demo(), "x"), "profile.table_no_row"));
    }
}
