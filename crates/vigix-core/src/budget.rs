//! Catálogo, lista de cantidades (BOM) y presupuesto (contrato 5.11).
//!
//! * El dinero se calcula con **decimales exactos** (enteros escalados), no con `f64`.
//! * Jamás se inventan precios: una línea sin precio queda "sin precio", no cuenta como 0 y el
//!   total se marca `incomplete`.
//! * No se mezclan monedas sin una tasa aportada por el usuario (no existe en esta versión).
//! * Cada total conserva la lista de líneas que lo componen (trazabilidad).

use crate::calc::{fingerprint, trace_step, CalcOutput, Status};
use crate::csv;
use crate::issues::{Issue, IssueList, Severity};
use crate::json::{fmt_num, Json};
use std::collections::BTreeMap;

pub const MODULE: &str = "budget";

// ------------------------------------------------------------------ decimal exacto
/// Decimal de punto fijo con 6 decimales (entero escalado).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Dec(pub i128);

impl Dec {
    pub const SCALE: i128 = 1_000_000;
    pub const ZERO: Dec = Dec(0);

    /// Acepta `12`, `12.5`, `-0.25` y la coma decimal `12,5` (si no hay punto). Rechaza separadores
    /// de millares y cualquier otro texto.
    pub fn parse(s: &str) -> Option<Dec> {
        let t = s.trim();
        if t.is_empty() {
            return None;
        }
        let t = if !t.contains('.') && t.matches(',').count() == 1 { t.replace(',', ".") } else { t.to_string() };
        let (neg, body) = match t.strip_prefix('-') {
            Some(r) => (true, r),
            None => (false, t.strip_prefix('+').unwrap_or(&t)),
        };
        let (int_s, frac_s) = match body.split_once('.') {
            Some((i, f)) => (i, f),
            None => (body, ""),
        };
        if int_s.is_empty() && frac_s.is_empty() {
            return None;
        }
        if !int_s.bytes().all(|b| b.is_ascii_digit()) || !frac_s.bytes().all(|b| b.is_ascii_digit()) || int_s.len() > 24 {
            return None;
        }
        let int: i128 = if int_s.is_empty() { 0 } else { int_s.parse().ok()? };
        let mut frac_digits: String = frac_s.chars().take(6).collect();
        let round_up = frac_s.chars().nth(6).map(|c| c >= '5').unwrap_or(false);
        while frac_digits.len() < 6 {
            frac_digits.push('0');
        }
        let mut v = int.checked_mul(Dec::SCALE)? + frac_digits.parse::<i128>().ok()?;
        if round_up {
            v += 1;
        }
        Some(Dec(if neg { -v } else { v }))
    }

    pub fn from_f64(x: f64) -> Option<Dec> {
        if !x.is_finite() || x.abs() > 1e18 {
            return None;
        }
        // Se pasa por la representación decimal más corta para no arrastrar error binario.
        Dec::parse(&format!("{}", x))
    }

    pub fn plus(self, o: Dec) -> Dec {
        Dec(self.0 + o.0)
    }
    pub fn minus(self, o: Dec) -> Dec {
        Dec(self.0 - o.0)
    }
    /// Producto con redondeo mitad-lejos-de-cero a 6 decimales.
    pub fn times(self, o: Dec) -> Dec {
        let p = self.0 * o.0;
        let half = Dec::SCALE / 2;
        Dec(if p >= 0 { (p + half) / Dec::SCALE } else { (p - half) / Dec::SCALE })
    }
    /// Redondeo mitad-lejos-de-cero a `dp` decimales (0..=6).
    pub fn round_dp(self, dp: u32) -> Dec {
        let dp = dp.min(6);
        let unit = 10i128.pow(6 - dp);
        let half = unit / 2;
        let v = self.0;
        Dec(if v >= 0 { (v + half) / unit * unit } else { (v - half) / unit * unit })
    }
    pub fn to_f64(self) -> f64 {
        self.0 as f64 / Dec::SCALE as f64
    }
    pub fn fmt(self, dp: u32) -> String {
        let r = self.round_dp(dp);
        let neg = r.0 < 0;
        let a = r.0.abs();
        let (int, frac) = (a / Dec::SCALE, a % Dec::SCALE);
        let dp = dp.min(6) as usize;
        let fs = format!("{:06}", frac);
        let mut s = String::new();
        if neg && (int != 0 || frac != 0) {
            s.push('-');
        }
        s.push_str(&int.to_string());
        if dp > 0 {
            s.push('.');
            s.push_str(&fs[..dp]);
        }
        s
    }
}

// ------------------------------------------------------------------ catálogo
pub const CATALOG_COLUMNS: [&str; 11] =
    ["id", "category", "description", "manufacturer", "model", "unit", "price", "currency", "source", "status", "notes"];
pub const CATALOG_STATUS: [&str; 4] = ["aportado_por_usuario", "fabricante", "demo", "verificado"];

#[derive(Clone, Debug, PartialEq)]
pub struct CatalogItem {
    pub id: String,
    pub category: String,
    pub description: String,
    pub manufacturer: String,
    pub model: String,
    pub unit: String,
    pub price: Option<Dec>,
    pub currency: Option<String>,
    pub source: String,
    pub status: String,
    pub notes: String,
}

impl CatalogItem {
    pub fn to_json(&self) -> Json {
        Json::obj()
            .with("id", Json::str(self.id.as_str()))
            .with("category", Json::str(self.category.as_str()))
            .with("description", Json::str(self.description.as_str()))
            .with("manufacturer", Json::str(self.manufacturer.as_str()))
            .with("model", Json::str(self.model.as_str()))
            .with("unit", Json::str(self.unit.as_str()))
            .with(
                "price",
                match self.price {
                    Some(p) => Json::str(p.fmt(6).trim_end_matches('0').trim_end_matches('.')),
                    None => Json::Null,
                },
            )
            .with("currency", Json::opt_str(self.currency.as_deref()))
            .with("source", Json::str(self.source.as_str()))
            .with("status", Json::str(self.status.as_str()))
            .with("notes", Json::str(self.notes.as_str()))
    }

    pub fn from_json(j: &Json) -> Option<CatalogItem> {
        let s = |k: &str| j.field(k).as_str().unwrap_or("").to_string();
        let price = match j.field("price") {
            Json::Null => None,
            Json::Str(t) if t.trim().is_empty() => None,
            Json::Str(t) => Some(Dec::parse(t)?),
            Json::Num(x) => Some(Dec::from_f64(*x)?),
            _ => return None,
        };
        Some(CatalogItem {
            id: s("id"),
            category: s("category"),
            description: s("description"),
            manufacturer: s("manufacturer"),
            model: s("model"),
            unit: s("unit"),
            price,
            currency: j.field("currency").as_str().filter(|c| !c.is_empty()).map(|c| c.to_string()),
            source: s("source"),
            status: s("status"),
            notes: s("notes"),
        })
    }
}

/// Importa un catálogo CSV validando cada fila. Devuelve los elementos válidos y las incidencias.
pub fn catalog_from_csv(text: &str) -> (Vec<CatalogItem>, IssueList) {
    let mut issues = IssueList::new();
    let delim = if text.lines().next().map(|l| l.matches(';').count() > l.matches(',').count()).unwrap_or(false) { ';' } else { ',' };
    let rows = match csv::parse(text, delim) {
        Ok(r) => r,
        Err(e) => {
            issues.push(Issue::block("catalog.csv_invalid").module(MODULE).param("detail", e));
            return (vec![], issues);
        }
    };
    let Some(header) = rows.first() else {
        issues.push(Issue::block("catalog.csv_empty").module(MODULE));
        return (vec![], issues);
    };
    let col: BTreeMap<&str, usize> = header.iter().enumerate().map(|(i, h)| (h.trim(), i)).collect();
    for need in ["id", "description", "unit", "status"] {
        if !col.contains_key(need) {
            issues.push(Issue::block("catalog.column_missing").module(MODULE).param("column", need));
        }
    }
    if issues.has_block() {
        return (vec![], issues);
    }
    let mut items: Vec<CatalogItem> = Vec::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for (ri, r) in rows.iter().enumerate().skip(1) {
        let get = |name: &str| col.get(name).and_then(|&i| r.get(i)).map(|s| s.trim().to_string()).unwrap_or_default();
        let line = ri + 1;
        let path = format!("catalog.items.{}", ri - 1);
        let id = get("id");
        if id.is_empty() {
            issues.push(Issue::error("catalog.row_invalid").module(MODULE).path(&path).param("line", line).param("field", "id"));
            continue;
        }
        if seen.insert(id.clone(), line).is_some() {
            issues.push(Issue::error("catalog.duplicate_id").module(MODULE).path(&path).object(&id).param("line", line));
            continue;
        }
        let price_txt = get("price");
        let price = if price_txt.is_empty() {
            None
        } else {
            match Dec::parse(&price_txt) {
                Some(p) if p >= Dec::ZERO => Some(p),
                _ => {
                    issues.push(
                        Issue::error("catalog.price_invalid")
                            .module(MODULE)
                            .path(&path)
                            .object(&id)
                            .param("line", line)
                            .param("value", price_txt.as_str()),
                    );
                    continue;
                }
            }
        };
        let currency = get("currency");
        if price.is_some() && !(currency.len() == 3 && currency.chars().all(|c| c.is_ascii_uppercase())) {
            issues.push(
                Issue::error("catalog.currency_invalid")
                    .module(MODULE)
                    .path(&path)
                    .object(&id)
                    .param("line", line)
                    .param("value", currency.as_str()),
            );
            continue;
        }
        let status = get("status");
        if !CATALOG_STATUS.contains(&status.as_str()) {
            issues.push(
                Issue::error("catalog.status_invalid")
                    .module(MODULE)
                    .path(&path)
                    .object(&id)
                    .param("line", line)
                    .param("value", status.as_str())
                    .param("allowed", CATALOG_STATUS.join("|")),
            );
            continue;
        }
        let (desc, unit) = (get("description"), get("unit"));
        if desc.is_empty() || unit.is_empty() {
            issues.push(
                Issue::error("catalog.row_invalid")
                    .module(MODULE)
                    .path(&path)
                    .object(&id)
                    .param("line", line)
                    .param("field", if desc.is_empty() { "description" } else { "unit" }),
            );
            continue;
        }
        if status == "demo" {
            issues.push(Issue::info("catalog.demo_item").module(MODULE).path(&path).object(&id));
        }
        items.push(CatalogItem {
            id,
            category: get("category"),
            description: desc,
            manufacturer: get("manufacturer"),
            model: get("model"),
            unit,
            price,
            currency: if currency.is_empty() { None } else { Some(currency) },
            source: get("source"),
            status,
            notes: get("notes"),
        });
    }
    (items, issues)
}

pub fn catalog_to_csv(items: &[CatalogItem]) -> String {
    let mut rows: Vec<Vec<String>> = vec![CATALOG_COLUMNS.iter().map(|s| s.to_string()).collect()];
    for it in items {
        rows.push(vec![
            it.id.clone(),
            it.category.clone(),
            it.description.clone(),
            it.manufacturer.clone(),
            it.model.clone(),
            it.unit.clone(),
            it.price.map(|p| p.fmt(6).trim_end_matches('0').trim_end_matches('.').to_string()).unwrap_or_default(),
            it.currency.clone().unwrap_or_default(),
            it.source.clone(),
            it.status.clone(),
            it.notes.clone(),
        ]);
    }
    csv::write(&rows, ',', true)
}

// ------------------------------------------------------------------ BOM
#[derive(Clone, Debug, PartialEq)]
pub struct BomLine {
    pub key: String,
    pub category: String,
    pub description: String,
    pub qty: f64,
    pub unit: String,
    pub sources: Vec<String>,
}

impl BomLine {
    pub fn to_json(&self) -> Json {
        Json::obj()
            .with("key", Json::str(self.key.as_str()))
            .with("category", Json::str(self.category.as_str()))
            .with("description", Json::str(self.description.as_str()))
            .with("qty", Json::num(self.qty))
            .with("unit", Json::str(self.unit.as_str()))
            .with("sources", Json::Arr(self.sources.iter().map(|s| Json::str(s.as_str())).collect()))
    }
}

struct Acc {
    category: String,
    description: String,
    unit: String,
    qty: f64,
    sources: Vec<String>,
}

fn add(m: &mut BTreeMap<String, Acc>, key: String, category: &str, description: String, unit: &str, qty: f64, source: &str) {
    let e = m.entry(key).or_insert_with(|| Acc {
        category: category.to_string(),
        description,
        unit: unit.to_string(),
        qty: 0.0,
        sources: vec![],
    });
    e.qty += qty;
    if !e.sources.iter().any(|s| s == source) {
        e.sources.push(source.to_string());
    }
}

/// Cantidades medidas en el modelo (sin precios). Orden determinista por clave.
pub fn bom(project: &Json) -> Vec<BomLine> {
    let mut m: BTreeMap<String, Acc> = BTreeMap::new();
    // redes de agua
    for net in project.at("systems.water.networks").map(Json::items).unwrap_or(&[]) {
        let nid = net.field("id").as_str().unwrap_or("?");
        for l in net.field("links").items() {
            let lid = l.field("id").as_str().unwrap_or("?");
            match l.field("kind").as_str() {
                Some("pipe") => {
                    let d = l.field("diameter_mm").as_f64().unwrap_or(0.0);
                    let len = l.field("length_m").as_f64().unwrap_or(0.0);
                    add(&mut m, format!("pipe|{}", fmt_num(d)), "tuberia", format!("Tubería DN/Ø {} mm", fmt_num(d)), "m", len, lid);
                    for f in l.field("fittings").items() {
                        let t = f.field("type").as_str().unwrap_or("?");
                        let c = f.field("count").as_f64().unwrap_or(0.0);
                        add(
                            &mut m,
                            format!("fitting|{}|{}", t, fmt_num(d)),
                            "accesorio",
                            format!("Accesorio {} Ø {} mm", t, fmt_num(d)),
                            "u",
                            c,
                            lid,
                        );
                    }
                }
                Some("valve") => add(
                    &mut m,
                    format!("valve|{}", fmt_num(l.field("diameter_mm").as_f64().unwrap_or(0.0))),
                    "valvula",
                    format!("Válvula Ø {} mm", fmt_num(l.field("diameter_mm").as_f64().unwrap_or(0.0))),
                    "u",
                    1.0,
                    lid,
                ),
                Some("pump") => add(&mut m, "pump".to_string(), "bomba", "Bomba".to_string(), "u", 1.0, lid),
                _ => {}
            }
        }
        for n in net.field("nodes").items() {
            let nd = n.field("id").as_str().unwrap_or("?");
            match n.field("kind").as_str() {
                Some(k @ ("sprinkler" | "nozzle" | "hydrant" | "hose_valve")) => {
                    let kf = n.field("k_lpm_bar05").as_f64();
                    let key = format!("device|{}|{}", k, kf.map(fmt_num).unwrap_or_else(|| "s/k".into()));
                    add(&mut m, key, "dispositivo", format!("{} (K={})", k, kf.map(fmt_num).unwrap_or_else(|| "n/d".into())), "u", 1.0, nd);
                }
                Some("tank") => add(&mut m, "tank".to_string(), "deposito", "Depósito".to_string(), "u", 1.0, nd),
                _ => {}
            }
        }
        let _ = nid;
    }
    // extintores portátiles
    for u in project.at("systems.extinguishers.units").map(Json::items).unwrap_or(&[]) {
        let agent = u.field("agent").as_str().unwrap_or("?");
        let rat = u.field("ratings").canonical();
        add(
            &mut m,
            format!("extinguisher|{}|{}", agent, rat),
            "extintor",
            format!("Extintor {} {}", agent, rat),
            "u",
            1.0,
            u.field("id").as_str().unwrap_or("?"),
        );
    }
    // cantidades de agente desde resultados vigentes
    let res = project.field("results");
    for (kind, label) in
        [("foam", "Concentrado de espuma"), ("gas", "Agente gaseoso"), ("dry_chemical", "Polvo químico"), ("aerosol", "Aerosol condensado")]
    {
        if let Some(map) = res.field(kind).as_obj() {
            for (sid, env) in map {
                let v = env.field("values");
                let (q, unit, what) = if let Some(x) = v.at("volumes.concentrate_stored_l").and_then(Json::as_f64) {
                    (x, "L", "concentrado almacenado")
                } else if let Some(x) = v.field("agent_mass_kg").as_f64() {
                    (x, "kg", "agente")
                } else if let Some(x) = v.field("gas_volume_reference_m3").as_f64() {
                    (x, "m3", "gas de referencia")
                } else if let Some(x) = v.field("agent_mass_g").as_f64() {
                    (x / 1000.0, "kg", "agente")
                } else {
                    continue;
                };
                add(&mut m, format!("agent|{}|{}", kind, sid), "agente", format!("{} ({}) — sistema {}", label, what, sid), unit, q, sid);
            }
        }
    }
    m.into_iter()
        .map(|(key, a)| BomLine { key, category: a.category, description: a.description, qty: a.qty, unit: a.unit, sources: a.sources })
        .collect()
}

// ------------------------------------------------------------------ presupuesto
/// Huella de entrada (presupuesto + catálogo del proyecto + cantidades medidas).
pub fn input_fingerprint(project: &Json) -> String {
    let boms = Json::Arr(bom(project).iter().map(BomLine::to_json).collect());
    fingerprint(
        "budget",
        &[project.field("budget"), project.field("catalog"), &boms, project.at("display.decimals").unwrap_or(&Json::Null)],
        None,
    )
}

pub fn compute(project: &Json, catalog: &[CatalogItem]) -> CalcOutput {
    let b = project.field("budget");
    let fp = input_fingerprint(project);
    let dp = project
        .at("display.decimals")
        .and_then(Json::as_f64)
        .filter(|d| (0.0..=6.0).contains(d) && d.fract() == 0.0)
        .map(|d| d as u32)
        .unwrap_or(2);
    let mut issues: Vec<Issue> = Vec::new();
    let cat_by_id: BTreeMap<&str, &CatalogItem> = catalog.iter().map(|c| (c.id.as_str(), c)).collect();
    let lines = b.field("lines").items();
    let mut currency: Option<String> = b.field("currency").as_str().filter(|c| !c.is_empty()).map(|c| c.to_string());
    let mut out_lines: Vec<Json> = Vec::new();
    let mut by_cat: BTreeMap<String, (Dec, Vec<String>)> = BTreeMap::new();
    let mut subtotal = Dec::ZERO;
    let mut components: Vec<String> = Vec::new();
    let mut unpriced = 0usize;
    let mut ids: BTreeMap<&str, ()> = BTreeMap::new();
    for (i, l) in lines.iter().enumerate() {
        let id = l.field("id").as_str().unwrap_or("");
        let path = format!("budget.lines.{}", i);
        if id.is_empty() {
            issues.push(Issue::block("data.missing").module(MODULE).path(&format!("{}.id", path)).param("field", "id"));
            continue;
        }
        if ids.insert(id, ()).is_some() {
            issues.push(Issue::block("geo.duplicate_id").module(MODULE).path(&format!("{}.id", path)).object(id).param("id", id));
            continue;
        }
        let qty = match l.field("qty") {
            Json::Num(q) if *q >= 0.0 => Dec::from_f64(*q),
            _ => None,
        };
        let Some(qty) = qty else {
            issues.push(Issue::block("data.missing").module(MODULE).path(&format!("{}.qty", path)).object(id).param("field", "qty"));
            continue;
        };
        let unit = l.field("unit").as_str().unwrap_or("").to_string();
        if unit.is_empty() {
            issues.push(Issue::block("data.missing").module(MODULE).path(&format!("{}.unit", path)).object(id).param("field", "unit"));
            continue;
        }
        // precio: propio de la línea o del catálogo
        let mut price: Option<Dec> = match l.field("unit_price") {
            Json::Null => None,
            Json::Str(t) if t.trim().is_empty() => None,
            Json::Str(t) => Dec::parse(t),
            Json::Num(x) => Dec::from_f64(*x),
            _ => None,
        };
        let mut cur: Option<String> = l.field("currency").as_str().filter(|c| !c.is_empty()).map(|c| c.to_string());
        let mut status = "manual".to_string();
        if let Some(r) = l.field("catalog_ref").as_str().filter(|r| !r.is_empty()) {
            match cat_by_id.get(r) {
                Some(item) => {
                    status = item.status.clone();
                    if item.unit != unit {
                        issues.push(
                            Issue::error("budget.unit_mismatch")
                                .module(MODULE)
                                .path(&format!("{}.unit", path))
                                .object(id)
                                .param("line_unit", unit.as_str())
                                .param("catalog_unit", item.unit.as_str()),
                        );
                    }
                    if price.is_none() {
                        price = item.price;
                        cur = item.currency.clone();
                    }
                    if item.status == "demo" {
                        issues.push(Issue::warning("budget.demo_item").module(MODULE).path(&path).object(id));
                    }
                }
                None => issues.push(
                    Issue::block("budget.catalog_ref_unknown")
                        .module(MODULE)
                        .path(&format!("{}.catalog_ref", path))
                        .object(id)
                        .param("ref", r),
                ),
            }
        }
        let category = l.field("category").as_str().unwrap_or("").to_string();
        let mut o = Json::obj()
            .with("id", Json::str(id))
            .with("description", l.field("description").clone())
            .with("category", Json::str(category.as_str()))
            .with("qty", Json::str(qty.fmt(6).trim_end_matches('0').trim_end_matches('.')))
            .with("unit", Json::str(unit.as_str()))
            .with("price_status", Json::str(status.as_str()));
        match price {
            Some(p) => {
                if currency.is_none() {
                    currency = cur.clone();
                }
                if cur.is_none() || cur != currency {
                    issues.push(
                        Issue::block("budget.currency_mismatch")
                            .module(MODULE)
                            .path(&format!("{}.currency", path))
                            .object(id)
                            .param("line", cur.as_deref().unwrap_or(""))
                            .param("budget", currency.as_deref().unwrap_or("")),
                    );
                    continue;
                }
                let total = qty.times(p).round_dp(dp);
                subtotal = subtotal.plus(total);
                components.push(id.to_string());
                let e = by_cat.entry(category.clone()).or_insert((Dec::ZERO, vec![]));
                e.0 = e.0.plus(total);
                e.1.push(id.to_string());
                o.set("unit_price", Json::str(p.fmt(6).trim_end_matches('0').trim_end_matches('.')));
                o.set("total", Json::str(total.fmt(dp)));
            }
            None => {
                unpriced += 1;
                issues.push(Issue::warning("budget.line_unpriced").module(MODULE).path(&path).object(id));
                o.set("unit_price", Json::Null);
                o.set("total", Json::Null);
            }
        }
        out_lines.push(o);
    }
    // cantidades medidas que no están presupuestadas
    let linked: Vec<&str> = lines.iter().filter_map(|l| l.field("bom_key").as_str()).collect();
    for bl in bom(project) {
        if !linked.contains(&bl.key.as_str()) {
            issues.push(
                Issue::warning("budget.bom_line_unbudgeted")
                    .module(MODULE)
                    .path("budget.lines")
                    .param("key", bl.key.as_str())
                    .param("qty", fmt_num(bl.qty))
                    .param("unit", bl.unit.as_str()),
            );
        }
    }
    // impuestos aportados por el usuario (sobre el subtotal, sin acumulación)
    let mut taxes_json: Vec<Json> = Vec::new();
    let mut taxes_total = Dec::ZERO;
    for (i, t) in b.field("taxes").items().iter().enumerate() {
        let name = t.field("name").as_str().unwrap_or("");
        let rate = match t.field("rate_pct") {
            Json::Num(r) if *r >= 0.0 && *r <= 100.0 => Dec::from_f64(*r),
            _ => None,
        };
        let (true, Some(rate)) = (!name.is_empty(), rate) else {
            issues.push(Issue::block("data.missing").module(MODULE).path(&format!("budget.taxes.{}", i)).param("field", "name/rate_pct"));
            continue;
        };
        let amount = subtotal.times(rate).times(Dec(Dec::SCALE / 100)).round_dp(dp);
        taxes_total = taxes_total.plus(amount);
        taxes_json.push(
            Json::obj()
                .with("name", Json::str(name))
                .with("rate_pct", Json::str(rate.fmt(6).trim_end_matches('0').trim_end_matches('.')))
                .with("amount", Json::str(amount.fmt(dp)))
                .with("base_lines", Json::Arr(components.iter().map(|c| Json::str(c.as_str())).collect())),
        );
    }
    let blocked = issues.iter().any(|i| i.severity == Severity::Block);
    issues.sort_by(|a, b| b.severity.cmp(&a.severity).then_with(|| a.code.cmp(&b.code)).then_with(|| a.object.cmp(&b.object)));
    let trace = vec![trace_step(
        "budget",
        "total_línea = redondeo(cantidad * precio_unitario, d); subtotal = suma de totales de línea; impuesto = redondeo(subtotal * tasa/100, d)",
        Json::obj().with("decimals", Json::num(dp as f64)).with("rounding", Json::str("half_away_from_zero_per_line")),
        Json::obj().with("subtotal", Json::str(subtotal.fmt(dp))),
    )];
    if blocked {
        return CalcOutput {
            kind: "budget".into(),
            status: Status::Blocked,
            values: Json::Null,
            trace,
            params_used: vec![],
            issues,
            fingerprint: fp,
        };
    }
    let subtotals: Vec<Json> = by_cat
        .iter()
        .map(|(c, (v, ls))| {
            Json::obj()
                .with("category", Json::str(c.as_str()))
                .with("subtotal", Json::str(v.fmt(dp)))
                .with("lines", Json::Arr(ls.iter().map(|x| Json::str(x.as_str())).collect()))
        })
        .collect();
    let total = subtotal.plus(taxes_total);
    let values = Json::obj()
        .with("currency", Json::opt_str(currency.as_deref()))
        .with("decimals", Json::num(dp as f64))
        .with("lines", Json::Arr(out_lines))
        .with("subtotals_by_category", Json::Arr(subtotals))
        .with("subtotal", Json::str(subtotal.fmt(dp)))
        .with("subtotal_components", Json::Arr(components.iter().map(|c| Json::str(c.as_str())).collect()))
        .with("taxes", Json::Arr(taxes_json))
        .with("total", Json::str(total.fmt(dp)))
        .with("incomplete", Json::Bool(unpriced > 0))
        .with("unpriced_lines", Json::num(unpriced as f64))
        .with("rounding", Json::str("half_away_from_zero_per_line"));
    CalcOutput { kind: "budget".into(), status: Status::Ok, values, trace, params_used: vec![], issues, fingerprint: fp }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_exacto_sin_error_binario() {
        // 0.1 + 0.2 en f64 no es 0.3; en Dec sí
        assert_eq!(Dec::parse("0.1").unwrap().plus(Dec::parse("0.2").unwrap()), Dec::parse("0.3").unwrap());
        assert_eq!(Dec::parse("12,5").unwrap(), Dec::parse("12.5").unwrap());
        assert_eq!(Dec::parse("-0.25").unwrap().fmt(2), "-0.25");
        assert_eq!(Dec::parse("1.005").unwrap().round_dp(2).fmt(2), "1.01");
        assert_eq!(Dec::parse("-1.005").unwrap().round_dp(2).fmt(2), "-1.01");
        assert_eq!(Dec::parse("2.5").unwrap().round_dp(0).fmt(0), "3");
        assert_eq!(Dec::parse("19.99").unwrap().times(Dec::parse("3").unwrap()).fmt(2), "59.97");
        assert_eq!(Dec::from_f64(0.1 + 0.2).unwrap().round_dp(2).fmt(2), "0.30");
        for bad in ["", "abc", "1.2.3", "1,234.50", "12 5", "--1", "1e5", "."] {
            assert!(Dec::parse(bad).is_none(), "{:?}", bad);
        }
        assert_eq!(Dec::parse("0.0000005").unwrap().fmt(6), "0.000001"); // redondeo al 7.º decimal
        assert_eq!(Dec(-5).fmt(2), "0.00"); // sin "-0.00"
    }

    const CSV: &str = "id,category,description,manufacturer,model,unit,price,currency,source,status,notes\r\n\
        SPR-1,dispositivo,Rociador K80,,,u,12.50,USD,Catálogo del fabricante X,fabricante,\r\n\
        PIPE-65,tuberia,Tubería acero 65 mm,,,m,8.20,USD,Cotización 123,aportado_por_usuario,\r\n\
        EXT-1,extintor,Extintor demo,,,u,,,,demo,sin precio\r\n";

    #[test]
    fn importa_catalogo_y_valida_filas() {
        let (items, iss) = catalog_from_csv(CSV);
        assert_eq!(items.len(), 3, "{:?}", iss);
        assert_eq!(items[0].price, Dec::parse("12.50"));
        assert!(items[2].price.is_none(), "sin precio no es 0");
        assert!(iss.0.iter().any(|i| i.code == "catalog.demo_item" && i.object.as_deref() == Some("EXT-1")));
        // ida y vuelta
        let (again, _) = catalog_from_csv(&catalog_to_csv(&items));
        assert_eq!(again, items);
    }

    #[test]
    fn filas_invalidas_se_reportan_con_linea() {
        let bad = "id,description,unit,status,price,currency\n\
            A,desc,u,fabricante,10,USD\n\
            A,duplicada,u,fabricante,10,USD\n\
            B,desc,u,inventado,10,USD\n\
            C,desc,u,fabricante,abc,USD\n\
            D,desc,u,fabricante,5,usd\n\
            ,desc,u,fabricante,5,USD\n\
            E,,u,fabricante,,\n";
        let (items, iss) = catalog_from_csv(bad);
        assert_eq!(items.len(), 1);
        let codes: Vec<&str> = iss.0.iter().map(|i| i.code.as_str()).collect();
        for c in
            ["catalog.duplicate_id", "catalog.status_invalid", "catalog.price_invalid", "catalog.currency_invalid", "catalog.row_invalid"]
        {
            assert!(codes.contains(&c), "falta {}: {:?}", c, codes);
        }
        let d = iss.0.iter().find(|i| i.code == "catalog.duplicate_id").unwrap();
        assert_eq!(d.params.get("line").map(String::as_str), Some("3"));
        // faltan columnas obligatorias
        let (none, iss) = catalog_from_csv("a,b\n1,2\n");
        assert!(none.is_empty() && iss.has_block());
        assert!(catalog_from_csv("a,\"b\n").1.has_block());
        // punto y coma (CSV regional)
        let (it, _) = catalog_from_csv("id;description;unit;status;price;currency\nX;Algo;u;fabricante;1,5;EUR\n");
        assert_eq!(it[0].price, Dec::parse("1.5"));
    }

    fn project() -> Json {
        Json::parse(
            r#"{"display":{"decimals":2},
            "systems":{"water":{"networks":[{"id":"N1","nodes":[
                {"id":"S1","kind":"sprinkler","k_lpm_bar05":80},{"id":"S2","kind":"sprinkler","k_lpm_bar05":80},{"id":"J","kind":"junction"}],
              "links":[{"id":"P1","kind":"pipe","diameter_mm":65,"length_m":30.5,"fittings":[{"type":"elbow_90","count":2}]},
                       {"id":"P2","kind":"pipe","diameter_mm":65,"length_m":10,"fittings":[]}]}]},
              "extinguishers":{"units":[{"id":"E1","agent":"water","ratings":{"A":2}},{"id":"E2","agent":"water","ratings":{"A":2}}]}},
            "results":{"gas":{"G1":{"values":{"agent_mass_kg":123.4}}}},
            "budget":{"currency":"USD","lines":[
               {"id":"B1","bom_key":"device|sprinkler|80","catalog_ref":"SPR-1","description":"Rociadores","qty":2,"unit":"u","category":"dispositivo"},
               {"id":"B2","bom_key":"pipe|65","catalog_ref":"PIPE-65","description":"Tubería","qty":40.5,"unit":"m","category":"tuberia"},
               {"id":"B3","description":"Mano de obra","qty":8,"unit":"h","unit_price":"15.333","currency":"USD","category":"mano_de_obra"},
               {"id":"B4","bom_key":"extinguisher|water|{\"A\":2}","catalog_ref":"EXT-1","description":"Extintores","qty":2,"unit":"u","category":"extintor"}],
              "taxes":[{"name":"IVA demo","rate_pct":10}]}}"#,
        )
        .unwrap()
    }

    #[test]
    fn bom_agrupa_y_suma_cantidades_medidas() {
        let b = bom(&project());
        let get = |k: &str| b.iter().find(|l| l.key == k).unwrap_or_else(|| panic!("falta {}", k));
        assert_eq!(get("pipe|65").qty, 40.5);
        assert_eq!(get("pipe|65").sources, vec!["P1", "P2"]);
        assert_eq!(get("fitting|elbow_90|65").qty, 2.0);
        assert_eq!(get("device|sprinkler|80").qty, 2.0);
        assert_eq!(get("extinguisher|water|{\"A\":2}").qty, 2.0);
        assert_eq!(get("agent|gas|G1").qty, 123.4);
        // orden determinista
        let keys: Vec<&String> = b.iter().map(|l| &l.key).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
    }

    #[test]
    fn presupuesto_exacto_con_trazabilidad_y_sin_precios_inventados() {
        let (cat, _) = catalog_from_csv(CSV);
        let o = compute(&project(), &cat);
        assert_eq!(o.status, Status::Ok, "{:?}", o.issues);
        let v = &o.values;
        // B1: 2*12.50 = 25.00; B2: 40.5*8.20 = 332.10; B3: 8*15.333 = 122.664 -> 122.66; B4 sin precio
        assert_eq!(v.at("lines.0.total").and_then(Json::as_str), Some("25.00"));
        assert_eq!(v.at("lines.1.total").and_then(Json::as_str), Some("332.10"));
        assert_eq!(v.at("lines.2.total").and_then(Json::as_str), Some("122.66"));
        assert!(v.at("lines.3.total").unwrap().is_null());
        // subtotal = 25.00 + 332.10 + 122.66 = 479.76 ; IVA 10 % = 47.976 -> 47.98 ; total 527.74
        assert_eq!(v.field("subtotal").as_str(), Some("479.76"));
        assert_eq!(v.at("taxes.0.amount").and_then(Json::as_str), Some("47.98"));
        assert_eq!(v.field("total").as_str(), Some("527.74"));
        // la línea sin precio NO cuenta como 0 y el total se declara incompleto
        assert_eq!(v.field("incomplete").as_bool(), Some(true));
        assert_eq!(v.field("unpriced_lines").as_f64(), Some(1.0));
        assert!(o.issues.iter().any(|i| i.code == "budget.line_unpriced" && i.object.as_deref() == Some("B4")));
        // trazabilidad: el subtotal lista las líneas que lo componen
        let comps: Vec<&str> = v.field("subtotal_components").items().iter().filter_map(Json::as_str).collect();
        assert_eq!(comps, vec!["B1", "B2", "B3"]);
        // el agente gaseoso medido no está presupuestado => aviso
        assert!(o
            .issues
            .iter()
            .any(|i| i.code == "budget.bom_line_unbudgeted" && i.params.get("key").map(String::as_str) == Some("agent|gas|G1")));
        assert!(o.issues.iter().any(|i| i.code == "budget.demo_item" && i.object.as_deref() == Some("B4")));
    }

    #[test]
    fn errores_de_catalogo_unidad_y_moneda() {
        let (cat, _) = catalog_from_csv(CSV);
        let mut p = project();
        p.set_at("budget.lines.0.catalog_ref", Json::str("NO-EXISTE"));
        let o = compute(&p, &cat);
        assert_eq!(o.status, Status::Blocked);
        assert!(o.issues.iter().any(|i| i.code == "budget.catalog_ref_unknown" && i.object.as_deref() == Some("B1")));
        let mut p = project();
        p.set_at("budget.lines.0.unit", Json::str("m")); // el catálogo dice "u"
        assert!(compute(&p, &cat).issues.iter().any(|i| i.code == "budget.unit_mismatch"));
        let mut p = project();
        p.set_at("budget.lines.2.currency", Json::str("EUR"));
        let o = compute(&p, &cat);
        assert_eq!(o.status, Status::Blocked);
        assert!(o.issues.iter().any(|i| i.code == "budget.currency_mismatch" && i.object.as_deref() == Some("B3")));
        // cantidad ausente
        let mut p = project();
        p.set_at("budget.lines.1.qty", Json::Null);
        assert!(compute(&p, &cat).issues.iter().any(|i| i.code == "data.missing" && i.path.ends_with("qty")));
    }

    #[test]
    fn determinista() {
        let (cat, _) = catalog_from_csv(CSV);
        let a = compute(&project(), &cat);
        let b = compute(&project(), &cat);
        assert_eq!(a.values.canonical(), b.values.canonical());
        assert_eq!(a.fingerprint, b.fingerprint);
    }
}
