//! Pruebas del módulo hidráulico guiado por perfil (usa el perfil de DEMOSTRACIÓN).
//! Las referencias son analíticas e independientes del solver (bisección escalar).

use std::fs;
use std::path::PathBuf;
use vigix_core::calc::Status;
use vigix_core::hydraulics::model::{run, Mode};
use vigix_core::issues::Severity;
use vigix_core::json::Json;
use vigix_core::profile::Profile;

fn demo_json() -> Json {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../profiles/demo/DEMO-NO-NORMATIVO_0.1.0.json");
    Json::parse(&fs::read_to_string(p).unwrap()).unwrap()
}

fn demo() -> Profile {
    Profile::from_json(demo_json()).unwrap()
}

const GAMMA: f64 = 1000.0 * 9.80665 / 1.0e5;

fn r_hw(len: f64, d: f64, c: f64) -> f64 {
    605000.0 * len / (c.powf(1.85) * d.powf(4.87))
}

/// Suministro -> P1 -> J -> (P2 -> S1, P3 -> S2). Todo a z=0 salvo los rociadores (z_dev).
fn net(z_dev: f64, extra_links: &str, extra_nodes: &str) -> Json {
    let s = format!(
        r#"{{
      "id":"NET1","name":"Red de prueba","system_type":"wet_pipe_sprinkler",
      "nodes":[
        {{"id":"SUP","kind":"supply","elevation_m":0,"static_pressure_bar":6.0,"residual_pressure_bar":4.0,"test_flow_lpm":1000}},
        {{"id":"J","kind":"junction","elevation_m":0}},
        {{"id":"S1","kind":"sprinkler","elevation_m":{z},"k_lpm_bar05":80,"coverage_area_m2":12,"min_pressure_bar":0.5}},
        {{"id":"S2","kind":"sprinkler","elevation_m":{z},"k_lpm_bar05":80,"coverage_area_m2":12,"min_pressure_bar":0.5}}
        {en}
      ],
      "links":[
        {{"id":"P1","kind":"pipe","from":"SUP","to":"J","length_m":30,"diameter_mm":65,"c_factor":120,"fittings":[]}},
        {{"id":"P2","kind":"pipe","from":"J","to":"S1","length_m":10,"diameter_mm":40,"c_factor":120,"fittings":[]}},
        {{"id":"P3","kind":"pipe","from":"J","to":"S2","length_m":10,"diameter_mm":40,"c_factor":120,"fittings":[]}}
        {el}
      ],
      "scenarios":[
        {{"id":"SC1","name":"Ambos","active_devices":["S1","S2"]}},
        {{"id":"SCD","name":"Con clase","active_devices":["S1","S2"],"hazard_class":"demo_low"}}
      ]
    }}"#,
        z = z_dev,
        en = extra_nodes,
        el = extra_links
    );
    Json::parse(&s).unwrap()
}

/// Referencia independiente: ecuación escalar en el caudal total Q (simetría entre S1 y S2).
fn reference_total_flow(z_dev: f64) -> f64 {
    let (r1, r2) = (r_hw(30.0, 65.0, 120.0), r_hw(10.0, 40.0, 120.0));
    let f = |q: f64| {
        let p_sup = 6.0 - (6.0 - 4.0) * (q / 1000.0).powf(1.85);
        let p_dev = p_sup - r1 * q.powf(1.85) - r2 * (q / 2.0).powf(1.85) - GAMMA * z_dev;
        let q_dev = if p_dev > 0.0 { 80.0 * p_dev.sqrt() } else { 0.0 };
        2.0 * q_dev - q
    };
    let (mut lo, mut hi) = (0.0, 5000.0);
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if f(mid) > 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

fn val(j: &Json, path: &str) -> f64 {
    j.at(path).and_then(Json::as_f64).unwrap_or_else(|| panic!("falta {}", path))
}

fn node<'a>(values: &'a Json, id: &str) -> &'a Json {
    values.field("nodes").items().iter().find(|n| n.field("id").as_str() == Some(id)).unwrap()
}
fn link<'a>(values: &'a Json, id: &str) -> &'a Json {
    values.field("links").items().iter().find(|n| n.field("id").as_str() == Some(id)).unwrap()
}

#[test]
fn punto_de_operacion_coincide_con_la_referencia_analitica() {
    for z in [0.0, 3.0, 8.0] {
        let out = run(&net(z, "", ""), "SC1", Mode::OperatingPoint, &demo(), None);
        assert_eq!(out.status, Status::Ok, "{:?}", out.issues);
        let qref = reference_total_flow(z);
        let q = val(&out.values, "summary.total_demand_lpm");
        assert!((q - qref).abs() < 1e-6 * qref, "z={} q={} ref={}", z, q, qref);
        // el flujo por el suministro es el total y la presión en la conexión sigue la curva
        let sup = &out.values.field("supplies").items()[0];
        assert!((val(sup, "flow_lpm") - qref).abs() < 1e-6 * qref);
        let p_conn = val(sup, "pressure_at_connection_bar");
        let p_curve = 6.0 - 2.0 * (qref / 1000.0).powf(1.85);
        assert!((p_conn - p_curve).abs() < 1e-7, "{} vs {}", p_conn, p_curve);
        assert!(val(&out.values, "convergence.residual_relative") < 1e-6);
        assert_eq!(out.values.at("convergence.converged").and_then(Json::as_bool), Some(true));
        // simetría
        assert!((val(node(&out.values, "S1"), "outflow_lpm") - val(node(&out.values, "S2"), "outflow_lpm")).abs() < 1e-7);
    }
}

#[test]
fn resultado_trae_trazabilidad_completa() {
    let out = run(&net(3.0, "", ""), "SC1", Mode::OperatingPoint, &demo(), None);
    assert_eq!(out.fingerprint.len(), 64);
    assert!(out.params_used.iter().all(|p| !p.reference.is_empty() && !p.unit.is_empty()));
    assert!(out.params_used.iter().any(|p| p.path == "parameters.hydraulics.friction.coefficient"));
    assert!(out.trace.iter().any(|t| t.field("step").as_str() == Some("friction")));
    let j = out.to_json(Some(&demo()), "2026-10-28T00:00:00Z");
    assert_eq!(j.at("profile.verified").and_then(Json::as_bool), Some(false));
    assert_eq!(j.at("profile.demo").and_then(Json::as_bool), Some(true));
    // determinismo y huella estables; cambia si cambia cualquier dato
    let out2 = run(&net(3.0, "", ""), "SC1", Mode::OperatingPoint, &demo(), None);
    assert_eq!(out.fingerprint, out2.fingerprint);
    assert_eq!(out.values.canonical(), out2.values.canonical());
    let out3 = run(&net(3.5, "", ""), "SC1", Mode::OperatingPoint, &demo(), None);
    assert_ne!(out.fingerprint, out3.fingerprint);
}

#[test]
fn presion_requerida_en_la_fuente_coincide_con_la_formula_cerrada() {
    // Clase demo_low: densidad 2.0 L/min/m2 x 12 m2 = 24 L/min por rociador; K=80 => P=(24/80)^2
    let z = 3.0;
    let out = run(&net(z, "", ""), "SCD", Mode::RequiredPressure, &demo(), None);
    assert_eq!(out.status, Status::Ok, "{:?}", out.issues);
    let req = out.values.field("required");
    let (r1, r2) = (r_hw(30.0, 65.0, 120.0), r_hw(10.0, 40.0, 120.0));
    let p_dev = (24.0f64 / 80.0).powi(2);
    let p_req = p_dev.max(0.5) + GAMMA * z + r2 * 24f64.powf(1.85) + r1 * 48f64.powf(1.85);
    // el criterio de presión mínima 0,5 bar domina sobre 0,09 bar
    let got = val(req, "required_pressure_bar");
    // con P_min = 0.5 bar el caudal de cada rociador será K*sqrt(0.5) = 56.57 > 24; recalcular referencia:
    let q_each = 80.0 * 0.5f64.sqrt();
    let p_ref = 0.5 + GAMMA * z + r2 * q_each.powf(1.85) + r1 * (2.0 * q_each).powf(1.85);
    assert!((got - p_ref).abs() < 1e-6, "got={} ref={} (p_req sin presión mínima={})", got, p_ref, p_req);
    assert_eq!(req.field("controlling_device").as_str().map(|s| s == "S1" || s == "S2"), Some(true));
    // el punto (Q, P) se compara con la curva de suministro
    let sup = &out.values.field("supplies").items()[0];
    assert!((val(sup, "flow_lpm") - 2.0 * q_each).abs() < 1e-6);
    let avail = 6.0 - 2.0 * ((2.0 * q_each) / 1000.0).powf(1.85);
    assert!((val(sup, "available_pressure_at_flow_bar") - avail).abs() < 1e-9);
    assert!(val(sup, "margin_bar") > 0.0);
    assert!(!out.issues.iter().any(|i| i.severity >= Severity::Error), "{:?}", out.issues);
}

#[test]
fn presion_requerida_solo_con_criterio_de_densidad() {
    // Sin presión mínima: manda la densidad. Quitamos min_pressure_bar de ambos rociadores.
    let txt = net(3.0, "", "").canonical().replace(r#","min_pressure_bar":0.5"#, "");
    let n = Json::parse(&txt).unwrap();
    let out = run(&n, "SCD", Mode::RequiredPressure, &demo(), None);
    assert_eq!(out.status, Status::Ok, "{:?}", out.issues);
    let (r1, r2) = (r_hw(30.0, 65.0, 120.0), r_hw(10.0, 40.0, 120.0));
    let p_ref = (24.0f64 / 80.0).powi(2) + GAMMA * 3.0 + r2 * 24f64.powf(1.85) + r1 * 48f64.powf(1.85);
    let got = val(out.values.field("required"), "required_pressure_bar");
    assert!((got - p_ref).abs() < 1e-6, "{} vs {}", got, p_ref);
    // cada rociador descarga exactamente la densidad de diseño x área (el criterio rige)
    assert!((val(node(&out.values, "S1"), "outflow_lpm") - 24.0).abs() < 1e-5);
}

#[test]
fn fuente_insuficiente_se_informa_como_error() {
    // Suministro pobre: 0.8 bar estáticos, 0.3 residual a 1000 L/min. La presión requerida en la
    // conexión (~0.84 bar para ~113 L/min) supera la disponible a ese caudal (~0.79 bar).
    let txt = net(3.0, "", "")
        .canonical()
        .replace(r#""static_pressure_bar":6"#, r#""static_pressure_bar":0.8"#)
        .replace(r#""residual_pressure_bar":4"#, r#""residual_pressure_bar":0.3"#);
    let out = run(&Json::parse(&txt).unwrap(), "SCD", Mode::RequiredPressure, &demo(), None);
    assert_eq!(out.status, Status::Ok, "{:?}", out.issues);
    assert!(out.issues.iter().any(|i| i.code == "hyd.supply_insufficient" && i.severity == Severity::Error), "{:?}", out.issues);
}

#[test]
fn accesorios_agregan_longitud_equivalente_de_la_tabla_del_perfil() {
    let base = net(0.0, "", "").canonical();
    let with = base.replace(
        r#"{"c_factor":120,"diameter_mm":65,"fittings":[],"from":"SUP","id":"P1","kind":"pipe","length_m":30,"to":"J"}"#,
        r#"{"c_factor":120,"diameter_mm":65,"fittings":[{"type":"elbow_90","count":3},{"type":"gate_valve","count":1}],"from":"SUP","id":"P1","kind":"pipe","length_m":30,"to":"J"}"#,
    );
    assert_ne!(base, with);
    let out = run(&Json::parse(&with).unwrap(), "SC1", Mode::OperatingPoint, &demo(), None);
    assert_eq!(out.status, Status::Ok, "{:?}", out.issues);
    // d=65 está en el rango [60,1000]: codo 2.0 m x3 + válvula 0.3 m = 6.3 m
    let p1 = link(&out.values, "P1");
    assert!((val(p1, "equiv_length_m") - 6.3).abs() < 1e-12);
    // la pérdida de P1 usa 36.3 m
    let q = val(p1, "flow_lpm");
    let expect = r_hw(36.3, 65.0, 120.0) * q.powf(1.85);
    assert!((val(p1, "headloss_bar") - expect).abs() < 1e-7 * expect.max(1.0), "{} vs {}", val(p1, "headloss_bar"), expect);
    assert!(out.params_used.iter().any(|p| p.path.starts_with("tables.hydraulics.fittings_equiv_length")));
}

#[test]
fn accesorio_sin_fila_en_la_tabla_bloquea() {
    let with = net(0.0, "", "")
        .canonical()
        .replace(r#""fittings":[],"from":"SUP""#, r#""fittings":[{"type":"accesorio_inexistente","count":1}],"from":"SUP""#);
    let out = run(&Json::parse(&with).unwrap(), "SC1", Mode::OperatingPoint, &demo(), None);
    assert_eq!(out.status, Status::Blocked);
    let i = out.issues.iter().find(|i| i.code == "profile.table_no_row").expect("debe indicar fila faltante");
    assert_eq!(i.object.as_deref(), Some("P1"));
}

#[test]
fn datos_faltantes_o_invalidos_bloquean_con_ruta_y_objeto() {
    // diámetro ausente
    let t = net(0.0, "", "")
        .canonical()
        .replace(r#""diameter_mm":40,"fittings":[],"from":"J","id":"P2""#, r#""fittings":[],"from":"J","id":"P2""#);
    let out = run(&Json::parse(&t).unwrap(), "SC1", Mode::OperatingPoint, &demo(), None);
    assert_eq!(out.status, Status::Blocked);
    assert!(out.values.is_null(), "un resultado bloqueado no debe traer valores parciales");
    let i = out.issues.iter().find(|i| i.code == "data.missing").unwrap();
    assert_eq!(i.object.as_deref(), Some("P2"));
    assert_eq!(i.path, "network.links.1.diameter_mm");
    // diámetro cero
    let t = net(0.0, "", "")
        .canonical()
        .replace(r#""diameter_mm":40,"fittings":[],"from":"J","id":"P2""#, r#""diameter_mm":0,"fittings":[],"from":"J","id":"P2""#);
    let out = run(&Json::parse(&t).unwrap(), "SC1", Mode::OperatingPoint, &demo(), None);
    assert!(out.issues.iter().any(|i| i.code == "data.not_positive" && i.object.as_deref() == Some("P2")));
    // K ausente en rociador activo
    let t = net(0.0, "", "").canonical().replace(r#""k_lpm_bar05":80,"#, "");
    let out = run(&Json::parse(&t).unwrap(), "SC1", Mode::OperatingPoint, &demo(), None);
    assert!(out.issues.iter().any(|i| i.code == "data.missing" && i.params.get("field").map(String::as_str) == Some("k_lpm_bar05")));
    // sin fuente
    let t = net(0.0, "", "").canonical().replace(r#""kind":"supply""#, r#""kind":"junction""#);
    let out = run(&Json::parse(&t).unwrap(), "SC1", Mode::OperatingPoint, &demo(), None);
    assert!(out.issues.iter().any(|i| i.code == "hyd.no_source"), "{:?}", out.issues);
    // escenario inexistente
    let out = run(&net(0.0, "", ""), "NOPE", Mode::OperatingPoint, &demo(), None);
    assert!(out.issues.iter().any(|i| i.code == "hyd.scenario_not_found"));
}

#[test]
fn parametro_del_perfil_ausente_o_con_unidad_incompatible_bloquea() {
    let mut p = demo_json();
    if let Some(Json::Obj(m)) = p.set_at_remove("parameters.hydraulics.friction", "exponent_flow") {
        let _ = m;
    }
    let prof = Profile::from_json(p).unwrap();
    let out = run(&net(0.0, "", ""), "SC1", Mode::OperatingPoint, &prof, None);
    assert_eq!(out.status, Status::Blocked);
    assert!(out.issues.iter().any(|i| i.code == "profile.param_missing" && i.path.ends_with("exponent_flow")), "{:?}", out.issues);
    // unidad incompatible
    let mut p = demo_json();
    p.set_at("parameters.hydraulics.fluid.density.unit", Json::str("bar"));
    let prof = Profile::from_json(p).unwrap();
    let out = run(&net(0.0, "", ""), "SC1", Mode::OperatingPoint, &prof, None);
    assert!(out.issues.iter().any(|i| i.code == "profile.unit_mismatch"), "{:?}", out.issues);
}

trait RemoveKey {
    fn set_at_remove(&mut self, path: &str, key: &str) -> Option<&mut Json>;
}
impl RemoveKey for Json {
    fn set_at_remove(&mut self, path: &str, key: &str) -> Option<&mut Json> {
        let mut cur = self;
        for seg in path.split('.') {
            cur = match cur {
                Json::Obj(m) => m.get_mut(seg)?,
                _ => return None,
            };
        }
        if let Json::Obj(m) = cur {
            m.remove(key);
        }
        Some(cur)
    }
}

#[test]
fn nodo_inalcanzable_con_carga_bloquea_y_sin_carga_solo_advierte() {
    // rociador S3 sin conexión
    let nodes = r#",{"id":"S3","kind":"sprinkler","elevation_m":0,"k_lpm_bar05":80}"#;
    let t = net(0.0, "", nodes).canonical().replace(
        r#""active_devices":["S1","S2"],"hazard_class":"demo_low""#,
        r#""active_devices":["S1","S2","S3"],"hazard_class":"demo_low""#,
    );
    let t = t.replace(r#""active_devices":["S1","S2"],"id":"SC1""#, r#""active_devices":["S1","S2","S3"],"id":"SC1""#);
    let out = run(&Json::parse(&t).unwrap(), "SC1", Mode::OperatingPoint, &demo(), None);
    assert_eq!(out.status, Status::Blocked);
    assert!(out.issues.iter().any(|i| i.code == "hyd.node_unreachable" && i.object.as_deref() == Some("S3")), "{:?}", out.issues);
    // nodo aislado sin carga: advertencia y cálculo normal
    let nodes = r#",{"id":"X","kind":"junction","elevation_m":0}"#;
    let out = run(&net(0.0, "", nodes), "SC1", Mode::OperatingPoint, &demo(), None);
    assert_eq!(out.status, Status::Ok);
    assert!(out.issues.iter().any(|i| i.code == "hyd.node_isolated" && i.severity == Severity::Warning));
}

#[test]
fn valvula_cerrada_aisla_y_bloquea_si_hay_carga_aguas_abajo() {
    let nodes = r#",{"id":"K","kind":"junction","elevation_m":0}"#;
    // P3 reemplazado por: J -> V (cerrada) -> K -> S2
    let t = net(0.0, "", nodes).canonical().replace(
        r#"{"c_factor":120,"diameter_mm":40,"fittings":[],"from":"J","id":"P3","kind":"pipe","length_m":10,"to":"S2"}"#,
        r#"{"c_factor":120,"diameter_mm":40,"fittings":[],"from":"J","id":"V1","kind":"valve","length_m":0.5,"state":"closed","to":"K"},{"c_factor":120,"diameter_mm":40,"fittings":[],"from":"K","id":"P3","kind":"pipe","length_m":10,"to":"S2"}"#,
    );
    let out = run(&Json::parse(&t).unwrap(), "SC1", Mode::OperatingPoint, &demo(), None);
    assert_eq!(out.status, Status::Blocked, "{:?}", out.issues);
    assert!(out.issues.iter().any(|i| i.code == "hyd.node_unreachable" && i.object.as_deref() == Some("S2")));
}

#[test]
fn exceder_la_velocidad_limite_es_error_pero_calcula() {
    // Límite ficticio de 1.5 m/s: la tubería principal de 65 mm lo supera con ~500 L/min.
    let mut p = demo_json();
    p.set_at("parameters.hydraulics.limits.max_velocity.value", Json::num(1.5));
    let prof = Profile::from_json(p).unwrap();
    let out = run(&net(0.0, "", ""), "SC1", Mode::OperatingPoint, &prof, None);
    assert_eq!(out.status, Status::Ok, "{:?}", out.issues);
    let v = out.issues.iter().find(|i| i.code == "hyd.velocity_exceeds" && i.object.as_deref() == Some("P1")).expect("debe señalar P1");
    assert_eq!(v.severity, Severity::Error);
    assert!(v.reference.as_deref().unwrap_or("").starts_with("DEMO"), "el aviso debe citar la referencia del perfil");
    // con el límite del perfil de demostración (12 m/s) no hay aviso
    let out = run(&net(0.0, "", ""), "SC1", Mode::OperatingPoint, &demo(), None);
    assert!(!out.issues.iter().any(|i| i.code == "hyd.velocity_exceeds"));
}

#[test]
fn presion_negativa_en_dispositivo_bloquea_el_resultado() {
    // Rociadores a 80 m de altura con 6 bar estáticos: la carga no alcanza
    let out = run(&net(80.0, "", ""), "SC1", Mode::OperatingPoint, &demo(), None);
    assert!(out.issues.iter().any(|i| i.code == "hyd.negative_pressure" && i.severity == Severity::Block), "{:?}", out.issues);
}

#[test]
fn sistema_no_cubierto_por_el_perfil_bloquea() {
    let t = net(0.0, "", "").canonical().replace("wet_pipe_sprinkler", "deluge_sprinkler");
    let out = run(&Json::parse(&t).unwrap(), "SC1", Mode::OperatingPoint, &demo(), None);
    assert_eq!(out.status, Status::Blocked);
    assert!(out.issues.iter().any(|i| i.code == "gate.profile_not_covering"));
}

#[test]
fn bomba_y_deposito_con_volumen() {
    let nodes =
        r#",{"id":"T","kind":"tank","elevation_m":0,"level_m":2.0,"usable_volume_m3":5.0},{"id":"D","kind":"junction","elevation_m":0}"#;
    let links = r#",{"id":"PU","kind":"pump","from":"T","to":"D","curve":[[0,12.0],[200,11.5],[400,10.5],[600,9.0],[800,7.0]]}"#;
    // Sustituimos P1 para alimentar J desde D (bomba) en lugar de SUP
    let t = net(0.0, links, nodes).canonical().replace(r#""from":"SUP","id":"P1""#, r#""from":"D","id":"P1""#);
    let out = run(&Json::parse(&t).unwrap(), "SCD", Mode::OperatingPoint, &demo(), None);
    // SUP queda aislado sin carga (advertencia) y el bombeo alimenta la red
    assert_eq!(out.status, Status::Ok, "{:?}", out.issues);
    let pumps = out.values.field("pumps").items();
    assert_eq!(pumps.len(), 1);
    assert_eq!(pumps[0].field("within_curve").as_bool(), Some(true));
    assert!(val(&pumps[0], "flow_lpm") > 0.0);
    // volumen requerido = caudal * duración (20 min? demo_low: 10 min) / 1000
    let tank = &out.values.field("tanks").items()[0];
    let q = val(tank, "outflow_lpm");
    assert!((val(tank, "required_volume_m3") - q * 10.0 / 1000.0).abs() < 1e-9);
    assert!(val(tank, "usable_volume_m3") == 5.0);
}
