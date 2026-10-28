//! Tablas de exportación compartidas por CSV, XLSX y PDF.
//!
//! Todas las columnas con magnitud física llevan su **unidad en el encabezado** (contrato 9.2).
//! Los datos de intercambio (CSV/XLSX/JSON) usan unidades canónicas con todos los dígitos; el PDF
//! convierte al sistema de unidades del proyecto y redondea a los decimales configurados.

use super::xlsx::{Cell, Sheet};
use super::ExportContext;
use crate::budget;
use crate::i18n::catalog;
use crate::issues::{Issue, Severity};
use crate::json::{fmt_num, Json};
use crate::units;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum V {
    S(String),
    N(f64),
    B(bool),
    E,
}

impl V {
    pub fn s<S: Into<String>>(x: S) -> V {
        V::S(x.into())
    }
    pub fn opt(x: Option<f64>) -> V {
        match x {
            Some(v) if v.is_finite() => V::N(v),
            _ => V::E,
        }
    }
    pub fn from_json(j: &Json) -> V {
        match j {
            Json::Num(n) => V::N(*n),
            Json::Str(s) => V::S(s.clone()),
            Json::Bool(b) => V::B(*b),
            _ => V::E,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Col {
    /// Clave i18n del encabezado.
    pub key: String,
    /// Unidad canónica de los valores (si es magnitud física).
    pub unit: Option<&'static str>,
    /// Rol de presentación para elegir la unidad del sistema de unidades (`pressure`, `flow`...).
    pub role: Option<&'static str>,
}

pub fn col(key: &str) -> Col {
    Col { key: key.to_string(), unit: None, role: None }
}
pub fn colu(key: &str, unit: &'static str, role: &'static str) -> Col {
    Col { key: key.to_string(), unit: Some(unit), role: Some(role) }
}

#[derive(Clone, Debug)]
pub struct Table {
    pub id: String,
    pub sheet_key: String,
    pub title: String,
    pub cols: Vec<Col>,
    pub rows: Vec<Vec<V>>,
}

/// Número para presentación: redondeo mitad-lejos-de-cero a `decimals`; un valor no nulo que el
/// redondeo dejaría en 0 se muestra en notación científica (p. ej. residuos 2.1e-14); `sep` es el
/// separador decimal del proyecto.
pub fn fmt_display(x: f64, decimals: u32, sep: char) -> String {
    if !x.is_finite() {
        return String::new();
    }
    let r = units::round_half_away(x, decimals);
    let t = if x != 0.0 && r == 0.0 {
        let e = format!("{:.2e}", x);
        let (m, ex) = e.split_once('e').unwrap_or((&e, "0"));
        let m = m.trim_end_matches('0').trim_end_matches('.');
        format!("{}e{}", m, ex)
    } else {
        fmt_num(r)
    };
    if sep == '.' {
        t
    } else {
        t.replace('.', &sep.to_string())
    }
}

/// Separador decimal del proyecto (coma por defecto en español, punto en inglés).
pub fn decimal_sep(project: &Json, lang: &str) -> char {
    match project.at("display.decimal_separator").and_then(Json::as_str) {
        Some(",") => ',',
        Some(".") => '.',
        _ => {
            if lang == "en" {
                '.'
            } else {
                ','
            }
        }
    }
}

/// Etiqueta legible de unidad.
pub fn pretty_unit(u: &str) -> String {
    match u {
        "C" => "°C".to_string(),
        "F" => "°F".to_string(),
        "gal_us" => "gal US".to_string(),
        other => other
            .replace("m2", "m²")
            .replace("m3", "m³")
            .replace("ft2", "ft²")
            .replace("ft3", "ft³")
            .replace("in2", "in²")
            .replace("in3", "in³"),
    }
}

/// Unidad mostrada para una columna: (unidad de presentación, factor de conversión desde la canónica).
fn display_unit(c: &Col, system: &str) -> Option<String> {
    let canon = c.unit?;
    match (c.role, system) {
        (Some(role), "US") | (Some(role), "SI") => {
            units::table().display_unit(system, role).map(|(_, u)| u.to_string()).or_else(|| Some(canon.to_string()))
        }
        _ => Some(canon.to_string()),
    }
}

impl Table {
    pub fn headers(&self, lang: &str, system: &str, canonical: bool) -> Vec<String> {
        let cat = catalog();
        self.cols
            .iter()
            .map(|c| {
                if let Some(raw) = c.key.strip_prefix("raw:") {
                    return format!("{} [s]", raw);
                }
                let label = cat.tr(lang, &c.key);
                match (c.unit, canonical) {
                    (Some(canon), true) => format!("{} [{}]", label, pretty_unit(canon)),
                    (Some(_), false) => format!("{} [{}]", label, pretty_unit(&display_unit(c, system).unwrap_or_default())),
                    _ => label,
                }
            })
            .collect()
    }

    /// Texto de cada celda. `canonical = true`: unidades canónicas, todos los dígitos y punto decimal
    /// (datos de intercambio). `canonical = false`: unidades del sistema del proyecto, redondeo a
    /// `decimals` y separador decimal `sep` (presentación).
    pub fn text_rows(&self, system: &str, canonical: bool, decimals: u32, sep: char) -> Vec<Vec<String>> {
        self.rows
            .iter()
            .map(|r| {
                r.iter()
                    .enumerate()
                    .map(|(i, v)| match v {
                        V::S(s) => s.clone(),
                        V::B(b) => if *b { "true" } else { "false" }.to_string(),
                        V::E => String::new(),
                        V::N(n) => {
                            let c = &self.cols[i];
                            if canonical {
                                return fmt_num(*n);
                            }
                            match c.unit {
                                None => fmt_display(*n, decimals, sep),
                                Some(canon) => {
                                    let to = display_unit(c, system).unwrap_or_default();
                                    let x = units::convert(*n, canon, &to).unwrap_or(*n);
                                    fmt_display(x, decimals, sep)
                                }
                            }
                        }
                    })
                    .collect()
            })
            .collect()
    }

    pub fn to_sheet(&self, lang: &str) -> Sheet {
        let cat = catalog();
        let mut rows = vec![self.headers(lang, "SI", true).into_iter().map(Cell::Str).collect::<Vec<_>>()];
        for r in &self.rows {
            rows.push(
                r.iter()
                    .map(|v| match v {
                        V::S(s) => Cell::Str(s.clone()),
                        V::N(n) => Cell::n(*n),
                        V::B(b) => Cell::Bool(*b),
                        V::E => Cell::Empty,
                    })
                    .collect(),
            );
        }
        Sheet { name: cat.tr(lang, &self.sheet_key), rows, header: true }
    }
}

// ------------------------------------------------------------------ valores curados por sistema
/// (clase de resultado, ruta en `values`, unidad canónica, rol de presentación)
/// Valor curado: (clase de resultado, ruta en `values`, (unidad canónica, rol de presentación)).
pub type ValueSpec = (&'static str, &'static str, Option<(&'static str, &'static str)>);

pub const VALUES: &[ValueSpec] = &[
    ("hydraulic", "summary.total_demand_lpm", Some(("L/min", "flow"))),
    ("hydraulic", "summary.source_flow_lpm", Some(("L/min", "flow"))),
    ("hydraulic", "summary.min_device_pressure_bar", Some(("bar", "pressure"))),
    ("hydraulic", "summary.max_velocity_m_s", Some(("m/s", "velocity"))),
    ("hydraulic", "summary.covered_area_m2", Some(("m2", "area"))),
    ("hydraulic", "required.required_pressure_bar", Some(("bar", "pressure"))),
    ("hydraulic", "required.controlling_device", None),
    ("hydraulic", "convergence.iterations", None),
    ("hydraulic", "convergence.residual_relative", None),
    ("hydraulic", "convergence.linear_solver", None),
    ("foam", "flows.solution_lpm", Some(("L/min", "flow"))),
    ("foam", "flows.concentrate_lpm", Some(("L/min", "flow"))),
    ("foam", "flows.water_lpm", Some(("L/min", "flow"))),
    ("foam", "volumes.solution_l", Some(("L", "volume_small"))),
    ("foam", "volumes.concentrate_l", Some(("L", "volume_small"))),
    ("foam", "volumes.concentrate_stored_l", Some(("L", "volume_small"))),
    ("foam", "duration_min", None),
    ("foam", "foam_discharge_m3_min", None),
    ("foam", "generators_required", None),
    ("foam", "hydraulic_demand.flow_lpm", Some(("L/min", "flow"))),
    ("foam", "hydraulic_demand.pressure_bar", Some(("bar", "pressure"))),
    ("gas", "agent_mass_kg", Some(("kg", "mass"))),
    ("gas", "gas_volume_reference_m3", Some(("m3", "volume"))),
    ("gas", "design_concentration_pct", None),
    ("gas", "details.specific_volume_m3_kg", None),
    ("gas", "details.altitude_correction", None),
    ("gas", "details.min_design_concentration_pct", None),
    ("gas", "details.noael_pct", None),
    ("gas", "details.loael_pct", None),
    ("gas", "cylinders.count", None),
    ("gas", "cylinders.fill_kg_each", Some(("kg", "mass"))),
    ("gas", "discharge_time_s", None),
    ("gas", "max_discharge_time_s", None),
    ("gas", "hold_time_min", None),
    ("gas", "required_hold_time_min", None),
    ("dry_chemical", "agent_mass_kg", Some(("kg", "mass"))),
    ("dry_chemical", "cylinders_required", None),
    ("dry_chemical", "nozzle_total_rate_kg_s", None),
    ("dry_chemical", "time_needed_s", None),
    ("dry_chemical", "discharge_time_s", None),
    ("wet_chemical", "total_flow_numbers", None),
    ("wet_chemical", "tank_capacity_flow_numbers", None),
    ("wet_chemical", "listing_ref", None),
    ("aerosol", "agent_mass_g", None),
    ("aerosol", "units_by_mass", None),
    ("aerosol", "units_by_volume", None),
    ("aerosol", "units_required", None),
    ("aerosol", "design_factor", None),
    ("aerosol", "listing_ref", None),
    ("extinguishers", "summary.zones_ok", None),
    ("extinguishers", "summary.zones_with_findings", None),
    ("extinguishers", "summary.units_total", None),
    ("extinguishers", "summary.uncovered_area_total_m2", Some(("m2", "area"))),
];

pub fn value_label_key(kind: &str, path: &str) -> String {
    format!("val.{}.{}", kind, path.replace('.', "_"))
}

/// Filas (etiqueta, texto) de los valores curados de un resultado almacenado.
pub fn value_rows(kind: &str, values: &Json, lang: &str, system: &str, decimals: u32, sep: char) -> Vec<(String, String)> {
    let cat = catalog();
    let mut out = Vec::new();
    for (k, path, unit) in VALUES.iter().filter(|v| v.0 == kind) {
        let Some(v) = values.at(path) else { continue };
        if v.is_null() {
            continue;
        }
        let label = cat.tr(lang, &value_label_key(k, path));
        let text = match (v, unit) {
            (Json::Num(n), Some((canon, role))) => {
                let to = units::table().display_unit(system, role).map(|(_, u)| u.to_string()).unwrap_or_else(|| canon.to_string());
                let x = units::convert(*n, canon, &to).unwrap_or(*n);
                format!("{} {}", fmt_display(x, decimals, sep), pretty_unit(&to))
            }
            (Json::Num(n), None) => fmt_display(*n, decimals.max(3), sep),
            (Json::Str(s), _) => s.clone(),
            (Json::Bool(b), _) => cat.tr(lang, if *b { "doc.yes" } else { "doc.no" }),
            _ => continue,
        };
        out.push((label, text));
    }
    out
}

// ------------------------------------------------------------------ metadatos
pub fn metadata_rows(ctx: &ExportContext) -> Vec<(String, String)> {
    let c = catalog();
    let l = ctx.lang;
    let p = ctx.project;
    let sv = |path: &str| p.at(path).and_then(Json::as_str).unwrap_or("").to_string();
    let mut v: Vec<(String, String)> = vec![
        (c.tr(l, "doc.meta.product"), c.tr(l, "app.name")),
        (c.tr(l, "doc.meta.version"), crate::ENGINE_VERSION.to_string()),
        (c.tr(l, "doc.project"), format!("{} ({})", sv("meta.name"), sv("meta.id"))),
        (c.tr(l, "doc.client"), sv("meta.client")),
        (c.tr(l, "doc.address"), sv("meta.address")),
        (c.tr(l, "doc.author"), sv("meta.author")),
        (c.tr(l, "doc.reviewer"), sv("meta.reviewer")),
        (c.tr(l, "doc.revision"), p.at("meta.revision").and_then(Json::as_f64).map(fmt_num).unwrap_or_default()),
        (c.tr(l, "doc.design_date"), sv("meta.design_date")),
        (c.tr(l, "doc.meta.units"), sv("display.unit_system")),
        (c.tr(l, "doc.status"), c.tr(l, &format!("status.{}", ctx.gate.status.as_str()))),
    ];
    match ctx.profile {
        Some(pr) => {
            v.push((c.tr(l, "doc.profile"), format!("{} v{}", pr.code(), pr.version())));
            v.push((
                c.tr(l, "doc.profile_state"),
                if pr.is_demo() {
                    c.tr(l, "status.profile_demo")
                } else if pr.is_verified() {
                    c.tr(l, "status.profile_verified")
                } else {
                    c.tr(l, "status.profile_unverified")
                },
            ));
            v.push((
                c.tr(l, "doc.meta.approval"),
                pr.raw.at("authorship.approval_status").and_then(Json::as_str).unwrap_or("").to_string(),
            ));
            v.push((c.tr(l, "doc.meta.profile_sha"), pr.sha256.clone()));
        }
        None => {
            v.push((c.tr(l, "doc.profile"), "-".to_string()));
            v.push((c.tr(l, "doc.profile_state"), c.tr(l, "status.profile_unverified")));
        }
    }
    v.push((c.tr(l, "doc.fingerprint"), crate::project::inputs_fingerprint(p)));
    v.push((c.tr(l, "doc.engine"), format!("{} {}", c.tr(l, "app.name"), crate::ENGINE_VERSION)));
    v.push((c.tr(l, "doc.generated"), format!("{} — {}", ctx.generated_at, c.tr(l, "doc.clock_note"))));
    v.push((c.tr(l, "doc.meta.user"), format!("{} / {}", ctx.user, ctx.role)));
    v.push((
        c.tr(l, "doc.meta.license"),
        format!(
            "{}{}",
            ctx.license.edition.as_str(),
            if ctx.license.watermark { format!(" — {}", c.tr(l, "status.eval_watermark")) } else { String::new() }
        ),
    ));
    if let Some(a) = ctx.audit_digest {
        v.push((c.tr(l, "doc.meta.audit"), a.to_string()));
    }
    v
}

pub fn metadata_table(ctx: &ExportContext) -> Table {
    Table {
        id: "metadata".into(),
        sheet_key: "xl.sheet.metadata".into(),
        title: catalog().tr(ctx.lang, "xl.sheet.metadata"),
        cols: vec![col("doc.col.parameter"), col("doc.col.value")],
        rows: metadata_rows(ctx)
            .into_iter()
            .chain(std::iter::once((catalog().tr(ctx.lang, "doc.meta.canonical_units"), String::new())))
            .map(|(k, v)| vec![V::S(k), V::S(v)])
            .collect(),
    }
}

// ------------------------------------------------------------------ incidencias
pub struct Found {
    pub issue: Issue,
    pub source: String,
}

/// Incidencias de la compuerta, de la validación del proyecto y de los resultados almacenados.
pub fn collect_issues(ctx: &ExportContext) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    let mut seen: BTreeMap<String, ()> = BTreeMap::new();
    let mut push = |i: Issue, src: &str| {
        let k = format!("{}|{}|{:?}|{}|{:?}", i.severity.as_str(), i.code, i.object, i.path, i.params);
        if seen.insert(k, ()).is_none() {
            out.push(Found { issue: i, source: src.to_string() });
        }
    };
    for i in &ctx.gate.issues.0 {
        push(i.clone(), "gate");
    }
    for i in crate::project::validate(ctx.project).0 {
        if i.severity != Severity::Pass {
            push(i, "project");
        }
    }
    if let Some(Json::Obj(res)) = ctx.project.get("results") {
        for (kind, node) in res {
            let envs: Vec<(String, &Json)> = if node.get("issues").is_some() {
                vec![(kind.clone(), node)]
            } else {
                node.as_obj().map(|m| m.iter().map(|(k, v)| (format!("{}:{}", kind, k), v)).collect()).unwrap_or_default()
            };
            for (label, env) in envs {
                for j in env.field("issues").items() {
                    if let Some(i) = Issue::from_json(j) {
                        if i.severity != Severity::Pass {
                            push(i, &label);
                        }
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| {
        b.issue
            .severity
            .cmp(&a.issue.severity)
            .then_with(|| a.issue.module.cmp(&b.issue.module))
            .then_with(|| a.issue.code.cmp(&b.issue.code))
            .then_with(|| a.issue.object.cmp(&b.issue.object))
    });
    out
}

pub fn issues_table(ctx: &ExportContext) -> Table {
    let cat = catalog();
    let rows: Vec<Vec<V>> = collect_issues(ctx)
        .into_iter()
        .map(|f| {
            let (msg, act) = cat.issue_text(ctx.lang, &f.issue);
            vec![
                V::S(cat.tr(ctx.lang, &format!("sev.{}", f.issue.severity.as_str()))),
                V::S(f.issue.code.clone()),
                V::S(f.source),
                V::S(f.issue.object.clone().unwrap_or_default()),
                V::S(msg),
                V::S(act),
                V::S(f.issue.reference.clone().unwrap_or_default()),
                V::S(f.issue.path.clone()),
            ]
        })
        .collect();
    Table {
        id: "issues".into(),
        sheet_key: "xl.sheet.issues".into(),
        title: cat.tr(ctx.lang, "doc.sec.issues"),
        cols: vec![
            col("doc.col.severity"),
            col("doc.col.code"),
            col("doc.col.kind"),
            col("doc.col.object"),
            col("doc.col.message"),
            col("doc.col.action"),
            col("doc.col.reference"),
            col("doc.col.parameter"),
        ],
        rows,
    }
}

// ------------------------------------------------------------------ parámetros normativos usados
pub fn params_table(ctx: &ExportContext) -> Table {
    let mut seen: BTreeMap<String, Vec<V>> = BTreeMap::new();
    if let Some(Json::Obj(res)) = ctx.project.get("results") {
        for node in res.values() {
            let envs: Vec<&Json> = if node.get("params_used").is_some() {
                vec![node]
            } else {
                node.as_obj().map(|m| m.values().collect()).unwrap_or_default()
            };
            for env in envs {
                for p in env.field("params_used").items() {
                    let path = p.field("path").as_str().unwrap_or("").to_string();
                    let key = format!("{}|{}", path, p.field("value").canonical());
                    seen.entry(key).or_insert_with(|| {
                        vec![
                            V::S(path),
                            V::from_json(p.field("value")),
                            V::S(p.field("unit").as_str().unwrap_or("").to_string()),
                            V::S(p.field("ref").as_str().unwrap_or("").to_string()),
                        ]
                    });
                }
            }
        }
    }
    Table {
        id: "params".into(),
        sheet_key: "xl.sheet.params".into(),
        title: catalog().tr(ctx.lang, "doc.sec.params"),
        cols: vec![col("doc.col.parameter"), col("doc.col.value"), col("doc.col.unit"), col("doc.col.reference")],
        rows: seen.into_values().collect(),
    }
}

// ------------------------------------------------------------------ extintores
pub fn ext_tables(ctx: &ExportContext) -> Vec<Table> {
    let cat = catalog();
    let l = ctx.lang;
    let mut zones = Vec::new();
    if let Some(env) = ctx.project.at("results.extinguishers") {
        for z in env.at("values.zones").map(Json::items).unwrap_or(&[]) {
            for c in z.field("classes").items() {
                let units: Vec<&str> = c.field("contributing_units").items().iter().filter_map(Json::as_str).collect();
                zones.push(vec![
                    V::from_json(z.field("id")),
                    V::from_json(z.field("hazard_class")),
                    V::from_json(c.field("fire_class")),
                    V::S(units.join(" ")),
                    V::opt(c.field("max_travel_m").as_f64()),
                    V::opt(c.field("covered_fraction").as_f64().map(|f| f * 100.0)),
                    V::opt(c.field("uncovered_area_m2").as_f64()),
                    V::S(cat.tr(l, if c.field("ok").as_bool() == Some(true) { "doc.finding_none" } else { "doc.finding_some" })),
                ]);
            }
        }
    }
    let zt = Table {
        id: "ext_zones".into(),
        sheet_key: "xl.sheet.zones".into(),
        title: cat.tr(l, "sys.portable_extinguishers"),
        cols: vec![
            col("doc.col.zone"),
            col("doc.col.hazard"),
            col("doc.col.class"),
            col("doc.col.contributing"),
            colu("doc.col.max_travel", "m", "length"),
            col("doc.col.covered_pct"),
            colu("doc.col.uncovered", "m2", "area"),
            col("doc.col.result"),
        ],
        rows: zones,
    };
    let mut units = Vec::new();
    for u in ctx.project.at("systems.extinguishers.units").map(Json::items).unwrap_or(&[]) {
        units.push(vec![
            V::from_json(u.field("id")),
            V::from_json(u.field("level")),
            V::opt(u.field("x").as_f64()),
            V::opt(u.field("y").as_f64()),
            V::from_json(u.field("agent")),
            V::S(u.field("ratings").canonical()),
        ]);
    }
    let ut = Table {
        id: "ext_units".into(),
        sheet_key: "xl.sheet.units".into(),
        title: cat.tr(l, "sys.portable_extinguishers"),
        cols: vec![
            col("doc.col.id"),
            col("doc.level"),
            colu("doc.col.length", "m", "length"),
            colu("doc.col.length", "m", "length"),
            col("doc.col.kind"),
            col("doc.col.value"),
        ],
        rows: units,
    };
    vec![zt, ut]
}

// ------------------------------------------------------------------ hidráulica
/// (etiqueta "red:escenario", sobre del resultado) de todos los cálculos hidráulicos guardados.
pub fn hydraulic_results<'a>(ctx: &ExportContext<'a>) -> Vec<(String, &'a Json)> {
    let p: &'a Json = ctx.project;
    p.at("results.hydraulic").and_then(Json::as_obj).map(|m| m.iter().map(|(k, v)| (k.clone(), v)).collect()).unwrap_or_default()
}

pub fn hydraulic_tables(ctx: &ExportContext) -> (Table, Table) {
    let cat = catalog();
    let l = ctx.lang;
    let mut nodes = Vec::new();
    let mut links = Vec::new();
    for (label, env) in hydraulic_results(ctx) {
        for n in env.at("values.nodes").map(Json::items).unwrap_or(&[]) {
            nodes.push(vec![
                V::S(label.clone()),
                V::from_json(n.field("id")),
                V::from_json(n.field("kind")),
                V::opt(n.field("elevation_m").as_f64()),
                V::opt(n.field("pressure_bar").as_f64()),
                V::opt(n.field("outflow_lpm").as_f64()),
            ]);
        }
        for k in env.at("values.links").map(Json::items).unwrap_or(&[]) {
            links.push(vec![
                V::S(label.clone()),
                V::from_json(k.field("id")),
                V::from_json(k.field("kind")),
                V::from_json(k.field("from")),
                V::from_json(k.field("to")),
                V::opt(k.field("length_m").as_f64()),
                V::opt(k.field("diameter_mm").as_f64()),
                V::opt(k.field("flow_lpm").as_f64()),
                V::opt(k.field("velocity_m_s").as_f64()),
                V::opt(k.field("headloss_bar").as_f64()),
            ]);
        }
    }
    (
        Table {
            id: "hyd_nodes".into(),
            sheet_key: "xl.sheet.nodes".into(),
            title: cat.tr(l, "res.hydraulic"),
            cols: vec![
                col("doc.col.parameter"),
                col("doc.col.id"),
                col("doc.col.kind"),
                colu("doc.col.elevation", "m", "length"),
                colu("doc.col.pressure", "bar", "pressure"),
                colu("doc.col.flow", "L/min", "flow"),
            ],
            rows: nodes,
        },
        Table {
            id: "hyd_links".into(),
            sheet_key: "xl.sheet.links".into(),
            title: cat.tr(l, "res.hydraulic"),
            cols: vec![
                col("doc.col.parameter"),
                col("doc.col.id"),
                col("doc.col.kind"),
                col("doc.col.from"),
                col("doc.col.to"),
                colu("doc.col.length", "m", "length"),
                colu("doc.col.diameter", "mm", "length_small"),
                colu("doc.col.flow", "L/min", "flow"),
                colu("doc.col.velocity", "m/s", "velocity"),
                colu("doc.col.headloss", "bar", "pressure"),
            ],
            rows: links,
        },
    )
}

// ------------------------------------------------------------------ agentes
pub fn agent_table(ctx: &ExportContext) -> Table {
    let cat = catalog();
    let l = ctx.lang;
    let system = ctx.project.at("display.unit_system").and_then(Json::as_str).unwrap_or("SI");
    let decimals = ctx.project.at("display.decimals").and_then(Json::as_f64).unwrap_or(2.0) as u32;
    let mut rows = Vec::new();
    for kind in ["foam", "gas", "dry_chemical", "wet_chemical", "aerosol"] {
        if let Some(m) = ctx.project.at(&format!("results.{}", kind)).and_then(Json::as_obj) {
            for (sid, env) in m {
                for (label, text) in value_rows(kind, env.field("values"), l, system, decimals, decimal_sep(ctx.project, l)) {
                    rows.push(vec![V::s(cat.tr(l, &format!("res.{}", kind))), V::s(sid.as_str()), V::s(label), V::S(text)]);
                }
            }
        }
    }
    Table {
        id: "agents".into(),
        sheet_key: "xl.sheet.agents".into(),
        title: cat.tr(l, "doc.sec.results"),
        cols: vec![col("doc.col.kind"), col("doc.col.id"), col("doc.col.parameter"), col("doc.col.value")],
        rows,
    }
}

// ------------------------------------------------------------------ causa-efecto
pub fn cause_effect_table(ctx: &ExportContext) -> Table {
    let cat = catalog();
    let l = ctx.lang;
    let ce = ctx.project.at("results.cause_effect.values").cloned().unwrap_or(Json::Null);
    let outputs: Vec<String> = ce.field("outputs").items().iter().filter_map(|o| o.field("id").as_str().map(|s| s.to_string())).collect();
    let mut cols = vec![col("doc.col.input"), col("doc.col.name")];
    let mut rows = Vec::new();
    // columnas dinámicas: una por salida (encabezado = id de salida); el título se resuelve con `doc.col.delay`
    for _ in &outputs {
        cols.push(col("doc.col.delay"));
    }
    for r in ce.field("rows").items() {
        let mut row = vec![V::from_json(r.field("input")), V::from_json(r.field("name"))];
        for o in &outputs {
            row.push(match r.at(&format!("effects.{}.delay_s", o)).and_then(Json::as_f64) {
                Some(d) => V::N(d),
                None => V::E,
            });
        }
        rows.push(row);
    }
    let mut t = Table {
        id: "cause_effect".into(),
        sheet_key: "xl.sheet.cause_effect".into(),
        title: cat.tr(l, "doc.sec.cause_effect"),
        cols,
        rows,
    };
    // los encabezados de las columnas de salida llevan el id de la salida
    for (i, o) in outputs.iter().enumerate() {
        t.cols[2 + i].key = format!("raw:{}", o);
    }
    t
}

// ------------------------------------------------------------------ cantidades y presupuesto
pub fn bom_table(ctx: &ExportContext) -> Table {
    let rows = budget::bom(ctx.project)
        .into_iter()
        .map(|b| vec![V::S(b.key), V::S(b.category), V::S(b.description), V::N(b.qty), V::S(b.unit), V::S(b.sources.join(" "))])
        .collect();
    Table {
        id: "bom".into(),
        sheet_key: "xl.sheet.bom".into(),
        title: catalog().tr(ctx.lang, "xl.sheet.bom"),
        cols: vec![
            col("doc.col.id"),
            col("doc.col.category"),
            col("doc.col.description"),
            col("doc.col.qty"),
            col("doc.col.unit"),
            col("doc.col.object"),
        ],
        rows,
    }
}

pub fn budget_table(ctx: &ExportContext) -> Table {
    let items: Vec<budget::CatalogItem> =
        ctx.project.at("catalog.items").map(Json::items).unwrap_or(&[]).iter().filter_map(budget::CatalogItem::from_json).collect();
    let out = budget::compute(ctx.project, &items);
    let mut rows = Vec::new();
    for l in out.values.field("lines").items() {
        rows.push(vec![
            V::from_json(l.field("id")),
            V::from_json(l.field("category")),
            V::from_json(l.field("description")),
            V::from_json(l.field("qty")),
            V::from_json(l.field("unit")),
            V::from_json(l.field("unit_price")),
            V::from_json(l.field("total")),
        ]);
    }
    if !out.values.is_null() {
        let cur = out.values.field("currency").as_str().unwrap_or("");
        rows.push(vec![
            V::E,
            V::E,
            V::S(catalog().tr(ctx.lang, "doc.sum.subtotal")),
            V::E,
            V::E,
            V::S(cur.to_string()),
            V::from_json(out.values.field("subtotal")),
        ]);
        for t in out.values.field("taxes").items() {
            rows.push(vec![
                V::E,
                V::E,
                V::from_json(t.field("name")),
                V::E,
                V::E,
                V::S(format!("{} %", t.field("rate_pct").as_str().unwrap_or(""))),
                V::from_json(t.field("amount")),
            ]);
        }
        rows.push(vec![
            V::E,
            V::E,
            V::S(catalog().tr(ctx.lang, "doc.sum.total")),
            V::E,
            V::E,
            V::S(cur.to_string()),
            V::from_json(out.values.field("total")),
        ]);
        if out.values.field("incomplete").as_bool() == Some(true) {
            rows.push(vec![V::E, V::E, V::S(catalog().tr(ctx.lang, "doc.sum.incomplete")), V::E, V::E, V::E, V::E]);
        }
    }
    Table {
        id: "budget".into(),
        sheet_key: "xl.sheet.budget".into(),
        title: catalog().tr(ctx.lang, "doc.sec.budget"),
        cols: vec![
            col("doc.col.id"),
            col("doc.col.category"),
            col("doc.col.description"),
            col("doc.col.qty"),
            col("doc.col.unit"),
            col("doc.col.price"),
            col("doc.col.total"),
        ],
        rows,
    }
}

/// Todas las tablas de datos de intercambio, en orden.
pub fn all_tables(ctx: &ExportContext) -> Vec<Table> {
    let mut v = vec![metadata_table(ctx), issues_table(ctx), params_table(ctx)];
    v.extend(ext_tables(ctx));
    let (n, k) = hydraulic_tables(ctx);
    v.push(n);
    v.push(k);
    v.push(agent_table(ctx));
    v.push(cause_effect_table(ctx));
    v.push(bom_table(ctx));
    v.push(budget_table(ctx));
    v
}
