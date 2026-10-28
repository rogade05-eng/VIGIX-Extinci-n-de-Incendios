//! Capa de dominio hidráulica: convierte la red del proyecto (JSON) en el problema numérico,
//! gobernado **íntegramente por el perfil normativo**, y verifica los resultados.
//!
//! Qué lee del perfil (ninguno tiene valor por defecto; si falta => bloqueo con ruta exacta):
//!  * `methods["hydraulics.friction"]` = `hazen_williams.power_law_si.v1` | `..._us.v1`
//!  * `parameters.hydraulics.fluid.density`            (densidad del fluido)
//!  * `parameters.hydraulics.friction.{coefficient,exponent_flow,exponent_diameter}`
//!  * `parameters.hydraulics.supply.curve_exponent`    (exponente de la curva de suministro)
//!  * opcionales: `limits.max_velocity`, método y tabla de accesorios, densidad/área de diseño.

use super::solver::{self, Link, LinkKind, Network, Node, Options, Solution, SolveError};
use crate::calc::{fingerprint, trace_step, CalcOutput, Status};
use crate::issues::{Issue, Severity};
use crate::json::Json;
use crate::profile::{Param, Profile};
use crate::reader::{Reader, Scope};
use crate::units;
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

pub const MODULE: &str = "water";
pub const FRICTION_SI: &str = "hazen_williams.power_law_si.v1";
pub const FRICTION_US: &str = "hazen_williams.power_law_us.v1";
pub const FITTINGS_PLAIN: &str = "equivalent_length_table.v1";
pub const FITTINGS_C_SCALED: &str = "equivalent_length_table_c_scaled.v1";

const DEVICE_KINDS: &[&str] = &["sprinkler", "nozzle", "hydrant", "hose_valve"];
const NODE_KINDS: &[&str] = &["junction", "sprinkler", "nozzle", "hydrant", "hose_valve", "supply", "tank", "reservoir"];
const LINK_KINDS: &[&str] = &["pipe", "valve", "pump"];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Basis {
    Si,
    Us,
}

#[derive(Clone, Debug)]
struct HydParams {
    gamma: f64,
    rho: f64,
    k: f64,
    n: f64,
    m: f64,
    basis: Basis,
    friction_method: String,
    supply_exponent: f64,
    fittings_method: Option<String>,
    c_ref: Option<f64>,
    vmax: Option<Param>,
}

fn load_params(profile: &Profile, issues: &mut Vec<Issue>, used: &mut Vec<Param>) -> Option<HydParams> {
    let mut ok = true;
    let take = |r: Result<Param, Issue>, used: &mut Vec<Param>, issues: &mut Vec<Issue>, ok: &mut bool| -> f64 {
        match r {
            Ok(p) => {
                let v = p.value;
                used.push(p);
                v
            }
            Err(i) => {
                issues.push(i);
                *ok = false;
                f64::NAN
            }
        }
    };
    let method = profile.method("hydraulics.friction").unwrap_or("").to_string();
    let basis = match method.as_str() {
        FRICTION_SI => Some(Basis::Si),
        FRICTION_US => Some(Basis::Us),
        _ => {
            issues.push(
                Issue::block("profile.method_unsupported")
                    .module(MODULE)
                    .path("methods.hydraulics.friction")
                    .param("method", method.as_str())
                    .param("module", "hydraulics.friction"),
            );
            ok = false;
            None
        }
    };
    let rho = take(profile.param_in("hydraulics.fluid.density", "kg/m3"), used, issues, &mut ok);
    let k = take(profile.param_in("hydraulics.friction.coefficient", "-"), used, issues, &mut ok);
    let n = take(profile.param_in("hydraulics.friction.exponent_flow", "-"), used, issues, &mut ok);
    let m = take(profile.param_in("hydraulics.friction.exponent_diameter", "-"), used, issues, &mut ok);
    let ns = take(profile.param_in("hydraulics.supply.curve_exponent", "-"), used, issues, &mut ok);
    if ok && !(rho > 0.0 && k > 0.0 && n > 0.0 && m > 0.0 && ns > 0.0) {
        issues.push(Issue::block("profile.param_invalid").module(MODULE).path("parameters.hydraulics").param("param", "rho/k/n/m/ns"));
        ok = false;
    }
    // Opcionales
    let fittings_method = profile.method("hydraulics.fittings").map(|s| s.to_string());
    let mut c_ref = None;
    match fittings_method.as_deref() {
        None | Some(FITTINGS_PLAIN) => {}
        Some(FITTINGS_C_SCALED) => match profile.param_in("hydraulics.fittings.c_reference", "-") {
            Ok(p) => {
                c_ref = Some(p.value);
                used.push(p);
            }
            Err(i) => {
                issues.push(i);
                ok = false;
            }
        },
        Some(other) => {
            issues.push(
                Issue::block("profile.method_unsupported")
                    .module(MODULE)
                    .path("methods.hydraulics.fittings")
                    .param("method", other)
                    .param("module", "hydraulics.fittings"),
            );
            ok = false;
        }
    }
    let vmax = match profile.param_in("hydraulics.limits.max_velocity", "m/s") {
        Ok(p) => Some(p),
        Err(i) if i.code == "profile.param_missing" => None,
        Err(i) => {
            issues.push(i);
            ok = false;
            None
        }
    };
    if !ok {
        return None;
    }
    if let Some(p) = &vmax {
        used.push(p.clone());
    }
    let g = units::table().standard_gravity;
    Some(HydParams {
        gamma: rho * g / 1.0e5,
        rho,
        k,
        n,
        m,
        basis: basis?,
        friction_method: method,
        supply_exponent: ns,
        fittings_method,
        c_ref,
        vmax,
    })
}

/// Resistencia `r` de un tramo: `h[bar] = r * Q[L/min]^n`.
fn pipe_resistance(hp: &HydParams, len_m: f64, d_mm: f64, c: f64) -> f64 {
    match hp.basis {
        Basis::Si => hp.k * len_m / (c.powf(hp.n) * d_mm.powf(hp.m)),
        Basis::Us => {
            let u = units::table();
            let l_ft = u.convert(len_m, "m", "ft").unwrap_or(f64::NAN);
            let d_in = u.convert(d_mm, "mm", "in").unwrap_or(f64::NAN);
            let q_scale = u.convert(1.0, "L/min", "gpm").unwrap_or(f64::NAN); // gpm por L/min
            let p_bar = u.convert(1.0, "psi", "bar").unwrap_or(f64::NAN); // bar por psi
            hp.k * l_ft * q_scale.powf(hp.n) / (c.powf(hp.n) * d_in.powf(hp.m)) * p_bar
        }
    }
}

fn velocity_m_s(q_lpm: f64, d_mm: f64) -> f64 {
    let area = std::f64::consts::FRAC_PI_4 * (d_mm / 1000.0).powi(2);
    (q_lpm / 60000.0) / area
}

#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub active: bool,
    pub k: Option<f64>,
    pub fixed_flow_lpm: Option<f64>,
    pub min_pressure_bar: Option<f64>,
    pub required_flow_lpm: Option<f64>,
    pub coverage_area_m2: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct NodeInfo {
    pub id: String,
    pub kind: String,
    pub elevation_m: f64,
    pub device: Option<DeviceInfo>,
    pub reachable: bool,
    pub tank_volume_m3: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct LinkInfo {
    pub id: String,
    pub kind: String,
    pub from: usize,
    pub to: usize,
    pub length_m: f64,
    pub equiv_length_m: f64,
    pub diameter_mm: f64,
    pub c: f64,
    pub closed: bool,
    pub reachable: bool,
}

#[derive(Clone, Debug)]
struct SupplyInfo {
    node: usize,
    static_bar: f64,
    residual_bar: f64,
    test_flow_lpm: f64,
    /// Solo en modo "operating": índices del nodo estático y del tramo de curva (en la red numérica).
    extra: Option<(usize, usize)>,
}

#[derive(Clone, Debug)]
pub struct Built {
    pub net: Network,
    pub nodes: Vec<NodeInfo>,
    pub links: Vec<LinkInfo>,
    /// índice numérico de cada nodo/tramo real (None si quedó aislado).
    snode: Vec<Option<usize>>,
    slink: Vec<Option<usize>>,
    supplies: Vec<SupplyInfo>,
    pumps: Vec<usize>,
    pub required_source: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    /// Se resuelve el punto de operación con la curva de suministro y bombas.
    OperatingPoint,
    /// Se busca la presión mínima en la conexión de la fuente que cumple todos los criterios.
    RequiredPressure,
}

struct Scenario<'a> {
    id: String,
    all: bool,
    ids: Vec<&'a str>,
    hazard: Option<&'a str>,
    required_source: Option<&'a str>,
    scope_path: String,
}

fn read_scenario<'a>(rd: &mut Reader, network: &'a Json, scenario_id: &str) -> Option<Scenario<'a>> {
    let root = Scope::new(network, "network");
    let list = network.field("scenarios").items();
    for (i, s) in list.iter().enumerate() {
        if s.field("id").as_str() == Some(scenario_id) {
            let sc = root.child("scenarios").item(i, s).with_id(scenario_id);
            let mut all = false;
            let mut ids: Vec<&str> = Vec::new();
            match s.field("active_devices") {
                Json::Str(t) if t == "all" => all = true,
                Json::Arr(a) => {
                    for v in a {
                        if let Some(t) = v.as_str() {
                            ids.push(t);
                        }
                    }
                }
                _ => {
                    rd.push(
                        Issue::block("data.missing")
                            .module(MODULE)
                            .path(&sc.key_path("active_devices"))
                            .object(scenario_id)
                            .param("field", "active_devices"),
                    );
                    return None;
                }
            }
            return Some(Scenario {
                id: scenario_id.to_string(),
                all,
                ids,
                hazard: s.field("hazard_class").as_str().filter(|t| !t.trim().is_empty()),
                required_source: s.field("required_source").as_str(),
                scope_path: sc.path.clone(),
            });
        }
    }
    rd.push(Issue::block("hyd.scenario_not_found").module(MODULE).path("network.scenarios").param("scenario", scenario_id));
    None
}

#[allow(clippy::too_many_arguments)]
fn build(
    network: &Json,
    sc: &Scenario,
    hp: &HydParams,
    profile: &Profile,
    mode: &Mode,
    rd: &mut Reader,
    used: &mut Vec<Param>,
    trace: &mut Vec<Json>,
) -> Option<Built> {
    let root = Scope::new(network, "network");
    let nodes_j = rd.arr(&root, "nodes")?;
    let links_j = rd.arr(&root, "links")?;
    let nodes_scope = root.child("nodes");
    let links_scope = root.child("links");

    // --- identificadores únicos
    let mut idx: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, nj) in nodes_j.iter().enumerate() {
        let s = nodes_scope.item(i, nj);
        if let Some(id) = rd.str(&s, "id") {
            if idx.insert(id, i).is_some() {
                rd.push(Issue::block("hyd.duplicate_id").module(MODULE).path(&s.key_path("id")).object(id).param("id", id));
            }
        }
    }
    let mut link_ids: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, lj) in links_j.iter().enumerate() {
        let s = links_scope.item(i, lj);
        if let Some(id) = rd.str(&s, "id") {
            if link_ids.insert(id, i).is_some() {
                rd.push(Issue::block("hyd.duplicate_id").module(MODULE).path(&s.key_path("id")).object(id).param("id", id));
            }
        }
    }
    if let Some(rs) = sc.required_source {
        if !idx.contains_key(rs) {
            rd.push(
                Issue::block("hyd.required_source_unknown")
                    .module(MODULE)
                    .path(&format!("{}.required_source", sc.scope_path))
                    .object(&sc.id)
                    .param("node", rs),
            );
        }
    }
    for id in &sc.ids {
        match idx.get(id) {
            Some(&i) => {
                let kind = nodes_j[i].field("kind").as_str().unwrap_or("");
                if !DEVICE_KINDS.contains(&kind) {
                    rd.push(
                        Issue::block("hyd.active_not_device")
                            .module(MODULE)
                            .path(&format!("{}.active_devices", sc.scope_path))
                            .object(id)
                            .param("node", *id),
                    );
                }
            }
            None => rd.push(
                Issue::block("hyd.active_unknown")
                    .module(MODULE)
                    .path(&format!("{}.active_devices", sc.scope_path))
                    .object(&sc.id)
                    .param("node", *id),
            ),
        }
    }
    if rd.has_block() {
        return None;
    }

    // --- nodos
    let mut infos: Vec<NodeInfo> = Vec::new();
    let mut supplies: Vec<SupplyInfo> = Vec::new();
    // nodos numéricos provisionales (índice == índice real); los extra se añaden al final
    let mut snodes: Vec<Node> = Vec::new();
    for (i, nj) in nodes_j.iter().enumerate() {
        let s = nodes_scope.item(i, nj);
        let id = nj.field("id").as_str().unwrap_or("?");
        let s = s.with_id(id);
        let kind = rd.enum_str(&s, "kind", NODE_KINDS).unwrap_or("junction");
        let z = rd.f64(&s, "elevation_m").unwrap_or(0.0);
        let mut node = Node { id: id.to_string(), elevation_m: z, fixed_pressure_bar: None, demand_lpm: 0.0, device_k: None };
        let mut dev: Option<DeviceInfo> = None;
        let mut tank_volume = None;
        match kind {
            k if DEVICE_KINDS.contains(&k) => {
                let active = sc.all || sc.ids.contains(&id);
                let has_k = !nj.field("k_lpm_bar05").is_null();
                let has_q = !nj.field("fixed_flow_lpm").is_null();
                let mut info = DeviceInfo {
                    active,
                    k: None,
                    fixed_flow_lpm: None,
                    min_pressure_bar: None,
                    required_flow_lpm: None,
                    coverage_area_m2: None,
                };
                if active {
                    if has_k && has_q {
                        rd.push(Issue::block("hyd.device_ambiguous").module(MODULE).path(&s.path).object(id));
                    } else if has_k {
                        info.k = rd.f64_pos(&s, "k_lpm_bar05");
                    } else if has_q {
                        info.fixed_flow_lpm = rd.f64_nonneg(&s, "fixed_flow_lpm");
                    } else {
                        rd.push(
                            Issue::block("data.missing")
                                .module(MODULE)
                                .path(&s.key_path("k_lpm_bar05"))
                                .object(id)
                                .param("field", "k_lpm_bar05"),
                        );
                    }
                    info.min_pressure_bar = rd.opt_f64(&s, "min_pressure_bar");
                    info.required_flow_lpm = rd.opt_f64(&s, "required_flow_lpm");
                    info.coverage_area_m2 = rd.opt_f64(&s, "coverage_area_m2");
                    node.device_k = info.k;
                    node.demand_lpm = info.fixed_flow_lpm.unwrap_or(0.0);
                }
                dev = Some(info);
            }
            "supply" => {
                let st = rd.f64_nonneg(&s, "static_pressure_bar");
                let rs = rd.f64_nonneg(&s, "residual_pressure_bar");
                let qt = rd.f64_pos(&s, "test_flow_lpm");
                if let (Some(st), Some(rs), Some(qt)) = (st, rs, qt) {
                    if st <= rs {
                        rd.push(
                            Issue::block("hyd.supply_curve_invalid")
                                .module(MODULE)
                                .path(&s.path)
                                .object(id)
                                .param("static", st)
                                .param("residual", rs),
                        );
                    } else {
                        supplies.push(SupplyInfo { node: i, static_bar: st, residual_bar: rs, test_flow_lpm: qt, extra: None });
                    }
                }
            }
            "tank" | "reservoir" => {
                let lvl = rd.f64_nonneg(&s, "level_m");
                if let Some(l) = lvl {
                    node.fixed_pressure_bar = Some(hp.gamma * l);
                }
                tank_volume = rd.opt_f64(&s, "usable_volume_m3");
            }
            _ => {}
        }
        infos.push(NodeInfo {
            id: id.to_string(),
            kind: kind.to_string(),
            elevation_m: z,
            device: dev,
            reachable: true,
            tank_volume_m3: tank_volume,
        });
        snodes.push(node);
    }

    // --- tramos
    struct RawLink {
        info: LinkInfo,
        kind: LinkKind,
        check: bool,
        q0: f64,
    }
    let mut raw: Vec<RawLink> = Vec::new();
    let mut pumps: Vec<usize> = Vec::new();
    let mut fit_trace: Vec<Json> = Vec::new();
    for (i, lj) in links_j.iter().enumerate() {
        let s = links_scope.item(i, lj);
        let id = lj.field("id").as_str().unwrap_or("?");
        let s = s.with_id(id);
        let kind = rd.enum_str(&s, "kind", LINK_KINDS).unwrap_or("pipe");
        let from = rd.str(&s, "from");
        let to = rd.str(&s, "to");
        let (Some(from), Some(to)) = (from, to) else { continue };
        let (Some(&fi), Some(&ti)) = (idx.get(from), idx.get(to)) else {
            rd.push(Issue::block("hyd.link_endpoint_unknown").module(MODULE).path(&s.path).object(id).param("from", from).param("to", to));
            continue;
        };
        if fi == ti {
            rd.push(Issue::block("hyd.link_self_loop").module(MODULE).path(&s.path).object(id));
            continue;
        }
        if kind == "pump" {
            let curve_j = rd.arr(&s, "curve");
            let Some(curve_j) = curve_j else { continue };
            let mut curve: Vec<(f64, f64)> = Vec::new();
            for pt in curve_j {
                match (pt.idx(0).and_then(Json::as_f64), pt.idx(1).and_then(Json::as_f64)) {
                    (Some(q), Some(g)) if q.is_finite() && g.is_finite() && q >= 0.0 && g >= 0.0 => curve.push((q, g)),
                    _ => {
                        rd.push(Issue::block("hyd.pump_curve_invalid").module(MODULE).path(&s.key_path("curve")).object(id));
                        curve.clear();
                        break;
                    }
                }
            }
            if curve.len() < 2 || !curve.windows(2).all(|w| w[1].0 > w[0].0 && w[1].1 < w[0].1) {
                rd.push(Issue::block("hyd.pump_curve_invalid").module(MODULE).path(&s.key_path("curve")).object(id));
                continue;
            }
            let qmid = 0.5 * (curve[0].0 + curve[curve.len() - 1].0);
            pumps.push(raw.len());
            raw.push(RawLink {
                info: LinkInfo {
                    id: id.to_string(),
                    kind: kind.to_string(),
                    from: fi,
                    to: ti,
                    length_m: 0.0,
                    equiv_length_m: 0.0,
                    diameter_mm: 0.0,
                    c: 0.0,
                    closed: false,
                    reachable: true,
                },
                kind: LinkKind::Pump { curve },
                check: true,
                q0: qmid,
            });
            continue;
        }
        // tubería o válvula
        let d = rd.f64_pos(&s, "diameter_mm");
        let c = rd.f64_pos(&s, "c_factor");
        let len = rd.f64_nonneg(&s, "length_m");
        let fit_arr = rd.arr(&s, "fittings");
        let closed = if kind == "valve" {
            match rd.enum_str(&s, "state", &["open", "closed"]) {
                Some(st) => st == "closed",
                None => continue,
            }
        } else {
            lj.field("status").as_str() == Some("closed")
        };
        let check = lj.field("check_valve").as_bool() == Some(true);
        let (Some(d), Some(c), Some(len), Some(fit_arr)) = (d, c, len, fit_arr) else { continue };
        // accesorios => longitud equivalente según la tabla del perfil
        let mut eq = 0.0;
        for (fi_idx, f) in fit_arr.iter().enumerate() {
            let fs = s.child("fittings").item(fi_idx, f);
            let (Some(ft), Some(cnt)) = (rd.str(&fs, "type"), rd.f64_nonneg(&fs, "count")) else { continue };
            if cnt == 0.0 {
                continue;
            }
            if hp.fittings_method.is_none() {
                rd.push(
                    Issue::block("profile.method_missing")
                        .module(MODULE)
                        .path("methods.hydraulics.fittings")
                        .object(id)
                        .param("module", "hydraulics.fittings"),
                );
                continue;
            }
            match profile.lookup_range_in("hydraulics.fittings_equiv_length", &[("fitting", ft)], d, "m") {
                Ok(p) => {
                    let mut one = p.value;
                    if hp.fittings_method.as_deref() == Some(FITTINGS_C_SCALED) {
                        if let Some(cr) = hp.c_ref {
                            one *= (c / cr).powf(hp.n);
                        }
                    }
                    eq += one * cnt;
                    if fit_trace.len() < 400 {
                        fit_trace.push(
                            Json::obj()
                                .with("link", Json::str(id))
                                .with("fitting", Json::str(ft))
                                .with("count", Json::num(cnt))
                                .with("diameter_mm", Json::num(d))
                                .with("table_value_m", Json::num(p.value))
                                .with("applied_each_m", Json::num(one))
                                .with("ref", Json::str(p.reference.as_str())),
                        );
                    }
                    if !used.iter().any(|u| u.path == p.path) {
                        used.push(p);
                    }
                }
                Err(iss) => rd.push(iss.object(id)),
            }
        }
        let total = len + eq;
        if total <= 0.0 {
            rd.push(
                Issue::block("data.not_positive")
                    .module(MODULE)
                    .path(&s.key_path("length_m"))
                    .object(id)
                    .param("field", "length_m")
                    .param("value", 0),
            );
            continue;
        }
        let r = pipe_resistance(hp, total, d, c);
        if !(r.is_finite() && r > 0.0) {
            rd.push(Issue::block("hyd.resistance_invalid").module(MODULE).path(&s.path).object(id));
            continue;
        }
        let q0 = std::f64::consts::FRAC_PI_4 * (d / 1000.0).powi(2) * 60000.0;
        raw.push(RawLink {
            info: LinkInfo {
                id: id.to_string(),
                kind: kind.to_string(),
                from: fi,
                to: ti,
                length_m: len,
                equiv_length_m: eq,
                diameter_mm: d,
                c,
                closed,
                reachable: true,
            },
            kind: if closed { LinkKind::Closed } else { LinkKind::Power { r, n: hp.n } },
            check,
            q0,
        });
    }
    if !fit_trace.is_empty() {
        trace.push(trace_step(
            "fittings",
            "L_eq = sum(count * table(fitting, d))  [* (C/C_ref)^n si el método del perfil lo indica]",
            Json::obj().with("method", Json::str(hp.fittings_method.as_deref().unwrap_or(""))),
            Json::Arr(fit_trace),
        ));
    }
    if rd.has_block() {
        return None;
    }

    // --- fuentes de agua: punto de operación (curva) o presión fija (modo requerido)
    let mut required_source: Option<usize> = None;
    if *mode == Mode::RequiredPressure {
        let pick = match sc.required_source {
            Some(id) => idx.get(id).copied(),
            None if supplies.len() == 1 => Some(supplies[0].node),
            None => None,
        };
        match pick {
            Some(i) if supplies.iter().any(|sp| sp.node == i) => required_source = Some(i),
            _ => {
                rd.push(
                    Issue::block("hyd.required_source_missing")
                        .module(MODULE)
                        .path(&format!("{}.required_source", sc.scope_path))
                        .object(&sc.id),
                );
                return None;
            }
        }
    }
    let n_real = snodes.len();
    let mut extra_links: Vec<Link> = Vec::new();
    let ns = hp.supply_exponent;
    for sp in supplies.iter_mut() {
        if Some(sp.node) == required_source {
            continue; // será nodo fijo con presión variable
        }
        let static_idx = snodes.len();
        snodes.push(Node {
            id: format!("{}#static", snodes[sp.node].id),
            elevation_m: snodes[sp.node].elevation_m,
            fixed_pressure_bar: Some(sp.static_bar),
            demand_lpm: 0.0,
            device_k: None,
        });
        let r = (sp.static_bar - sp.residual_bar) / sp.test_flow_lpm.powf(ns);
        let link_idx = raw.len() + extra_links.len();
        extra_links.push(Link {
            id: format!("{}#curve", snodes[sp.node].id),
            from: static_idx,
            to: sp.node,
            kind: LinkKind::Power { r, n: ns },
            check_valve: true,
            q0_lpm: 0.5 * sp.test_flow_lpm,
        });
        sp.extra = Some((static_idx, link_idx));
    }
    if let Some(rs) = required_source {
        snodes[rs].fixed_pressure_bar = Some(0.0); // se ajusta en la búsqueda
    }
    if !supplies.is_empty() {
        trace.push(trace_step(
            "supply_curve",
            "P_disp(Q) = P_est - (P_est - P_res) * (Q / Q_ens)^n_sum",
            Json::obj().with("n_supply", Json::num(ns)).with("supplies", Json::num(supplies.len() as f64)),
            Json::Null,
        ));
    }

    if !snodes.iter().any(|n| n.fixed_pressure_bar.is_some()) {
        rd.push(Issue::block("hyd.no_source").module(MODULE).path("network.nodes"));
        return None;
    }

    // --- alcanzabilidad desde nodos de carga fija a través de tramos abiertos
    let total_nodes = snodes.len();
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); total_nodes];
    let mut all_links: Vec<(usize, usize, bool)> = Vec::new(); // (from,to,open)
    for r in &raw {
        all_links.push((r.info.from, r.info.to, !matches!(r.kind, LinkKind::Closed)));
    }
    for l in &extra_links {
        all_links.push((l.from, l.to, true));
    }
    for &(a, b, open) in &all_links {
        if open {
            adj[a].push(b);
            adj[b].push(a);
        }
    }
    let mut reach = vec![false; total_nodes];
    let mut stack: Vec<usize> = Vec::new();
    for (i, n) in snodes.iter().enumerate() {
        if n.fixed_pressure_bar.is_some() {
            reach[i] = true;
            stack.push(i);
        }
    }
    while let Some(u) = stack.pop() {
        for &v in &adj[u] {
            if !reach[v] {
                reach[v] = true;
                stack.push(v);
            }
        }
    }
    for i in 0..n_real {
        infos[i].reachable = reach[i];
        if !reach[i] {
            let active_load = snodes[i].device_k.is_some() || snodes[i].demand_lpm > 0.0;
            if active_load {
                rd.push(
                    Issue::block("hyd.node_unreachable")
                        .module(MODULE)
                        .path(&format!("network.nodes.{}", i))
                        .object(&infos[i].id)
                        .param("node", infos[i].id.as_str()),
                );
            } else {
                rd.push(
                    Issue::warning("hyd.node_isolated")
                        .module(MODULE)
                        .path(&format!("network.nodes.{}", i))
                        .object(&infos[i].id)
                        .param("node", infos[i].id.as_str()),
                );
            }
        }
    }
    if rd.has_block() {
        return None;
    }
    if !snodes.iter().any(|n| n.fixed_pressure_bar.is_some()) {
        rd.push(Issue::block("hyd.no_source").module(MODULE).path("network.nodes"));
        return None;
    }

    // --- red numérica solo con la parte alcanzable
    let mut node_map: Vec<Option<usize>> = vec![None; total_nodes];
    let mut nodes_out: Vec<Node> = Vec::new();
    for (i, n) in snodes.iter().enumerate() {
        if reach[i] {
            node_map[i] = Some(nodes_out.len());
            nodes_out.push(n.clone());
        }
    }
    let mut links_out: Vec<Link> = Vec::new();
    let mut slink: Vec<Option<usize>> = vec![None; raw.len()];
    for (k, r) in raw.iter().enumerate() {
        if let (Some(a), Some(b)) = (node_map[r.info.from], node_map[r.info.to]) {
            slink[k] = Some(links_out.len());
            links_out.push(Link { id: r.info.id.clone(), from: a, to: b, kind: r.kind.clone(), check_valve: r.check, q0_lpm: r.q0 });
        }
    }
    let n_raw = raw.len();
    let mut extra_map: Vec<usize> = Vec::new();
    for l in extra_links.into_iter() {
        let (a, b) = (node_map[l.from], node_map[l.to]);
        if let (Some(a), Some(b)) = (a, b) {
            extra_map.push(links_out.len());
            links_out.push(Link { from: a, to: b, ..l });
        } else {
            extra_map.push(usize::MAX);
        }
    }
    let supplies: Vec<SupplyInfo> = supplies
        .into_iter()
        .map(|mut sp| {
            sp.extra =
                sp.extra.map(|(sn, sl)| (node_map[sn].unwrap_or(usize::MAX), extra_map.get(sl - n_raw).copied().unwrap_or(usize::MAX)));
            sp
        })
        .collect();
    let infos_links: Vec<LinkInfo> = raw
        .iter()
        .map(|r| {
            let mut li = r.info.clone();
            li.reachable = node_map[li.from].is_some() && node_map[li.to].is_some();
            li
        })
        .collect();
    Some(Built {
        net: Network { nodes: nodes_out, links: links_out, gamma: hp.gamma },
        nodes: infos,
        links: infos_links,
        snode: node_map[..n_real].to_vec(),
        slink,
        supplies,
        pumps,
        required_source: required_source.and_then(|i| node_map[i]),
    })
}

struct Eval {
    sol: Solution,
}

fn solver_options() -> Options {
    Options::default()
}

fn status_from_solve(e: &SolveError, rd: &mut Vec<Issue>) -> Status {
    match e {
        SolveError::Cancelled => Status::Cancelled,
        SolveError::NoFixedNode => {
            rd.push(Issue::block("hyd.no_source").module(MODULE));
            Status::Blocked
        }
        SolveError::Singular => {
            rd.push(Issue::block("hyd.singular").module(MODULE));
            Status::Blocked
        }
        SolveError::Numerical(m) => {
            rd.push(Issue::block("hyd.numerical_failure").module(MODULE).param("detail", m.as_str()));
            Status::NotConverged
        }
    }
}

/// Punto de partida: ¿cuál es la relación mínima criterio/valor de los dispositivos activos?
struct Criteria {
    /// (índice numérico del nodo, caudal mínimo L/min)
    flow: Vec<(usize, f64)>,
    /// (índice numérico del nodo, presión mínima bar)
    pressure: Vec<(usize, f64)>,
}

fn margin(c: &Criteria, sol: &Solution) -> (f64, Option<usize>) {
    let mut worst = f64::INFINITY;
    let mut who = None;
    for &(i, q) in &c.flow {
        let r = sol.outflow_lpm[i] / q;
        if r < worst {
            worst = r;
            who = Some(i);
        }
    }
    for &(i, p) in &c.pressure {
        let r = sol.pressure_bar[i] / p;
        if r < worst {
            worst = r;
            who = Some(i);
        }
    }
    (worst - 1.0, who)
}

/// Huella de entrada (red + escenario + modo + perfil + versión del motor).
pub fn input_fingerprint(network: &Json, scenario_id: &str, mode: &Mode, profile: &Profile) -> String {
    let mode_name = if *mode == Mode::OperatingPoint { "operating_point" } else { "required_pressure" };
    fingerprint("hydraulic", &[network, &Json::str(scenario_id), &Json::str(mode_name)], Some(profile))
}

/// Ejecuta el análisis hidráulico de una red del proyecto para un escenario.
pub fn run(network: &Json, scenario_id: &str, mode: Mode, profile: &Profile, cancel: Option<&AtomicBool>) -> CalcOutput {
    let mode_name = if mode == Mode::OperatingPoint { "operating_point" } else { "required_pressure" };
    let fp = input_fingerprint(network, scenario_id, &mode, profile);
    let mut rd = Reader::new(MODULE);
    let mut used: Vec<Param> = Vec::new();
    let mut trace: Vec<Json> = Vec::new();

    let system_type = network.field("system_type").as_str().unwrap_or("");
    if system_type.is_empty() {
        rd.push(Issue::block("data.missing").module(MODULE).path("network.system_type").param("field", "system_type"));
    } else if !profile.covers(system_type) {
        rd.push(Issue::block("gate.profile_not_covering").module(MODULE).path("network.system_type").param("system", system_type));
    }
    let hp = load_params(profile, &mut rd.issues, &mut used);
    let sc = read_scenario(&mut rd, network, scenario_id);
    let (Some(hp), Some(sc)) = (hp, sc) else {
        return CalcOutput::blocked("hydraulic", rd.issues, fp);
    };
    if rd.has_block() {
        return CalcOutput::blocked("hydraulic", rd.issues, fp);
    }
    trace.push(trace_step(
        "fluid",
        "gamma = rho * g / 1e5   [bar/m]",
        Json::obj().with("rho_kg_m3", Json::num(hp.rho)).with("g_m_s2", Json::num(units::table().standard_gravity)),
        Json::obj().with("gamma_bar_per_m", Json::num(hp.gamma)),
    ));
    let friction_eq = match hp.basis {
        Basis::Si => "dp[bar] = K * L[m] * Q[L/min]^n / (C^n * d[mm]^m)",
        Basis::Us => "dp[psi] = K * L[ft] * Q[gpm]^n / (C^n * d[in]^m)  (convertido a bar)",
    };
    trace.push(trace_step(
        "friction",
        friction_eq,
        Json::obj()
            .with("method", Json::str(hp.friction_method.as_str()))
            .with("K", Json::num(hp.k))
            .with("n", Json::num(hp.n))
            .with("m", Json::num(hp.m)),
        Json::Null,
    ));

    // Criterios de diseño del escenario (densidad × área de cobertura)
    let mut density: Option<Param> = None;
    let mut design_area: Option<Param> = None;
    let mut duration: Option<Param> = None;
    if let Some(h) = sc.hazard {
        match profile.lookup_in("hydraulics.design_density", &[("hazard_class", h)], "L/min/m2") {
            Ok(p) => {
                used.push(p.clone());
                density = Some(p);
            }
            Err(i) => rd.push(i.object(&sc.id)),
        }
        match profile.lookup_in("hydraulics.design_area", &[("hazard_class", h)], "m2") {
            Ok(p) => {
                used.push(p.clone());
                design_area = Some(p);
            }
            Err(i) => rd.push(i.object(&sc.id)),
        }
        if let Ok(p) = profile.lookup_in("hydraulics.supply_duration", &[("hazard_class", h)], "min") {
            used.push(p.clone());
            duration = Some(p);
        }
        if rd.has_block() {
            return CalcOutput::blocked("hydraulic", rd.issues, fp);
        }
    }

    let Some(built) = build(network, &sc, &hp, profile, &mode, &mut rd, &mut used, &mut trace) else {
        return CalcOutput::blocked("hydraulic", rd.issues, fp);
    };
    let mut issues = rd.issues; // advertencias de construcción incluidas

    // Criterios por dispositivo
    let mut crit = Criteria { flow: vec![], pressure: vec![] };
    let mut cov_sum = 0.0;
    let mut dev_req: BTreeMap<usize, f64> = BTreeMap::new();
    for (i, n) in built.nodes.iter().enumerate() {
        let (Some(d), Some(si)) = (&n.device, built.snode[i]) else { continue };
        if !d.active {
            continue;
        }
        if let Some(cov) = d.coverage_area_m2 {
            cov_sum += cov;
        }
        let mut req: Option<f64> = d.required_flow_lpm;
        if let Some(dp) = &density {
            match d.coverage_area_m2 {
                Some(a) if a > 0.0 => {
                    let q = dp.value * a;
                    req = Some(req.map(|x| x.max(q)).unwrap_or(q));
                }
                _ => {
                    if n.kind == "sprinkler" {
                        issues.push(
                            Issue::block("data.missing")
                                .module(MODULE)
                                .path(&format!("network.nodes.{}.coverage_area_m2", i))
                                .object(&n.id)
                                .param("field", "coverage_area_m2")
                                .reference(&dp.reference),
                        );
                    }
                }
            }
        }
        if let Some(q) = req {
            crit.flow.push((si, q));
            dev_req.insert(si, q);
        }
        if let Some(p) = d.min_pressure_bar {
            if p > 0.0 {
                crit.pressure.push((si, p));
            }
        }
    }
    if issues.iter().any(|i| i.severity == Severity::Block) {
        return CalcOutput::blocked("hydraulic", issues, fp);
    }

    // --- resolución
    let opts = solver_options();
    let mut net = built.net.clone();
    let mut required_info: Option<Json> = None;
    let sol: Solution;
    if mode == Mode::OperatingPoint {
        match solver::solve(&net, None, &opts, cancel) {
            Ok(s) => sol = s,
            Err(e) => {
                let st = status_from_solve(&e, &mut issues);
                return CalcOutput {
                    kind: "hydraulic".into(),
                    status: st,
                    values: Json::Null,
                    trace,
                    params_used: used,
                    issues,
                    fingerprint: fp,
                };
            }
        }
    } else {
        let Some(rs) = built.required_source else {
            issues.push(Issue::block("hyd.required_source_missing").module(MODULE));
            return CalcOutput::blocked("hydraulic", issues, fp);
        };
        if crit.flow.is_empty() && crit.pressure.is_empty() {
            issues.push(Issue::block("hyd.no_design_criteria").module(MODULE).object(&sc.id));
            return CalcOutput::blocked("hydraulic", issues, fp);
        }
        let mut evals = 0usize;
        let mut warm: Option<Solution> = None;
        let eval = |p: f64, net: &mut Network, warm: &mut Option<Solution>, issues: &mut Vec<Issue>| -> Result<(f64, Eval), Status> {
            net.nodes[rs].fixed_pressure_bar = Some(p);
            match solver::solve(net, warm.as_ref(), &opts, cancel) {
                Ok(s) => {
                    if !s.converged {
                        issues.push(Issue::block("hyd.not_converged").module(MODULE).param("iterations", s.iterations));
                        return Err(Status::NotConverged);
                    }
                    let (g, _) = margin(&crit, &s);
                    *warm = Some(s.clone());
                    Ok((g, Eval { sol: s }))
                }
                Err(e) => Err(status_from_solve(&e, issues)),
            }
        };
        // Búsqueda numérica (no normativa): cota superior inicial 1 bar, se duplica hasta
        // 1000 bar; si no se alcanza, se informa.
        let mut lo = 0.0f64;
        let (g0, e0) = match eval(lo, &mut net, &mut warm, &mut issues) {
            Ok(v) => v,
            Err(st) => {
                return CalcOutput {
                    kind: "hydraulic".into(),
                    status: st,
                    values: Json::Null,
                    trace,
                    params_used: used,
                    issues,
                    fingerprint: fp,
                }
            }
        };
        evals += 1;
        let mut best: Eval;
        let p_req: f64;
        if g0 >= 0.0 {
            p_req = lo;
            best = e0;
        } else {
            let mut hi = 1.0f64;
            let mut ghi;
            let mut ehi;
            loop {
                match eval(hi, &mut net, &mut warm, &mut issues) {
                    Ok((g, e)) => {
                        ghi = g;
                        ehi = e;
                    }
                    Err(st) => {
                        return CalcOutput {
                            kind: "hydraulic".into(),
                            status: st,
                            values: Json::Null,
                            trace,
                            params_used: used,
                            issues,
                            fingerprint: fp,
                        }
                    }
                }
                evals += 1;
                if ghi >= 0.0 {
                    break;
                }
                lo = hi;
                hi *= 2.0;
                if hi > 1000.0 {
                    issues.push(Issue::block("hyd.required_pressure_not_found").module(MODULE).param("limit_bar", 1000));
                    return CalcOutput {
                        kind: "hydraulic".into(),
                        status: Status::Blocked,
                        values: Json::Null,
                        trace,
                        params_used: used,
                        issues,
                        fingerprint: fp,
                    };
                }
            }
            // Illinois (regula falsi modificada) sobre [lo, hi]; g(lo) < 0 <= g(hi)
            let mut glo = if lo == 0.0 { g0 } else { -1.0 };
            if lo != 0.0 {
                match eval(lo, &mut net, &mut warm, &mut issues) {
                    Ok((g, _)) => glo = g,
                    Err(st) => {
                        return CalcOutput {
                            kind: "hydraulic".into(),
                            status: st,
                            values: Json::Null,
                            trace,
                            params_used: used,
                            issues,
                            fingerprint: fp,
                        }
                    }
                }
                evals += 1;
            }
            let mut side = 0i32;
            for _ in 0..60 {
                if (hi - lo) <= 1e-9 * hi.max(1e-3) || ghi.abs() < 1e-10 {
                    break;
                }
                let mut x = (lo * ghi - hi * glo) / (ghi - glo);
                if !x.is_finite() || x <= lo || x >= hi {
                    x = 0.5 * (lo + hi);
                }
                let (gx, ex) = match eval(x, &mut net, &mut warm, &mut issues) {
                    Ok(v) => v,
                    Err(st) => {
                        return CalcOutput {
                            kind: "hydraulic".into(),
                            status: st,
                            values: Json::Null,
                            trace,
                            params_used: used,
                            issues,
                            fingerprint: fp,
                        }
                    }
                };
                evals += 1;
                if gx >= 0.0 {
                    hi = x;
                    ghi = gx;
                    ehi = ex;
                    if side == 1 {
                        glo *= 0.5;
                    }
                    side = 1;
                } else {
                    lo = x;
                    glo = gx;
                    if side == -1 {
                        ghi *= 0.5;
                    }
                    side = -1;
                }
            }
            p_req = hi;
            best = ehi;
        }
        // Reevaluar en p_req para dejar la solución exactamente en ese punto
        net.nodes[rs].fixed_pressure_bar = Some(p_req);
        match solver::solve(&net, Some(&best.sol), &opts, cancel) {
            Ok(s) => best = Eval { sol: s },
            Err(e) => {
                let st = status_from_solve(&e, &mut issues);
                return CalcOutput {
                    kind: "hydraulic".into(),
                    status: st,
                    values: Json::Null,
                    trace,
                    params_used: used,
                    issues,
                    fingerprint: fp,
                };
            }
        }
        let (gfin, who) = margin(&crit, &best.sol);
        let real_of = |si: usize| built.snode.iter().position(|s| *s == Some(si));
        let controlling = who.and_then(real_of).map(|ri| built.nodes[ri].id.clone());
        let source_id = real_of(rs).map(|ri| built.nodes[ri].id.clone());
        required_info = Some(
            Json::obj()
                .with("required_pressure_bar", Json::num(p_req))
                .with("margin_ratio", Json::num(gfin))
                .with("evaluations", Json::num(evals as f64))
                .with("controlling_device", Json::opt_str(controlling.as_deref()))
                .with("source_node", Json::opt_str(source_id.as_deref())),
        );
        sol = best.sol;
    }

    // --- resultados y verificaciones
    let mut nodes_json: Vec<Json> = Vec::new();
    let mut min_dev_p = f64::INFINITY;
    let mut total_demand = 0.0;
    for (i, n) in built.nodes.iter().enumerate() {
        let mut o = Json::obj()
            .with("id", Json::str(n.id.as_str()))
            .with("kind", Json::str(n.kind.as_str()))
            .with("elevation_m", Json::num(n.elevation_m));
        match built.snode[i] {
            Some(si) => {
                o.set("head_bar", Json::num(sol.head_bar[si]));
                o.set("pressure_bar", Json::num(sol.pressure_bar[si]));
                o.set("outflow_lpm", Json::num(sol.outflow_lpm[si]));
                if let Some(d) = &n.device {
                    o.set("active", Json::Bool(d.active));
                    if d.active {
                        total_demand += sol.outflow_lpm[si];
                        min_dev_p = min_dev_p.min(sol.pressure_bar[si]);
                        o.set("required_flow_lpm", Json::opt_num(dev_req.get(&si).copied()));
                        o.set("min_pressure_bar", Json::opt_num(d.min_pressure_bar));
                        if sol.pressure_bar[si] < 0.0 {
                            issues.push(
                                Issue::block("hyd.negative_pressure")
                                    .module(MODULE)
                                    .path(&format!("network.nodes.{}", i))
                                    .object(&n.id)
                                    .param("pressure_bar", crate::json::fmt_num(sol.pressure_bar[si])),
                            );
                        }
                        if let Some(q) = dev_req.get(&si) {
                            if sol.outflow_lpm[si] < *q * (1.0 - 1e-9) {
                                let rf = density.as_ref().map(|p| p.reference.as_str()).unwrap_or("");
                                issues.push(
                                    Issue::error("hyd.min_flow")
                                        .module(MODULE)
                                        .path(&format!("network.nodes.{}", i))
                                        .object(&n.id)
                                        .param("flow_lpm", crate::json::fmt_num(sol.outflow_lpm[si]))
                                        .param("required_lpm", crate::json::fmt_num(*q))
                                        .reference(rf),
                                );
                            }
                        }
                        if let Some(p) = d.min_pressure_bar {
                            if sol.pressure_bar[si] < p * (1.0 - 1e-9) && sol.pressure_bar[si] >= 0.0 {
                                issues.push(
                                    Issue::error("hyd.min_pressure")
                                        .module(MODULE)
                                        .path(&format!("network.nodes.{}", i))
                                        .object(&n.id)
                                        .param("pressure_bar", crate::json::fmt_num(sol.pressure_bar[si]))
                                        .param("required_bar", crate::json::fmt_num(p)),
                                );
                            }
                        }
                    }
                }
            }
            None => {
                o.set("isolated", Json::Bool(true));
            }
        }
        nodes_json.push(o);
    }
    let mut links_json: Vec<Json> = Vec::new();
    let mut max_v = 0.0f64;
    let mut vmax_node: Option<String> = None;
    for (k, l) in built.links.iter().enumerate() {
        let mut o = Json::obj()
            .with("id", Json::str(l.id.as_str()))
            .with("kind", Json::str(l.kind.as_str()))
            .with("from", Json::str(built.nodes[l.from].id.as_str()))
            .with("to", Json::str(built.nodes[l.to].id.as_str()))
            .with("length_m", Json::num(l.length_m))
            .with("equiv_length_m", Json::num(l.equiv_length_m))
            .with("diameter_mm", if l.diameter_mm > 0.0 { Json::num(l.diameter_mm) } else { Json::Null })
            .with("c_factor", if l.c > 0.0 { Json::num(l.c) } else { Json::Null });
        match built.slink[k] {
            Some(sk) => {
                let q = sol.flow_lpm[sk];
                let nl = &built.net.links[sk];
                let dh = sol.head_bar[nl.from] - sol.head_bar[nl.to];
                o.set("flow_lpm", Json::num(q));
                o.set("open", Json::Bool(sol.link_open[sk]));
                o.set("headloss_bar", Json::num(dh));
                if l.diameter_mm > 0.0 {
                    let v = velocity_m_s(q.abs(), l.diameter_mm);
                    o.set("velocity_m_s", Json::num(v));
                    let tot = l.length_m + l.equiv_length_m;
                    o.set("unit_loss_bar_per_m", Json::num(if tot > 0.0 { dh.abs() / tot } else { 0.0 }));
                    if v > max_v {
                        max_v = v;
                        vmax_node = Some(l.id.clone());
                    }
                    if let Some(vm) = &hp.vmax {
                        if v > vm.value * (1.0 + 1e-9) {
                            issues.push(
                                Issue::error("hyd.velocity_exceeds")
                                    .module(MODULE)
                                    .path(&format!("network.links.{}", k))
                                    .object(&l.id)
                                    .param("velocity_m_s", crate::json::fmt_num(v))
                                    .param("limit_m_s", crate::json::fmt_num(vm.value))
                                    .reference(&vm.reference),
                            );
                        }
                    }
                }
            }
            None => {
                o.set("isolated", Json::Bool(true));
            }
        }
        links_json.push(o);
    }
    // bombas
    let mut pumps_json: Vec<Json> = Vec::new();
    for &pi in &built.pumps {
        let l = &built.links[pi];
        let Some(sk) = built.slink[pi] else { continue };
        let q = sol.flow_lpm[sk];
        let nl = &built.net.links[sk];
        let (qmin, qmax) = match &nl.kind {
            LinkKind::Pump { curve } => (curve[0].0, curve[curve.len() - 1].0),
            _ => (0.0, 0.0),
        };
        let within = q >= qmin - 1e-9 && q <= qmax + 1e-9;
        if sol.link_open[sk] && !within {
            issues.push(
                Issue::error("hyd.pump_out_of_curve")
                    .module(MODULE)
                    .path(&format!("network.links.{}", pi))
                    .object(&l.id)
                    .param("flow_lpm", crate::json::fmt_num(q))
                    .param("max_lpm", crate::json::fmt_num(qmax)),
            );
        }
        pumps_json.push(
            Json::obj()
                .with("id", Json::str(l.id.as_str()))
                .with("flow_lpm", Json::num(q))
                .with("gain_bar", Json::num(sol.head_bar[nl.to] - sol.head_bar[nl.from]))
                .with("curve_min_flow_lpm", Json::num(qmin))
                .with("curve_max_flow_lpm", Json::num(qmax))
                .with("within_curve", Json::Bool(within))
                .with("running", Json::Bool(sol.link_open[sk])),
        );
    }
    // fuentes de suministro
    let mut supply_json: Vec<Json> = Vec::new();
    let ns = hp.supply_exponent;
    for sp in &built.supplies {
        let id = built.nodes[sp.node].id.clone();
        let si = built.snode[sp.node];
        let (q, p_conn) = match (sp.extra, si) {
            (Some((_, sl)), Some(si)) if sl != usize::MAX => (sol.flow_lpm[sl], sol.pressure_bar[si]),
            (None, Some(si)) => (sol.source_flow_lpm.max(0.0), sol.pressure_bar[si]),
            _ => continue,
        };
        let avail = sp.static_bar - (sp.static_bar - sp.residual_bar) * (q.max(0.0) / sp.test_flow_lpm).powf(ns);
        let mut o = Json::obj()
            .with("id", Json::str(id.as_str()))
            .with("static_bar", Json::num(sp.static_bar))
            .with("residual_bar", Json::num(sp.residual_bar))
            .with("test_flow_lpm", Json::num(sp.test_flow_lpm))
            .with("flow_lpm", Json::num(q))
            .with("pressure_at_connection_bar", Json::num(p_conn))
            .with("available_pressure_at_flow_bar", Json::num(avail));
        if mode == Mode::RequiredPressure
            && Some(sp.node) == built.required_source.and_then(|rs| built.snode.iter().position(|s| *s == Some(rs)))
        {
            let margin_bar = avail - p_conn;
            o.set("required_pressure_bar", Json::num(p_conn));
            o.set("margin_bar", Json::num(margin_bar));
            if margin_bar < 0.0 {
                issues.push(
                    Issue::error("hyd.supply_insufficient")
                        .module(MODULE)
                        .path(&format!("network.nodes.{}", sp.node))
                        .object(&id)
                        .param("required_bar", crate::json::fmt_num(p_conn))
                        .param("available_bar", crate::json::fmt_num(avail))
                        .param("flow_lpm", crate::json::fmt_num(q)),
                );
            }
        }
        supply_json.push(o);
    }
    // depósitos
    let mut tanks_json: Vec<Json> = Vec::new();
    for (i, n) in built.nodes.iter().enumerate() {
        if n.kind != "tank" && n.kind != "reservoir" {
            continue;
        }
        let Some(si) = built.snode[i] else { continue };
        let out_q = sol.source_flow_lpm; // solo se informa si hay un único origen fijo
        let _ = out_q;
        let mut net_out = 0.0;
        for (k, l) in built.net.links.iter().enumerate() {
            if l.from == si {
                net_out += sol.flow_lpm[k];
            }
            if l.to == si {
                net_out -= sol.flow_lpm[k];
            }
        }
        let mut o = Json::obj().with("id", Json::str(n.id.as_str())).with("outflow_lpm", Json::num(net_out));
        if let (Some(dur), Some(vol)) = (&duration, n.tank_volume_m3) {
            let need = net_out.max(0.0) * dur.value / 1000.0;
            o.set("required_volume_m3", Json::num(need));
            o.set("usable_volume_m3", Json::num(vol));
            if vol < need {
                issues.push(
                    Issue::error("hyd.tank_volume_insufficient")
                        .module(MODULE)
                        .path(&format!("network.nodes.{}", i))
                        .object(&n.id)
                        .param("required_m3", crate::json::fmt_num(need))
                        .param("usable_m3", crate::json::fmt_num(vol))
                        .reference(&dur.reference),
                );
            }
        } else if n.kind == "tank" {
            issues.push(Issue::warning("hyd.tank_volume_unchecked").module(MODULE).path(&format!("network.nodes.{}", i)).object(&n.id));
        }
        tanks_json.push(o);
    }
    // área de diseño
    if let Some(da) = &design_area {
        if cov_sum + 1e-9 < da.value {
            issues.push(
                Issue::error("hyd.design_area_small")
                    .module(MODULE)
                    .path(&sc.scope_path)
                    .object(&sc.id)
                    .param("covered_m2", crate::json::fmt_num(cov_sum))
                    .param("required_m2", crate::json::fmt_num(da.value))
                    .reference(&da.reference),
            );
        }
    }
    // convergencia
    if !sol.converged {
        issues.push(Issue::block("hyd.not_converged").module(MODULE).param("iterations", sol.iterations));
    }
    if sol.rel_imbalance > 1e-3 {
        issues.push(Issue::block("hyd.residual_too_large").module(MODULE).param("residual", crate::json::fmt_num(sol.rel_imbalance)));
    }
    trace.push(trace_step(
        "solver",
        "Gradiente global (Todini-Pilati), Newton; dispositivos Q = K*sqrt(P)",
        Json::obj()
            .with("tol_rel", Json::num(opts.tol_rel))
            .with("tol_imbalance", Json::num(opts.tol_imbalance))
            .with("max_iter", Json::num(opts.max_iter as f64)),
        Json::obj()
            .with("iterations", Json::num(sol.iterations as f64))
            .with("linear_solver", Json::str(sol.linear_solver))
            .with("rel_change", Json::num(sol.rel_change)),
    ));
    if let Some(dp) = &density {
        trace.push(trace_step(
            "design_density",
            "Q_req,i = densidad * area_cobertura_i",
            Json::obj()
                .with("hazard_class", Json::str(sc.hazard.unwrap_or("")))
                .with("density_L_min_m2", Json::num(dp.value))
                .with("ref", Json::str(dp.reference.as_str())),
            Json::obj().with("covered_area_m2", Json::num(cov_sum)),
        ));
    }

    let converged_ok = sol.converged && sol.rel_imbalance <= 1e-3;
    let status = if converged_ok { Status::Ok } else { Status::NotConverged };
    let mut values = Json::obj()
        .with("mode", Json::str(mode_name))
        .with("scenario", Json::obj().with("id", Json::str(sc.id.as_str())).with("hazard_class", Json::opt_str(sc.hazard)))
        .with(
            "convergence",
            Json::obj()
                .with("converged", Json::Bool(sol.converged))
                .with("iterations", Json::num(sol.iterations as f64))
                .with("rel_change", Json::num(sol.rel_change))
                .with("residual_relative", Json::num(sol.rel_imbalance))
                .with("max_imbalance_lpm", Json::num(sol.max_imbalance_lpm))
                .with("max_link_error_bar", Json::num(sol.max_link_error_bar))
                .with("linear_solver", Json::str(sol.linear_solver)),
        )
        .with(
            "summary",
            Json::obj()
                .with("total_demand_lpm", Json::num(total_demand))
                .with("source_flow_lpm", Json::num(sol.source_flow_lpm))
                .with("min_device_pressure_bar", if min_dev_p.is_finite() { Json::num(min_dev_p) } else { Json::Null })
                .with("max_velocity_m_s", Json::num(max_v))
                .with("max_velocity_link", Json::opt_str(vmax_node.as_deref()))
                .with("covered_area_m2", Json::num(cov_sum))
                .with("gamma_bar_per_m", Json::num(hp.gamma)),
        )
        .with("nodes", Json::Arr(nodes_json))
        .with("links", Json::Arr(links_json))
        .with("pumps", Json::Arr(pumps_json))
        .with("supplies", Json::Arr(supply_json))
        .with("tanks", Json::Arr(tanks_json));
    if let Some(r) = required_info {
        values.set("required", r);
    }
    issues.sort_by(|a, b| b.severity.cmp(&a.severity).then_with(|| a.code.cmp(&b.code)).then_with(|| a.object.cmp(&b.object)));
    CalcOutput { kind: "hydraulic".into(), status, values, trace, params_used: used, issues, fingerprint: fp }
}
