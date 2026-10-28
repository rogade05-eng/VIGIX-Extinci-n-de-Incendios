//! Compuerta de conformidad y estado del documento (contrato 3.5 y 4.4).
//!
//! El software **no declara conformidad**: solo decide si un documento puede salir como
//! *borrador*, *para revisión del especialista* o *paquete de presentación sujeto a firma y
//! aprobación externa*. Cualquier bloqueo fuerza la leyenda "BORRADOR / NO APTO PARA
//! CONSTRUCCIÓN" en cada página y exportación. Los errores (no bloqueos) solo pueden dispensarse
//! con una excepción aprobada por un revisor, y la excepción queda registrada y visible.

use crate::agents;
use crate::cause_effect;
use crate::extinguishers;
use crate::hydraulics::model as hyd;
use crate::issues::{Issue, IssueList, Severity};
use crate::json::Json;
use crate::profile::{Level, Profile, Registry};
use crate::project;
use crate::timeutil::parse_date;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocStatus {
    /// Hay bloqueos: "BORRADOR / NO APTO PARA CONSTRUCCIÓN".
    Draft,
    /// Sin bloqueos pero sin aprobación vigente del revisor especialista.
    ForReview,
    /// Aprobación del revisor sobre los datos actuales: sigue sujeto a firma y aprobación externa.
    PermitPackage,
}

impl DocStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            DocStatus::Draft => "draft",
            DocStatus::ForReview => "for_review",
            DocStatus::PermitPackage => "permit_package",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Gate {
    pub status: DocStatus,
    pub issues: IssueList,
}

impl Gate {
    pub fn to_json(&self) -> Json {
        Json::obj()
            .with("status", Json::str(self.status.as_str()))
            .with("issues", self.issues.to_json())
            .with("summary", self.issues.summary_json())
    }
}

/// Resultado requerido: (clase, clave dentro de `results.<clase>`; `None` = singleton).
#[derive(Clone, Debug, PartialEq)]
pub struct Required {
    pub kind: &'static str,
    pub key: Option<String>,
    pub system: &'static str,
}

/// Qué cálculos debe tener el proyecto para poder emitir un paquete.
pub fn required_results(p: &Json) -> Vec<Required> {
    let mut v = Vec::new();
    let sys = p.field("systems");
    if !sys.field("extinguishers").field("units").items().is_empty() {
        v.push(Required { kind: "extinguishers", key: None, system: "portable_extinguishers" });
    }
    for net in sys.field("water").field("networks").items() {
        let nid = net.field("id").as_str().unwrap_or("");
        let st: &'static str = match net.field("system_type").as_str().unwrap_or("") {
            "wet_pipe_sprinkler" => "wet_pipe_sprinkler",
            "dry_pipe_sprinkler" => "dry_pipe_sprinkler",
            "preaction_sprinkler" => "preaction_sprinkler",
            "deluge_sprinkler" => "deluge_sprinkler",
            "water_spray" => "water_spray",
            "water_mist" => "water_mist",
            "standpipe_hose" => "standpipe_hose",
            "private_hydrant" => "private_hydrant",
            "fire_pump" => "fire_pump",
            "water_tank" => "water_tank",
            _ => "wet_pipe_sprinkler",
        };
        for sc in net.field("scenarios").items() {
            let sid = sc.field("id").as_str().unwrap_or("");
            v.push(Required { kind: "hydraulic", key: Some(format!("{}:{}", nid, sid)), system: st });
        }
    }
    for (kind, system) in
        [("foam", "foam"), ("dry_chemical", "dry_chemical"), ("wet_chemical", "wet_chemical_kitchen"), ("aerosol", "condensed_aerosol")]
    {
        for s in sys.field(kind).items() {
            v.push(Required {
                kind: if kind == "wet_chemical" { "wet_chemical" } else { kind },
                key: Some(s.field("id").as_str().unwrap_or("").to_string()),
                system,
            });
        }
    }
    for s in sys.field("gas").items() {
        let system = match s.field("family").as_str() {
            Some("inert") => "gas_inert",
            Some("co2") => "gas_co2",
            _ => "gas_clean_agent",
        };
        v.push(Required { kind: "gas", key: Some(s.field("id").as_str().unwrap_or("").to_string()), system });
    }
    if !p.at("cause_effect.rules").map(Json::items).unwrap_or(&[]).is_empty() || !sys.field("gas").items().is_empty() {
        v.push(Required { kind: "cause_effect", key: None, system: "cause_effect" });
    }
    v
}

fn result_envelope<'a>(p: &'a Json, r: &Required) -> Option<&'a Json> {
    let node = p.field("results").field(r.kind);
    match &r.key {
        None => node.as_obj().map(|_| node),
        Some(k) => node.get(k),
    }
}

/// Huella vigente de un resultado requerido (para detectar obsolescencia).
pub fn current_fingerprint(p: &Json, r: &Required, profile: &Profile, env: &Json) -> Option<String> {
    match r.kind {
        "extinguishers" => Some(extinguishers::input_fingerprint(p, profile, None)),
        "cause_effect" => Some(cause_effect::input_fingerprint(p, Some(profile))),
        "hydraulic" => {
            let key = r.key.as_deref()?;
            let (nid, sid) = key.split_once(':')?;
            let net = p.at("systems.water.networks")?.items().iter().find(|n| n.field("id").as_str() == Some(nid))?;
            let mode = match env.at("args.mode").and_then(Json::as_str) {
                Some("required_pressure") => hyd::Mode::RequiredPressure,
                _ => hyd::Mode::OperatingPoint,
            };
            Some(hyd::input_fingerprint(net, sid, &mode, profile))
        }
        kind => {
            let key = r.key.as_deref()?;
            let list = match kind {
                "foam" | "gas" | "dry_chemical" | "wet_chemical" | "aerosol" => p.at(&format!("systems.{}", kind))?.items(),
                _ => return None,
            };
            let sys = list.iter().find(|s| s.field("id").as_str() == Some(key))?;
            Some(agents::input_fingerprint(kind, sys, profile))
        }
    }
}

fn waived(p: &Json, code: &str, object: Option<&str>) -> bool {
    p.field("exceptions").items().iter().any(|e| {
        e.field("code").as_str() == Some(code)
            && e.field("object").as_str() == object
            && e.field("approved_by").as_str().map(|s| !s.trim().is_empty()).unwrap_or(false)
            && matches!(e.field("approved_role").as_str(), Some("reviewer") | Some("admin"))
            && e.field("reason").as_str().map(|s| !s.trim().is_empty()).unwrap_or(false)
    })
}

pub fn evaluate(p: &Json, registry: &Registry) -> Gate {
    let mut out = IssueList::new();
    let blk = |code: &str| Issue::block(code).module("gate");

    // 1) perfil
    let code = p.at("jurisdiction.profile_code").and_then(Json::as_str);
    let ver = p.at("jurisdiction.profile_version").and_then(Json::as_str);
    let profile: Option<&Profile> = match (code, ver) {
        (Some(c), Some(v)) => match registry.get(c, v) {
            Some(pr) => Some(pr),
            None => {
                out.push(blk("gate.profile_missing").path("jurisdiction").param("code", c).param("version", v));
                None
            }
        },
        _ => {
            out.push(blk("gate.no_profile").path("jurisdiction.profile_code"));
            None
        }
    };
    let design_days = p.at("meta.design_date").and_then(Json::as_str).and_then(parse_date);
    if let Some(pr) = profile {
        if let Some(h) = p.at("jurisdiction.profile_sha256").and_then(Json::as_str) {
            if h != pr.sha256 {
                out.push(
                    blk("gate.profile_hash_changed")
                        .path("jurisdiction.profile_sha256")
                        .param("project", h)
                        .param("registry", pr.sha256.as_str()),
                );
            }
        }
        if pr.is_demo() {
            out.push(blk("gate.profile_demo").path("jurisdiction.profile_code").param("code", pr.code()));
        }
        if pr.level != Level::Verified {
            out.push(
                blk("gate.profile_not_verified").path("jurisdiction.profile_code").param("code", pr.code()).param("version", pr.version()),
            );
        }
        match design_days {
            None => out.push(blk("gate.no_design_date").path("meta.design_date")),
            Some(d) => {
                if let Some(i) = pr.validity_issue(d) {
                    out.push(i);
                }
            }
        }
    }

    // 2) estructura y geometría del proyecto
    for i in project::validate(p).0 {
        if i.severity == Severity::Block {
            out.push(i);
        }
    }

    // 3) supuestos sin confirmar
    for (i, a) in p.field("assumptions").items().iter().enumerate() {
        if a.field("confirmed").as_bool() != Some(true) {
            out.push(blk("gate.assumption_unconfirmed").path(&format!("assumptions.{}", i)).opt_object(a.field("id").as_str()));
        }
    }

    // 4) cobertura del perfil y resultados vigentes de cada sistema
    for r in required_results(p) {
        if let Some(pr) = profile {
            if !pr.covers(r.system) {
                out.push(blk("gate.profile_not_covering").path("scope.systems").param("system", r.system));
            }
        }
        let label = match &r.key {
            Some(k) => format!("{}:{}", r.kind, k),
            None => r.kind.to_string(),
        };
        let Some(env) = result_envelope(p, &r) else {
            out.push(blk("gate.result_missing").param("result", label.as_str()));
            continue;
        };
        if env.field("status").as_str() != Some("ok") {
            out.push(
                blk("gate.result_blocked").param("result", label.as_str()).param("status", env.field("status").as_str().unwrap_or("?")),
            );
            continue;
        }
        if let Some(pr) = profile {
            match current_fingerprint(p, &r, pr, env) {
                Some(fp) if env.field("input_fingerprint").as_str() == Some(fp.as_str()) => {}
                _ => {
                    out.push(blk("gate.result_stale").param("result", label.as_str()));
                    continue;
                }
            }
        }
        for iss in env.field("issues").items().iter().filter_map(Issue::from_json) {
            match iss.severity {
                Severity::Block => out.push(
                    blk("gate.result_has_block")
                        .param("result", label.as_str())
                        .param("code", iss.code.as_str())
                        .opt_object(iss.object.as_deref()),
                ),
                Severity::Error => {
                    if waived(p, &iss.code, iss.object.as_deref()) {
                        out.push(
                            Issue::info("gate.exception_applied")
                                .module("gate")
                                .param("result", label.as_str())
                                .param("code", iss.code.as_str())
                                .opt_object(iss.object.as_deref()),
                        );
                    } else {
                        out.push(
                            blk("gate.unresolved_error")
                                .param("result", label.as_str())
                                .param("code", iss.code.as_str())
                                .opt_object(iss.object.as_deref())
                                .path(&iss.path),
                        );
                    }
                }
                _ => {}
            }
        }
    }

    // 5) estado del documento
    out.sort();
    let status = if out.has_block() {
        DocStatus::Draft
    } else {
        let fp = project::inputs_fingerprint(p);
        let psha = profile.map(|x| x.sha256.as_str()).unwrap_or("");
        let approvals: Vec<&Json> = p
            .field("reviews")
            .items()
            .iter()
            .filter(|r| {
                r.field("decision").as_str() == Some("approve") && matches!(r.field("role").as_str(), Some("reviewer") | Some("admin"))
            })
            .collect();
        let current = approvals
            .iter()
            .any(|r| r.field("inputs_fingerprint").as_str() == Some(fp.as_str()) && r.field("profile_sha256").as_str() == Some(psha));
        if !approvals.is_empty() && !current {
            out.push(Issue::info("gate.review_stale").module("gate").path("reviews"));
            out.sort();
        }
        if current {
            DocStatus::PermitPackage
        } else {
            DocStatus::ForReview
        }
    };
    Gate { status, issues: out }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::testutil::{demo_json, set_method};
    use crate::profile::seal;

    fn verified_registry() -> (Registry, Json) {
        let mut j = demo_json();
        j.set_at("demo", Json::Bool(false));
        j.set_at("profile_code", Json::str("VERIF-TEST"));
        j.set_at("authorship.approval_status", Json::str("aprobado"));
        j.set_at("authorship.approval_ref", Json::str("ACTA-TEST-1"));
        j.set_at("authorship.reviewer", Json::str("Especialista de prueba"));
        // casos de prueba por sistema cubierto (contenido mínimo; la ejecución real se prueba en selftest)
        let systems: Vec<String> = j.at("scope.systems").unwrap().items().iter().map(|s| s.as_str().unwrap().to_string()).collect();
        j.set_at(
            "test_cases",
            Json::Arr(
                systems
                    .iter()
                    .map(|s| {
                        Json::obj()
                            .with("id", Json::str(format!("TC-{}", s)))
                            .with("system", Json::str(s.as_str()))
                            .with("input", Json::obj())
                            .with("expected", Json::arr())
                    })
                    .collect(),
            ),
        );
        seal(&mut j);
        let mut reg = Registry::new();
        reg.add(j.clone()).unwrap();
        (reg, j)
    }

    fn project_with_profile(code: &str, ver: &str, sha: Option<&str>) -> Json {
        let mut p = project::new_project("P1", "Prueba", "es", "SI", "2026-10-28T00:00:00Z");
        p.set_at("jurisdiction.profile_code", Json::str(code));
        p.set_at("jurisdiction.profile_version", Json::str(ver));
        if let Some(s) = sha {
            p.set_at("jurisdiction.profile_sha256", Json::str(s));
        }
        p.set_at("meta.design_date", Json::str("2026-10-28"));
        p
    }

    fn codes(g: &Gate) -> Vec<String> {
        g.issues.0.iter().map(|i| i.code.clone()).collect()
    }

    #[test]
    fn sin_perfil_o_perfil_inexistente_es_borrador() {
        let reg = Registry::new();
        let p = project::new_project("P1", "x", "es", "SI", "2026-10-28T00:00:00Z");
        let g = evaluate(&p, &reg);
        assert_eq!(g.status, DocStatus::Draft);
        assert!(codes(&g).contains(&"gate.no_profile".to_string()));
        let g = evaluate(&project_with_profile("X", "1", None), &reg);
        assert!(codes(&g).contains(&"gate.profile_missing".to_string()));
    }

    #[test]
    fn el_perfil_de_demostracion_nunca_permite_paquete() {
        let mut reg = Registry::new();
        reg.add(demo_json()).unwrap();
        let p = project_with_profile("DEMO-NO-NORMATIVO", "0.1.0", None);
        let g = evaluate(&p, &reg);
        assert_eq!(g.status, DocStatus::Draft);
        let c = codes(&g);
        assert!(c.contains(&"gate.profile_demo".to_string()));
        assert!(c.contains(&"gate.profile_not_verified".to_string()));
    }

    #[test]
    fn proyecto_vacio_con_perfil_verificado_pasa_a_revision_y_luego_a_paquete() {
        let (reg, _) = verified_registry();
        let psha = reg.get("VERIF-TEST", "0.1.0").unwrap().sha256.clone();
        let mut p = project_with_profile("VERIF-TEST", "0.1.0", Some(&psha));
        let g = evaluate(&p, &reg);
        assert_eq!(g.status, DocStatus::ForReview, "{:?}", g.issues);
        // aprobación del revisor ligada a los datos actuales y al perfil
        let fp = project::inputs_fingerprint(&p);
        p.set_at(
            "reviews",
            Json::Arr(vec![Json::obj()
                .with("id", Json::str("RV1"))
                .with("decision", Json::str("approve"))
                .with("role", Json::str("reviewer"))
                .with("inputs_fingerprint", Json::str(fp.as_str()))
                .with("profile_sha256", Json::str(psha.as_str()))]),
        );
        assert_eq!(evaluate(&p, &reg).status, DocStatus::PermitPackage);
        // cualquier cambio posterior invalida la aprobación
        p.set_at("site.height_m", Json::num(12.0));
        let g = evaluate(&p, &reg);
        assert_eq!(g.status, DocStatus::ForReview);
        assert!(codes(&g).contains(&"gate.review_stale".to_string()));
        // una aprobación de un proyectista no cuenta
        let mut q = project_with_profile("VERIF-TEST", "0.1.0", Some(&psha));
        let fp = project::inputs_fingerprint(&q);
        q.set_at(
            "reviews",
            Json::Arr(vec![Json::obj()
                .with("decision", Json::str("approve"))
                .with("role", Json::str("designer"))
                .with("inputs_fingerprint", Json::str(fp.as_str()))
                .with("profile_sha256", Json::str(psha.as_str()))]),
        );
        assert_eq!(evaluate(&q, &reg).status, DocStatus::ForReview);
    }

    #[test]
    fn falta_fecha_de_diseno_vigencia_y_huella_alterada_bloquean() {
        let (reg, _) = verified_registry();
        let psha = reg.get("VERIF-TEST", "0.1.0").unwrap().sha256.clone();
        let mut p = project_with_profile("VERIF-TEST", "0.1.0", Some(&psha));
        p.set_at("meta.design_date", Json::Null);
        assert!(codes(&evaluate(&p, &reg)).contains(&"gate.no_design_date".to_string()));
        p.set_at("meta.design_date", Json::str("2020-01-01")); // antes de la vigencia (2026-10-28)
        assert!(codes(&evaluate(&p, &reg)).contains(&"profile.not_yet_effective".to_string()));
        let p = project_with_profile("VERIF-TEST", "0.1.0", Some("0000"));
        assert!(codes(&evaluate(&p, &reg)).contains(&"gate.profile_hash_changed".to_string()));
    }

    fn extinguisher_project(reg: &Registry) -> Json {
        let psha = reg.get("VERIF-TEST", "0.1.0").unwrap().sha256.clone();
        let mut p = project_with_profile("VERIF-TEST", "0.1.0", Some(&psha));
        p.set_at(
            "site.levels.0.entities",
            Json::parse(r#"[{"id":"R1","type":"room","pts":[[0,0],[6,0],[6,6],[0,6]],"hazard_class":"demo_low","fire_classes":["A"]}]"#)
                .unwrap(),
        );
        p.set_at(
            "systems.extinguishers.units",
            Json::parse(r#"[{"id":"E1","level":"L1","x":3,"y":3,"agent":"water","ratings":{"A":5}}]"#).unwrap(),
        );
        p
    }

    #[test]
    fn resultado_ausente_obsoleto_o_con_errores_bloquea_y_las_excepciones_solo_dispensan_errores() {
        let (reg, _) = verified_registry();
        let prof = reg.get("VERIF-TEST", "0.1.0").unwrap();
        let mut p = extinguisher_project(&reg);
        // 1) sin resultado
        assert!(evaluate(&p, &reg)
            .issues
            .0
            .iter()
            .any(|i| i.code == "gate.result_missing" && i.params.get("result").map(String::as_str) == Some("extinguishers")));
        // 2) con resultado vigente y sin errores
        let out = extinguishers::run(&p, prof, None);
        assert_eq!(out.status, crate::calc::Status::Ok, "{:?}", out.issues);
        let env = out.to_json(Some(prof), "2026-10-28T00:00:00Z");
        p.set_at("results.extinguishers", env.clone());
        let g = evaluate(&p, &reg);
        assert!(
            !codes(&g).contains(&"gate.result_missing".to_string()) && !codes(&g).contains(&"gate.result_stale".to_string()),
            "{:?}",
            g.issues
        );
        // 3) el resultado trae errores (área insuficiente por el criterio de 100 m2: sala de 36 m2 => 1 unidad, bien).
        //    Forzamos un error: agregar una zona sin unidades de clase C compatible
        let mut p2 = p.clone();
        p2.set_at(
            "site.levels.0.entities",
            Json::parse(
                r#"[{"id":"R1","type":"room","pts":[[0,0],[6,0],[6,6],[0,6]],"hazard_class":"demo_low","fire_classes":["A","C"]}]"#,
            )
            .unwrap(),
        );
        let out2 = extinguishers::run(&p2, prof, None);
        p2.set_at("results.extinguishers", out2.to_json(Some(prof), "2026-10-28T00:00:00Z"));
        let g2 = evaluate(&p2, &reg);
        assert_eq!(g2.status, DocStatus::Draft);
        let unresolved: Vec<String> =
            g2.issues.0.iter().filter(|i| i.code == "gate.unresolved_error").filter_map(|i| i.params.get("code").cloned()).collect();
        assert!(
            unresolved.contains(&"ext.no_serving_unit".to_string()) && unresolved.contains(&"ext.insufficient_by_area".to_string()),
            "{:?}",
            unresolved
        );
        // 4) una excepción aprobada por revisor dispensa SOLO su error; una sin aprobación no dispensa nada
        let mk = |code: &str, approved: bool| {
            Json::obj()
                .with("id", Json::str(format!("EX-{}", code)))
                .with("code", Json::str(code))
                .with("object", Json::str("R1"))
                .with("reason", Json::str("Zona sin riesgo C real (verificado en sitio)"))
                .with("approved_by", if approved { Json::str("Ing. Revisor") } else { Json::Null })
                .with("approved_role", if approved { Json::str("reviewer") } else { Json::Null })
        };
        p2.set_at("exceptions", Json::Arr(vec![mk("ext.no_serving_unit", false), mk("ext.insufficient_by_area", false)]));
        assert_eq!(evaluate(&p2, &reg).issues.0.iter().filter(|i| i.code == "gate.unresolved_error").count(), 2);
        p2.set_at("exceptions", Json::Arr(vec![mk("ext.no_serving_unit", true), mk("ext.insufficient_by_area", false)]));
        let g3 = evaluate(&p2, &reg);
        assert_eq!(g3.issues.0.iter().filter(|i| i.code == "gate.unresolved_error").count(), 1, "{:?}", g3.issues);
        assert!(codes(&g3).contains(&"gate.exception_applied".to_string()));
        p2.set_at("exceptions", Json::Arr(vec![mk("ext.no_serving_unit", true), mk("ext.insufficient_by_area", true)]));
        let g4 = evaluate(&p2, &reg);
        assert!(!codes(&g4).contains(&"gate.unresolved_error".to_string()), "{:?}", g4.issues);
        assert_eq!(g4.status, DocStatus::ForReview, "con todos los errores dispensados queda para revisión: {:?}", g4.issues);
        // 5) cambiar un dato deja el resultado obsoleto
        let mut p3 = p.clone();
        p3.set_at("systems.extinguishers.units.0.x", Json::num(2.0));
        assert!(codes(&evaluate(&p3, &reg)).contains(&"gate.result_stale".to_string()));
    }

    #[test]
    fn un_bloqueo_dentro_del_resultado_no_se_puede_dispensar_con_excepcion() {
        let (reg, _) = verified_registry();
        let prof = reg.get("VERIF-TEST", "0.1.0").unwrap();
        let mut p = extinguisher_project(&reg);
        let out = extinguishers::run(&p, prof, None);
        let mut env = out.to_json(Some(prof), "2026-10-28T00:00:00Z");
        // inyectamos un bloqueo en el resultado almacenado
        let mut issues = env.field("issues").items().to_vec();
        issues.push(Issue::block("ext.grid_too_large").module("extinguishers").object("L1").to_json());
        env.set("issues", Json::Arr(issues));
        p.set_at("results.extinguishers", env);
        p.set_at(
            "exceptions",
            Json::Arr(vec![Json::obj()
                .with("code", Json::str("ext.grid_too_large"))
                .with("object", Json::str("L1"))
                .with("reason", Json::str("x"))
                .with("approved_by", Json::str("R"))
                .with("approved_role", Json::str("reviewer"))]),
        );
        let g = evaluate(&p, &reg);
        assert_eq!(g.status, DocStatus::Draft);
        assert!(g.issues.0.iter().any(|i| i.code == "gate.result_has_block"));
    }

    #[test]
    fn supuestos_sin_confirmar_y_perfil_que_no_cubre_bloquean() {
        let (reg, _) = verified_registry();
        let psha = reg.get("VERIF-TEST", "0.1.0").unwrap().sha256.clone();
        let mut p = project_with_profile("VERIF-TEST", "0.1.0", Some(&psha));
        p.set_at("assumptions", Json::parse(r#"[{"id":"S1","text":"x","confirmed":false}]"#).unwrap());
        assert!(codes(&evaluate(&p, &reg)).contains(&"gate.assumption_unconfirmed".to_string()));
        // sistema que el perfil no cubre: red de rociadores de diluvio
        let mut p = project_with_profile("VERIF-TEST", "0.1.0", Some(&psha));
        p.set_at(
            "systems.water.networks",
            Json::parse(
                r#"[{"id":"N1","system_type":"deluge_sprinkler","nodes":[],"links":[],"scenarios":[{"id":"S1","active_devices":"all"}]}]"#,
            )
            .unwrap(),
        );
        let g = evaluate(&p, &reg);
        assert!(g
            .issues
            .0
            .iter()
            .any(|i| i.code == "gate.profile_not_covering" && i.params.get("system").map(String::as_str) == Some("deluge_sprinkler")));
        let _ = set_method;
    }

    #[test]
    fn requisitos_de_resultados_segun_los_sistemas_presentes() {
        let mut p = project::new_project("P1", "x", "es", "SI", "2026-10-28T00:00:00Z");
        assert!(required_results(&p).is_empty());
        p.set_at("systems.gas", Json::parse(r#"[{"id":"G1","family":"co2"}]"#).unwrap());
        p.set_at("systems.foam", Json::parse(r#"[{"id":"F1"}]"#).unwrap());
        let r = required_results(&p);
        assert!(r.iter().any(|x| x.kind == "gas" && x.system == "gas_co2"));
        assert!(r.iter().any(|x| x.kind == "foam"));
        assert!(r.iter().any(|x| x.kind == "cause_effect"), "con gas se exige la matriz causa-efecto");
    }
}
