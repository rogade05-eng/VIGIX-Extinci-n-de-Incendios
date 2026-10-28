//! Químico húmedo para cocinas (contrato 5.8). Se basa en el **listado del fabricante** (datos
//! aportados en el sistema, con referencia): el motor no inventa cantidades de boquillas.
//!  * boquillas necesarias por aparato = ceil(área / cobertura listada);
//!  * suma de números de flujo <= capacidad listada del tanque;
//!  * plenum y ducto: boquillas provistas >= requeridas por el listado;
//!  * corte de combustible/energía si el perfil lo exige.

use super::{sys_scope, Calc};
use crate::calc::{fingerprint, trace_step, CalcOutput};
use crate::issues::Issue;
use crate::json::{fmt_num, Json};
use crate::profile::Profile;

pub const M_LISTING: &str = "wet_chemical.listing_based.v1";

pub fn run(sys: &Json, profile: &Profile, base: &str) -> CalcOutput {
    let fp = fingerprint("wet_chemical", &[sys], Some(profile));
    let mut c = Calc::new(profile, "wet_chemical");
    c.require_scope("wet_chemical_kitchen");
    c.require_method("wet_chemical.kitchen", &[M_LISTING]);
    let s = sys_scope(sys, base);
    let id = sys.field("id").as_str().unwrap_or("");
    let listing = c.rd.str(&s, "listing_ref");
    let apps = c.rd.arr(&s, "appliances");
    let tank = c.rd.obj(&s, "tank");
    let cap = tank.as_ref().and_then(|t| c.rd.f64_pos(t, "max_flow_numbers"));
    let shutoff_req = c.flag("kitchen.fuel_shutoff_required");
    let (Some(listing), Some(apps), Some(cap), Some(shutoff_req)) = (listing, apps, cap, shutoff_req) else {
        return c.finish("wet_chemical", fp, Json::Null);
    };
    if apps.is_empty() {
        c.push(Issue::block("kitchen.no_appliances").module("wet_chemical").path(&s.key_path("appliances")).object(id));
        return c.finish("wet_chemical", fp, Json::Null);
    }
    let mut total_flow_numbers = 0.0;
    let mut apps_json = Vec::new();
    let aps = s.child("appliances");
    for (i, a) in apps.iter().enumerate() {
        let sc = aps.item(i, a);
        let aid = a.field("id").as_str().unwrap_or("?");
        let sc = sc.with_id(aid);
        let typ = c.rd.str(&sc, "type");
        let area = c.rd.f64_pos(&sc, "area_m2");
        let nz = c.rd.obj(&sc, "nozzle");
        let (cov, fnum) = match &nz {
            Some(n) => (c.rd.f64_pos(n, "coverage_area_m2"), c.rd.f64_pos(n, "flow_number")),
            None => (None, None),
        };
        let count = c.rd.f64_nonneg(&sc, "nozzle_count");
        let (Some(typ), Some(area), Some(cov), Some(fnum), Some(count)) = (typ, area, cov, fnum, count) else { continue };
        let required = (area / cov - 1e-9).ceil().max(1.0);
        if count + 1e-9 < required {
            c.push(
                Issue::error("kitchen.nozzles_insufficient")
                    .module("wet_chemical")
                    .path(&sc.key_path("nozzle_count"))
                    .object(aid)
                    .param("required", fmt_num(required))
                    .param("provided", fmt_num(count))
                    .param("type", typ),
            );
        }
        total_flow_numbers += count * fnum;
        apps_json.push(
            Json::obj()
                .with("id", Json::str(aid))
                .with("type", Json::str(typ))
                .with("nozzles_required", Json::num(required))
                .with("nozzles_provided", Json::num(count))
                .with("flow_numbers", Json::num(count * fnum)),
        );
    }
    // plenum y ducto: requeridos por el listado del fabricante
    let mut zones_json = Vec::new();
    for zone in ["plenum", "duct"] {
        if let Some(z) = c.rd.opt_obj(&s, zone) {
            let req = c.rd.f64_nonneg(&z, "required_nozzles");
            let prov = c.rd.f64_nonneg(&z, "nozzle_count");
            let fnum = c.rd.f64_nonneg(&z, "flow_number_each");
            if let (Some(req), Some(prov), Some(fnum)) = (req, prov, fnum) {
                if prov + 1e-9 < req {
                    c.push(
                        Issue::error("kitchen.zone_nozzles_insufficient")
                            .module("wet_chemical")
                            .path(&z.key_path("nozzle_count"))
                            .object(id)
                            .param("zone", zone)
                            .param("required", fmt_num(req))
                            .param("provided", fmt_num(prov)),
                    );
                }
                total_flow_numbers += prov * fnum;
                zones_json
                    .push(Json::obj().with("zone", Json::str(zone)).with("required", Json::num(req)).with("provided", Json::num(prov)));
            }
        }
    }
    if total_flow_numbers > cap * (1.0 + 1e-9) {
        c.push(
            Issue::error("kitchen.tank_capacity_exceeded")
                .module("wet_chemical")
                .path(&s.key_path("tank"))
                .object(id)
                .param("flow_numbers", fmt_num(total_flow_numbers))
                .param("capacity", fmt_num(cap)),
        );
    }
    // corte de combustible/energía
    let fs = c.rd.obj(&s, "fuel_shutoff");
    let provided = fs.as_ref().and_then(|f| c.rd.bool(f, "provided"));
    if let (true, Some(false)) = (shutoff_req, provided) {
        c.push(Issue::error("kitchen.fuel_shutoff_missing").module("wet_chemical").path(&s.key_path("fuel_shutoff")).object(id));
    }
    c.trace.push(trace_step(
        "kitchen_listing",
        "n_req = ceil(A/cobertura_listada); sum(n*numero_de_flujo) <= capacidad_listada",
        Json::obj().with("listing_ref", Json::str(listing)).with("capacity_flow_numbers", Json::num(cap)),
        Json::obj().with("total_flow_numbers", Json::num(total_flow_numbers)),
    ));
    let values = Json::obj()
        .with("listing_ref", Json::str(listing))
        .with("appliances", Json::Arr(apps_json))
        .with("zones", Json::Arr(zones_json))
        .with("total_flow_numbers", Json::num(total_flow_numbers))
        .with("tank_capacity_flow_numbers", Json::num(cap))
        .with("fuel_shutoff_required", Json::Bool(shutoff_req))
        .with(
            "fuel_shutoff_provided",
            match provided {
                Some(b) => Json::Bool(b),
                None => Json::Null,
            },
        );
    c.finish("wet_chemical", fp, values)
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;
    use crate::calc::Status;

    fn kitchen() -> Json {
        Json::parse(
            r#"{"id":"K1","listing_ref":"Listado fabricante X rev.3","tank":{"max_flow_numbers":12},
            "appliances":[
              {"id":"A1","type":"fryer","area_m2":0.9,"nozzle":{"coverage_area_m2":0.5,"flow_number":1},"nozzle_count":2},
              {"id":"A2","type":"griddle","area_m2":0.6,"nozzle":{"coverage_area_m2":0.6,"flow_number":2},"nozzle_count":1}],
            "plenum":{"required_nozzles":1,"nozzle_count":1,"flow_number_each":2},
            "duct":{"required_nozzles":1,"nozzle_count":1,"flow_number_each":2},
            "fuel_shutoff":{"provided":true}}"#,
        )
        .unwrap()
    }

    #[test]
    fn cocina_cumple_con_el_listado() {
        let o = run(&kitchen(), &demo(), "systems.wet_chemical.0");
        assert_eq!(o.status, Status::Ok, "{:?}", o.issues);
        // A1: ceil(0.9/0.5) = 2 ✓; A2: ceil(0.6/0.6)=1 ✓; flujos 2*1 + 1*2 + 2 + 2 = 8 <= 12
        assert_eq!(val(&o.values, "total_flow_numbers"), 8.0);
        assert!(!o.issues.iter().any(|i| i.severity >= crate::issues::Severity::Error), "{:?}", o.issues);
    }

    #[test]
    fn faltantes_de_boquillas_capacidad_y_corte_de_combustible() {
        let mut s = kitchen();
        s.set_at("appliances.0.nozzle_count", Json::num(1.0));
        s.set_at("tank.max_flow_numbers", Json::num(4.0)); // total tras quitar boquillas = 5
        s.set_at("fuel_shutoff.provided", Json::Bool(false));
        s.set_at("duct.nozzle_count", Json::num(0.0));
        let o = run(&s, &demo(), "x");
        assert_eq!(o.status, Status::Ok);
        for code in [
            "kitchen.nozzles_insufficient",
            "kitchen.tank_capacity_exceeded",
            "kitchen.fuel_shutoff_missing",
            "kitchen.zone_nozzles_insufficient",
        ] {
            assert!(has_issue(&o, code), "falta {}: {:?}", code, o.issues);
        }
        let i = o.issues.iter().find(|i| i.code == "kitchen.nozzles_insufficient").unwrap();
        assert_eq!(i.object.as_deref(), Some("A1"));
        assert_eq!(i.params.get("required").map(String::as_str), Some("2"));
    }

    #[test]
    fn sin_referencia_al_listado_o_sin_aparatos_bloquea() {
        let mut s = kitchen();
        if let Json::Obj(m) = &mut s {
            m.remove("listing_ref");
        }
        let o = run(&s, &demo(), "x");
        assert_eq!(o.status, Status::Blocked);
        assert!(o.issues.iter().any(|i| i.code == "data.missing" && i.path.ends_with("listing_ref")));
        let mut s = kitchen();
        s.set_at("appliances", Json::arr());
        assert!(has_issue(&run(&s, &demo(), "x"), "kitchen.no_appliances"));
    }
}
