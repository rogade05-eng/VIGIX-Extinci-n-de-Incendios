//! Modelo de proyecto (esquema `vigix.project/1`): plantilla, validación estructural y geométrica,
//! migración versionada y huellas de datos (contrato 4.4, 5.1, 5.2 y 9.1).
//!
//! El proyecto es un documento JSON. Las unidades están **en el nombre de la clave**
//! (`length_m`, `diameter_mm`, `pressure_bar`, `flow_lpm`...), de modo que cualquier exportación
//! conserva valores y unidades. Los datos desconocidos son `null` (nunca 0).

use crate::geometry::{self, Pt};
use crate::hash::sha256_hex;
use crate::issues::{Issue, IssueList};
use crate::json::Json;
use crate::{ENGINE_VERSION, PRODUCT_NAME};
use std::collections::BTreeMap;

pub const SCHEMA: &str = "vigix.project/1";
const MODULE: &str = "project";
const COORD_LIMIT: f64 = 1.0e6;

/// Proyecto vacío con **todas** las secciones presentes y los datos desconocidos en `null`.
pub fn new_project(id: &str, name: &str, language: &str, unit_system: &str, now_iso: &str) -> Json {
    let level = Json::obj()
        .with("id", Json::str("L1"))
        .with("name", Json::str("Nivel 1"))
        .with("elevation_m", Json::num(0.0))
        .with("height_m", Json::num(3.0))
        .with("entities", Json::arr());
    Json::obj()
        .with("schema", Json::str(SCHEMA))
        .with("app", Json::obj().with("name", Json::str(PRODUCT_NAME)).with("version", Json::str(ENGINE_VERSION)))
        .with(
            "meta",
            Json::obj()
                .with("id", Json::str(id))
                .with("name", Json::str(name))
                .with("client", Json::str(""))
                .with("address", Json::str(""))
                .with("author", Json::str(""))
                .with("reviewer", Json::str(""))
                .with("status", Json::str("borrador"))
                .with("mode", Json::str("new_design"))
                .with("design_date", Json::Null)
                .with("created_at", Json::str(now_iso))
                .with("updated_at", Json::str(now_iso))
                .with("revision", Json::num(1.0))
                .with("revisions", Json::arr()),
        )
        .with(
            "jurisdiction",
            Json::obj()
                .with("country", Json::Null)
                .with("region", Json::Null)
                .with("authority", Json::Null)
                .with("profile_code", Json::Null)
                .with("profile_version", Json::Null)
                .with("profile_sha256", Json::Null),
        )
        .with(
            "display",
            Json::obj().with("unit_system", Json::str(unit_system)).with("language", Json::str(language)).with("decimals", Json::num(2.0)),
        )
        .with(
            "site",
            Json::obj()
                .with("occupancy", Json::Null)
                .with("building_use", Json::Null)
                .with("height_m", Json::Null)
                .with("levels", Json::Arr(vec![level]))
                .with("hazards", Json::arr())
                .with("restrictions", Json::str("")),
        )
        .with(
            "water_supply",
            Json::obj()
                .with("type", Json::Null)
                .with("static_pressure_bar", Json::Null)
                .with("residual_pressure_bar", Json::Null)
                .with("test_flow_lpm", Json::Null)
                .with("elevation_m", Json::Null)
                .with("limitations", Json::str("")),
        )
        .with("assumptions", Json::arr())
        .with(
            "systems",
            Json::obj()
                .with("extinguishers", Json::obj().with("analysis", Json::obj().with("grid_m", Json::num(0.25))).with("units", Json::arr()))
                .with("water", Json::obj().with("networks", Json::arr()))
                .with("foam", Json::arr())
                .with("gas", Json::arr())
                .with("dry_chemical", Json::arr())
                .with("wet_chemical", Json::arr())
                .with("aerosol", Json::arr()),
        )
        .with("cause_effect", Json::obj().with("inputs", Json::arr()).with("outputs", Json::arr()).with("rules", Json::arr()))
        .with("catalog", Json::obj().with("items", Json::arr()))
        .with(
            "budget",
            Json::obj().with("currency", Json::Null).with("lines", Json::arr()).with("taxes", Json::arr()).with("notes", Json::str("")),
        )
        .with("results", Json::obj())
        .with("exceptions", Json::arr())
        .with("reviews", Json::arr())
        .with("inspections", Json::arr())
}

const TOP_KEYS: &[&str] = &[
    "schema",
    "meta",
    "jurisdiction",
    "display",
    "site",
    "water_supply",
    "assumptions",
    "systems",
    "cause_effect",
    "catalog",
    "budget",
    "results",
    "exceptions",
    "reviews",
];

const SYSTEM_ARRAYS: &[&str] = &["foam", "gas", "dry_chemical", "wet_chemical", "aerosol"];

fn pts_of(j: &Json) -> Option<Vec<Pt>> {
    let a = j.as_arr()?;
    let mut v = Vec::with_capacity(a.len());
    for p in a {
        v.push([p.idx(0)?.as_f64()?, p.idx(1)?.as_f64()?]);
    }
    Some(v)
}

/// Validación estructural y geométrica. No exige completitud de datos de ingeniería (eso lo hace
/// cada motor al calcular): aquí se detectan documentos corruptos y geometría imposible.
pub fn validate(p: &Json) -> IssueList {
    let mut out = IssueList::new();
    if p.field("schema").as_str() != Some(SCHEMA) {
        out.push(
            Issue::block("project.unsupported_schema")
                .module(MODULE)
                .path("schema")
                .param("schema", p.field("schema").as_str().unwrap_or("")),
        );
        return out;
    }
    for k in TOP_KEYS {
        if p.get(k).is_none() {
            out.push(Issue::block("project.missing_section").module(MODULE).path(k).param("section", *k));
        }
    }
    if out.has_block() {
        return out;
    }
    // --- meta
    if p.at("meta.name").and_then(Json::as_str).map(|s| s.trim().is_empty()).unwrap_or(true) {
        out.push(Issue::warning("project.name_empty").module(MODULE).path("meta.name"));
    }
    match p.at("meta.design_date") {
        Some(Json::Str(d)) => {
            if crate::timeutil::parse_date(d).is_none() {
                out.push(Issue::error("project.bad_design_date").module(MODULE).path("meta.design_date").param("value", d.as_str()));
            }
        }
        _ => out.push(Issue::warning("project.no_design_date").module(MODULE).path("meta.design_date")),
    }
    if !matches!(p.at("display.unit_system").and_then(Json::as_str), Some("SI") | Some("US")) {
        out.push(Issue::error("project.bad_unit_system").module(MODULE).path("display.unit_system"));
    }
    if let Some(d) = p.at("display.decimal_separator") {
        if !matches!(d.as_str(), Some(",") | Some(".")) {
            out.push(Issue::error("project.bad_decimal_separator").module(MODULE).path("display.decimal_separator"));
        }
    }
    if !matches!(p.at("display.language").and_then(Json::as_str), Some("es") | Some("en")) {
        out.push(Issue::error("project.bad_language").module(MODULE).path("display.language"));
    }
    if p.at("jurisdiction.profile_code").map(Json::is_null).unwrap_or(true) {
        out.push(Issue::warning("project.no_profile").module(MODULE).path("jurisdiction.profile_code"));
    }
    // --- niveles y entidades
    let levels = p.at("site.levels").map(Json::items).unwrap_or(&[]);
    if levels.is_empty() {
        out.push(Issue::block("project.no_levels").module(MODULE).path("site.levels"));
    }
    let mut level_ids: BTreeMap<String, usize> = BTreeMap::new();
    for (li, l) in levels.iter().enumerate() {
        let lp = format!("site.levels.{}", li);
        match l.field("id").as_str().filter(|s| !s.trim().is_empty()) {
            Some(id) => {
                if level_ids.insert(id.to_string(), li).is_some() {
                    out.push(Issue::block("geo.duplicate_id").module(MODULE).path(&format!("{}.id", lp)).object(id).param("id", id));
                }
            }
            None => out.push(Issue::block("data.missing").module(MODULE).path(&format!("{}.id", lp)).param("field", "id")),
        }
        if l.field("elevation_m").as_f64().is_none() {
            out.push(Issue::block("data.missing").module(MODULE).path(&format!("{}.elevation_m", lp)).param("field", "elevation_m"));
        }
        match l.field("height_m").as_f64() {
            Some(h) if h > 0.0 => {}
            Some(_) => {
                out.push(Issue::block("data.not_positive").module(MODULE).path(&format!("{}.height_m", lp)).param("field", "height_m"))
            }
            None => out.push(Issue::block("data.missing").module(MODULE).path(&format!("{}.height_m", lp)).param("field", "height_m")),
        }
        let mut ids: BTreeMap<String, ()> = BTreeMap::new();
        let mut rooms: Vec<(String, Vec<Pt>)> = Vec::new();
        for (ei, e) in l.field("entities").items().iter().enumerate() {
            let ep = format!("{}.entities.{}", lp, ei);
            let id = e.field("id").as_str().unwrap_or("");
            if id.is_empty() {
                out.push(Issue::block("data.missing").module(MODULE).path(&format!("{}.id", ep)).param("field", "id"));
            } else if ids.insert(id.to_string(), ()).is_some() {
                out.push(Issue::block("geo.duplicate_id").module(MODULE).path(&format!("{}.id", ep)).object(id).param("id", id));
            }
            let ty = e.field("type").as_str().unwrap_or("");
            match ty {
                "wall" | "room" | "door" | "dimension" | "line" | "polyline" => {
                    let Some(pts) = pts_of(e.field("pts")) else {
                        out.push(Issue::block("data.type").module(MODULE).path(&format!("{}.pts", ep)).object(id).param("field", "pts"));
                        continue;
                    };
                    if pts.iter().any(|q| q[0].abs() > COORD_LIMIT || q[1].abs() > COORD_LIMIT) {
                        out.push(Issue::error("geo.coordinate_out_of_range").module(MODULE).path(&format!("{}.pts", ep)).object(id));
                    }
                    match ty {
                        "room" => {
                            if pts.len() < 3 {
                                out.push(Issue::block("geo.polygon_too_few_points").module(MODULE).path(&format!("{}.pts", ep)).object(id));
                            } else if geometry::polygon_self_intersects(&pts) {
                                // el área neta de un polígono cruzado no tiene significado: se informa el cruce
                                out.push(
                                    Issue::error("geo.polygon_self_intersects").module(MODULE).path(&format!("{}.pts", ep)).object(id),
                                );
                            } else if geometry::area(&pts) < 1e-9 {
                                out.push(Issue::block("geo.polygon_zero_area").module(MODULE).path(&format!("{}.pts", ep)).object(id));
                            } else {
                                if geometry::has_degenerate_edges(&pts, 1e-3) {
                                    out.push(Issue::warning("geo.degenerate_edge").module(MODULE).path(&format!("{}.pts", ep)).object(id));
                                }
                                rooms.push((id.to_string(), pts));
                            }
                        }
                        "wall" => {
                            if pts.len() < 2 {
                                out.push(Issue::block("geo.polygon_too_few_points").module(MODULE).path(&format!("{}.pts", ep)).object(id));
                            }
                            match e.field("thickness_m").as_f64() {
                                Some(t) if t > 0.0 => {
                                    if t > 1.0 {
                                        out.push(
                                            Issue::warning("geo.wall_thickness_suspicious")
                                                .module(MODULE)
                                                .path(&format!("{}.thickness_m", ep))
                                                .object(id)
                                                .param("thickness_m", crate::json::fmt_num(t)),
                                        );
                                    }
                                }
                                Some(_) => out.push(
                                    Issue::block("data.not_positive")
                                        .module(MODULE)
                                        .path(&format!("{}.thickness_m", ep))
                                        .object(id)
                                        .param("field", "thickness_m"),
                                ),
                                None => out.push(
                                    Issue::block("data.missing")
                                        .module(MODULE)
                                        .path(&format!("{}.thickness_m", ep))
                                        .object(id)
                                        .param("field", "thickness_m"),
                                ),
                            }
                        }
                        "door" => {
                            if pts.len() != 2 || geometry::dist(pts[0], pts[1]) < 1e-3 {
                                out.push(Issue::block("geo.door_invalid").module(MODULE).path(&format!("{}.pts", ep)).object(id));
                            } else if geometry::dist(pts[0], pts[1]) > 10.0 {
                                out.push(
                                    Issue::warning("geo.door_suspicious_width").module(MODULE).path(&format!("{}.pts", ep)).object(id),
                                );
                            }
                        }
                        _ => {}
                    }
                }
                "" => out.push(Issue::block("data.missing").module(MODULE).path(&format!("{}.type", ep)).object(id).param("field", "type")),
                _ => {}
            }
        }
        // superposición de zonas del mismo nivel
        let mut reported = 0;
        for i in 0..rooms.len() {
            for j in (i + 1)..rooms.len() {
                if reported < 50 && geometry::polygons_overlap(&rooms[i].1, &rooms[j].1) {
                    out.push(
                        Issue::warning("geo.rooms_overlap")
                            .module(MODULE)
                            .path(&lp)
                            .object(&rooms[i].0)
                            .param("a", rooms[i].0.as_str())
                            .param("b", rooms[j].0.as_str()),
                    );
                    reported += 1;
                }
            }
        }
    }
    // --- sistemas
    let sys = p.field("systems");
    if sys.field("extinguishers").field("units").as_arr().is_none() {
        out.push(
            Issue::block("project.missing_section")
                .module(MODULE)
                .path("systems.extinguishers.units")
                .param("section", "systems.extinguishers.units"),
        );
    }
    if sys.field("water").field("networks").as_arr().is_none() {
        out.push(
            Issue::block("project.missing_section")
                .module(MODULE)
                .path("systems.water.networks")
                .param("section", "systems.water.networks"),
        );
    }
    for k in SYSTEM_ARRAYS {
        if sys.field(k).as_arr().is_none() {
            out.push(
                Issue::block("project.missing_section")
                    .module(MODULE)
                    .path(&format!("systems.{}", k))
                    .param("section", format!("systems.{}", k)),
            );
        }
    }
    let unique_ids = |out: &mut IssueList, base: &str, items: &[Json]| {
        let mut seen: BTreeMap<&str, ()> = BTreeMap::new();
        for (i, it) in items.iter().enumerate() {
            match it.field("id").as_str().filter(|s| !s.trim().is_empty()) {
                Some(id) => {
                    if seen.insert(id, ()).is_some() {
                        out.push(
                            Issue::block("geo.duplicate_id").module(MODULE).path(&format!("{}.{}.id", base, i)).object(id).param("id", id),
                        );
                    }
                }
                None => out.push(Issue::block("data.missing").module(MODULE).path(&format!("{}.{}.id", base, i)).param("field", "id")),
            }
        }
    };
    unique_ids(&mut out, "systems.extinguishers.units", sys.field("extinguishers").field("units").items());
    unique_ids(&mut out, "systems.water.networks", sys.field("water").field("networks").items());
    for k in SYSTEM_ARRAYS {
        unique_ids(&mut out, &format!("systems.{}", k), sys.field(k).items());
    }
    // --- supuestos sin confirmar (informativo aquí; bloquea la memoria de conformidad)
    for (i, a) in p.field("assumptions").items().iter().enumerate() {
        if a.field("confirmed").as_bool() != Some(true) {
            out.push(
                Issue::warning("project.assumption_unconfirmed")
                    .module(MODULE)
                    .path(&format!("assumptions.{}", i))
                    .object(a.field("id").as_str().unwrap_or("")),
            );
        }
    }
    out.sort();
    out
}

/// Huella de los **datos de entrada** del proyecto: excluye resultados, revisiones, excepciones
/// y marcas volátiles (`updated_at`, `revision`).
pub fn inputs_fingerprint(p: &Json) -> String {
    let mut c = p.clone();
    if let Json::Obj(m) = &mut c {
        m.remove("results");
        m.remove("reviews");
        m.remove("exceptions");
        m.remove("inspections");
        if let Some(Json::Obj(meta)) = m.get_mut("meta") {
            meta.remove("updated_at");
            meta.remove("revision");
            meta.remove("revisions");
            meta.remove("status");
        }
    }
    sha256_hex(c.canonical().as_bytes())
}

/// Migra un documento a la versión actual. Devuelve el documento y las notas de migración.
/// Una versión **más nueva** o desconocida se rechaza (no se pierde información en silencio).
pub fn migrate(p: Json) -> Result<(Json, Vec<String>), Issue> {
    let schema = p.field("schema").as_str().unwrap_or("").to_string();
    match schema.as_str() {
        SCHEMA => Ok((p, vec![])),
        "vigix.project/0" => {
            // Esquema 0 (formato interno previo): niveles y unidades en la raíz.
            let mut notes = Vec::new();
            let mut out = new_project(
                p.at("meta.id").and_then(Json::as_str).unwrap_or("MIGRADO"),
                p.at("meta.name").and_then(Json::as_str).unwrap_or(""),
                p.at("display.language").and_then(Json::as_str).unwrap_or("es"),
                p.at("display.unit_system").and_then(Json::as_str).unwrap_or("SI"),
                p.at("meta.created_at").and_then(Json::as_str).unwrap_or("1970-01-01T00:00:00Z"),
            );
            if let Some(levels) = p.get("levels") {
                out.set_at("site.levels", levels.clone());
                notes.push("levels -> site.levels".to_string());
            }
            if let Some(u) = p.get("extinguishers") {
                out.set_at("systems.extinguishers.units", u.clone());
                notes.push("extinguishers -> systems.extinguishers.units".to_string());
            }
            if let Some(j) = p.get("jurisdiction") {
                out.set_at("jurisdiction", j.clone());
            }
            notes.push("results descartados: deben recalcularse con el motor actual".to_string());
            Ok((out, notes))
        }
        s if s.starts_with("vigix.project/") => {
            let ver = s.rsplit('/').next().and_then(|v| v.parse::<u32>().ok()).unwrap_or(u32::MAX);
            if ver > 1 {
                Err(Issue::block("project.newer_schema").module(MODULE).path("schema").param("schema", s))
            } else {
                Err(Issue::block("project.unsupported_schema").module(MODULE).path("schema").param("schema", s))
            }
        }
        other => Err(Issue::block("project.unsupported_schema").module(MODULE).path("schema").param("schema", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Json {
        new_project("P1", "Proyecto de prueba", "es", "SI", "2026-10-28T00:00:00Z")
    }

    fn with_entities(ents: &str) -> Json {
        let mut p = base();
        p.set_at("site.levels.0.entities", Json::parse(ents).unwrap());
        p
    }

    fn codes(l: &IssueList) -> Vec<String> {
        l.0.iter().map(|i| i.code.clone()).collect()
    }

    #[test]
    fn la_plantilla_es_valida_y_sin_ceros_ocultos() {
        let p = base();
        let v = validate(&p);
        assert!(!v.has_block(), "{:?}", v);
        // advertencias esperadas: sin fecha de diseño ni perfil
        let c = codes(&v);
        assert!(c.contains(&"project.no_design_date".to_string()) && c.contains(&"project.no_profile".to_string()));
        // los datos de ingeniería desconocidos son null, jamás 0
        assert!(p.at("water_supply.static_pressure_bar").unwrap().is_null());
        assert!(p.at("site.height_m").unwrap().is_null());
        assert!(p.at("jurisdiction.profile_sha256").unwrap().is_null());
        // ida y vuelta por JSON canónico
        assert_eq!(Json::parse(&p.to_pretty()).unwrap(), p);
    }

    #[test]
    fn rechaza_documentos_sin_esquema_o_incompletos() {
        assert!(validate(&Json::obj()).has_block());
        let mut p = base();
        if let Json::Obj(m) = &mut p {
            m.remove("site");
            m.remove("systems");
        }
        let v = validate(&p);
        assert!(v.0.iter().any(|i| i.code == "project.missing_section" && i.path == "site"));
        assert!(v.has_block());
    }

    #[test]
    fn geometria_imposible_se_detecta() {
        let ents = r#"[
          {"id":"R1","type":"room","pts":[[0,0],[4,0],[4,3],[0,3]]},
          {"id":"R2","type":"room","pts":[[0,0],[2,0]]},
          {"id":"R3","type":"room","pts":[[0,0],[1,1],[2,2]]},
          {"id":"R4","type":"room","pts":[[0,0],[2,2],[2,0],[0,2]]},
          {"id":"R5","type":"room","pts":[[2,1],[6,1],[6,4],[2,4]]},
          {"id":"W1","type":"wall","pts":[[0,0],[5,0]],"thickness_m":0},
          {"id":"W2","type":"wall","pts":[[0,0],[5,0]]},
          {"id":"W3","type":"wall","pts":[[0,0],[5,0]],"thickness_m":2.5},
          {"id":"D1","type":"door","pts":[[1,1],[1,1]]},
          {"id":"R1","type":"room","pts":[[0,0],[1,0],[1,1],[0,1]]},
          {"id":"R6","type":"room","pts":[[0,0],[2e9,0],[2e9,3]]}
        ]"#;
        let v = validate(&with_entities(ents));
        let find = |code: &str, obj: &str| v.0.iter().any(|i| i.code == code && i.object.as_deref() == Some(obj));
        assert!(find("geo.polygon_too_few_points", "R2"));
        assert!(find("geo.polygon_zero_area", "R3"));
        assert!(find("geo.polygon_self_intersects", "R4"));
        assert!(find("geo.rooms_overlap", "R1")); // R1 con R5 (y R4)
        assert!(find("data.not_positive", "W1"));
        assert!(find("data.missing", "W2"));
        assert!(find("geo.wall_thickness_suspicious", "W3"));
        assert!(find("geo.door_invalid", "D1"));
        assert!(find("geo.duplicate_id", "R1"));
        assert!(find("geo.coordinate_out_of_range", "R6"));
        assert!(v.has_block());
        // la incidencia lleva la ruta exacta
        let i = v.0.iter().find(|i| i.code == "geo.polygon_zero_area").unwrap();
        assert_eq!(i.path, "site.levels.0.entities.2.pts");
    }

    #[test]
    fn zonas_contiguas_no_se_marcan_como_superpuestas() {
        let ents = r#"[
          {"id":"A","type":"room","pts":[[0,0],[5,0],[5,5],[0,5]]},
          {"id":"B","type":"room","pts":[[5,0],[10,0],[10,5],[5,5]]}]"#;
        let v = validate(&with_entities(ents));
        assert!(!codes(&v).contains(&"geo.rooms_overlap".to_string()), "{:?}", v);
    }

    #[test]
    fn niveles_y_sistemas_duplicados_o_sin_id() {
        let mut p = base();
        p.set_at("site.levels", Json::parse(r#"[{"id":"L1","elevation_m":0,"height_m":3,"entities":[]},{"id":"L1","elevation_m":3,"height_m":-1,"entities":[]},{"elevation_m":6,"height_m":3,"entities":[]}]"#).unwrap());
        p.set_at("systems.foam", Json::parse(r#"[{"id":"F1"},{"id":"F1"},{"nombre":"sin id"}]"#).unwrap());
        let v = validate(&p);
        assert!(v.0.iter().any(|i| i.code == "geo.duplicate_id" && i.path == "site.levels.1.id"));
        assert!(v.0.iter().any(|i| i.code == "data.not_positive" && i.path == "site.levels.1.height_m"));
        assert!(v.0.iter().any(|i| i.code == "data.missing" && i.path == "site.levels.2.id"));
        assert!(v.0.iter().any(|i| i.code == "geo.duplicate_id" && i.path == "systems.foam.1.id"));
        assert!(v.0.iter().any(|i| i.code == "data.missing" && i.path == "systems.foam.2.id"));
    }

    #[test]
    fn fecha_idioma_y_unidades_invalidos() {
        let mut p = base();
        p.set_at("meta.design_date", Json::str("2026-13-40"));
        p.set_at("display.language", Json::str("fr"));
        p.set_at("display.unit_system", Json::str("imperial"));
        let c = codes(&validate(&p));
        for k in ["project.bad_design_date", "project.bad_language", "project.bad_unit_system"] {
            assert!(c.contains(&k.to_string()), "{}", k);
        }
        p.set_at("meta.design_date", Json::str("2026-10-28"));
        assert!(!codes(&validate(&p)).contains(&"project.no_design_date".to_string()));
    }

    #[test]
    fn supuestos_sin_confirmar_se_avisan() {
        let mut p = base();
        p.set_at(
            "assumptions",
            Json::parse(r#"[{"id":"S1","text":"x","confirmed":false},{"id":"S2","text":"y","confirmed":true}]"#).unwrap(),
        );
        let v = validate(&p);
        assert_eq!(v.0.iter().filter(|i| i.code == "project.assumption_unconfirmed").count(), 1);
    }

    #[test]
    fn huella_de_entradas_ignora_resultados_y_marcas_volatiles() {
        let mut p = base();
        let h = inputs_fingerprint(&p);
        p.set_at("results.hydraulic", Json::obj().with("x", Json::num(1.0)));
        p.set_at("meta.updated_at", Json::str("2027-01-01T00:00:00Z"));
        p.set_at("meta.revision", Json::num(7.0));
        p.set_at("reviews", Json::Arr(vec![Json::obj().with("id", Json::str("RV1"))]));
        assert_eq!(inputs_fingerprint(&p), h);
        p.set_at("site.height_m", Json::num(12.0));
        assert_ne!(inputs_fingerprint(&p), h);
    }

    #[test]
    fn migracion_conserva_datos_y_rechaza_versiones_futuras() {
        let (p, n) = migrate(base()).unwrap();
        assert!(n.is_empty());
        assert_eq!(p.field("schema").as_str(), Some(SCHEMA));
        let v0 = Json::parse(
            r#"{"schema":"vigix.project/0","meta":{"id":"OLD","name":"Antiguo"},"levels":[{"id":"N1","elevation_m":0,"height_m":3,"entities":[]}],
                "extinguishers":[{"id":"E1","level":"N1","x":1,"y":1,"agent":"water","ratings":{"A":1}}],"results":{"x":1}}"#,
        )
        .unwrap();
        let (m, notes) = migrate(v0).unwrap();
        assert_eq!(m.field("schema").as_str(), Some(SCHEMA));
        assert_eq!(m.at("site.levels.0.id").and_then(Json::as_str), Some("N1"));
        assert_eq!(m.at("systems.extinguishers.units.0.id").and_then(Json::as_str), Some("E1"));
        assert!(m.field("results").as_obj().unwrap().is_empty(), "los resultados antiguos no se arrastran");
        assert!(notes.iter().any(|n| n.contains("results descartados")));
        assert!(!validate(&m).has_block(), "{:?}", validate(&m));
        // versión más nueva => error explícito, sin tocar el documento
        let e = migrate(Json::obj().with("schema", Json::str("vigix.project/2"))).unwrap_err();
        assert_eq!(e.code, "project.newer_schema");
        assert_eq!(migrate(Json::obj()).unwrap_err().code, "project.unsupported_schema");
        assert_eq!(migrate(Json::obj().with("schema", Json::str("otro/1"))).unwrap_err().code, "project.unsupported_schema");
    }
}
