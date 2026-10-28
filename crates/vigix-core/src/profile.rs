//! Perfiles normativos (contrato sección 3).
//!
//! Un perfil es un **paquete de datos versionado** (no condicionales dispersos por la
//! interfaz). Este módulo:
//!  * calcula la huella SHA-256 canónica del paquete (`authorship.sha256` se excluye);
//!  * evalúa si el perfil es *verificado* o *no verificado / solo informativo*;
//!  * da acceso a parámetros y tablas **siempre con unidad y referencia**; y
//!  * mantiene el registro (un perfil publicado es inmutable: nueva edición = nueva versión).
//!
//! El software **no** puede dar por aprobado un perfil: la aprobación es un dato escrito por
//! el especialista (`authorship.approval_status = "aprobado"` + `approval_ref`) que el
//! programa solo verifica (integridad, completitud, pruebas), nunca decide (contrato 3.2).

use crate::hash::sha256_hex;
use crate::issues::Issue;
use crate::json::Json;
use crate::timeutil::parse_date;
use crate::units;

pub const SCHEMA: &str = "vigix.profile/1";
pub const FAMILIES: [&str; 4] = ["NFPA", "EN-ISO", "NACIONAL-LOCAL", "OTRA"];
pub const APPROVAL_STATES: [&str; 4] = ["borrador", "revisado", "aprobado", "retirado"];

/// Tipos de sistema que un perfil puede declarar como cubiertos (contrato 2.2 y Anexo B).
pub const SYSTEMS: &[&str] = &[
    "portable_extinguishers",
    "wet_pipe_sprinkler",
    "dry_pipe_sprinkler",
    "preaction_sprinkler",
    "deluge_sprinkler",
    "water_spray",
    "water_mist",
    "standpipe_hose",
    "private_hydrant",
    "fire_pump",
    "water_tank",
    "foam",
    "foam_water",
    "gas_co2",
    "gas_clean_agent",
    "gas_inert",
    "dry_chemical",
    "wet_chemical_kitchen",
    "condensed_aerosol",
    "cause_effect",
];

/// Secciones obligatorias de primer nivel.
const REQUIRED_SECTIONS: &[&str] = &[
    "profile_code",
    "profile_version",
    "family",
    "country",
    "region",
    "authority",
    "language",
    "unit_system",
    "sources",
    "scope",
    "methods",
    "parameters",
    "validity",
    "authorship",
    "test_cases",
    "changelog",
    "compatibility",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Aprobado por especialista, íntegro y completo.
    Verified,
    /// Utilizable solo de modo informativo ("no verificado / solo informativo").
    Unverified,
    /// Estructura inválida: no se puede usar.
    Invalid,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Verified => "verified",
            Level::Unverified => "unverified",
            Level::Invalid => "invalid",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Status {
    pub level: Level,
    pub issues: Vec<Issue>,
    pub computed_sha256: String,
    pub declared_sha256: Option<String>,
}

/// Huella canónica del paquete: JSON compacto con claves ordenadas y **sin**
/// `authorship.sha256` (que es la propia huella).
pub fn compute_hash(pkg: &Json) -> String {
    let mut p = pkg.clone();
    if let Json::Obj(m) = &mut p {
        if let Some(Json::Obj(a)) = m.get_mut("authorship") {
            a.remove("sha256");
        }
    }
    sha256_hex(p.canonical().as_bytes())
}

/// Escribe `authorship.sha256` (la huella) en el paquete. Lo usa la herramienta del especialista.
pub fn seal(pkg: &mut Json) -> String {
    let h = compute_hash(pkg);
    pkg.set_at("authorship.sha256", Json::str(h.as_str()));
    h
}

fn is_leaf(j: &Json) -> bool {
    matches!(j, Json::Obj(m) if m.contains_key("value") || m.contains_key("flag") || m.contains_key("text"))
}

/// Recorre las hojas de `parameters` (ruta, hoja).
fn walk_leaves<'a>(j: &'a Json, path: &str, out: &mut Vec<(String, &'a Json)>) {
    if is_leaf(j) {
        out.push((path.to_string(), j));
        return;
    }
    if let Json::Obj(m) = j {
        for (k, v) in m {
            let p = if path.is_empty() { k.clone() } else { format!("{}.{}", path, k) };
            walk_leaves(v, &p, out);
        }
    }
}

/// Evalúa el paquete (sin ejecutar casos de prueba: eso lo hace `selftest`).
pub fn assess(pkg: &Json) -> Status {
    let computed = compute_hash(pkg);
    let declared = pkg.at("authorship.sha256").and_then(Json::as_str).map(|s| s.to_string());
    let mut issues: Vec<Issue> = Vec::new();
    let mut invalid = false;
    let inv = |issues: &mut Vec<Issue>, code: &str, path: &str| {
        issues.push(Issue::block(code).module("profile").path(path));
    };

    if pkg.field("schema").as_str() != Some(SCHEMA) {
        inv(&mut issues, "profile.bad_schema", "schema");
        invalid = true;
    }
    for s in REQUIRED_SECTIONS {
        if pkg.get(s).map(Json::is_null).unwrap_or(true) {
            issues.push(Issue::block("profile.missing_section").module("profile").path(s).param("section", *s));
            invalid = true;
        }
    }
    if invalid {
        return Status { level: Level::Invalid, issues, computed_sha256: computed, declared_sha256: declared };
    }

    // ---- estructura ----
    if !FAMILIES.contains(&pkg.field("family").as_str().unwrap_or("")) {
        inv(&mut issues, "profile.bad_family", "family");
        invalid = true;
    }
    for k in ["profile_code", "profile_version", "country", "authority", "language", "unit_system"] {
        if pkg.field(k).as_str().map(|s| s.trim().is_empty()).unwrap_or(true) {
            issues.push(Issue::block("profile.missing_section").module("profile").path(k).param("section", k));
            invalid = true;
        }
    }
    if !matches!(pkg.field("unit_system").as_str(), Some("SI") | Some("US")) {
        inv(&mut issues, "profile.bad_unit_system", "unit_system");
        invalid = true;
    }
    // fuentes
    let sources = pkg.field("sources").items();
    if sources.is_empty() {
        inv(&mut issues, "profile.no_sources", "sources");
        invalid = true;
    }
    for (i, s) in sources.iter().enumerate() {
        for k in ["id", "body", "code", "title", "edition", "effective_date"] {
            if s.field(k).as_str().map(|t| t.trim().is_empty()).unwrap_or(true) {
                issues.push(
                    Issue::block("profile.source_incomplete").module("profile").path(&format!("sources.{}.{}", i, k)).param("field", k),
                );
                invalid = true;
            }
        }
        if let Some(d) = s.field("effective_date").as_str() {
            if parse_date(d).is_none() {
                issues.push(
                    Issue::block("profile.bad_date").module("profile").path(&format!("sources.{}.effective_date", i)).param("value", d),
                );
                invalid = true;
            }
        }
    }
    // ámbito
    let systems = pkg.at("scope.systems").map(Json::items).unwrap_or(&[]);
    if systems.is_empty() {
        inv(&mut issues, "profile.empty_scope", "scope.systems");
        invalid = true;
    }
    for (i, s) in systems.iter().enumerate() {
        match s.as_str() {
            Some(n) if SYSTEMS.contains(&n) => {}
            _ => {
                issues.push(
                    Issue::block("profile.unknown_system")
                        .module("profile")
                        .path(&format!("scope.systems.{}", i))
                        .param("system", s.as_str().unwrap_or("?")),
                );
                invalid = true;
            }
        }
    }
    if pkg.at("scope.exclusions").is_none() {
        inv(&mut issues, "profile.missing_section", "scope.exclusions");
        invalid = true;
    }
    // vigencia
    match pkg.at("validity.effective_date").and_then(Json::as_str).and_then(parse_date) {
        Some(eff) => {
            if let Some(exp) = pkg.at("validity.expires").and_then(Json::as_str) {
                match parse_date(exp) {
                    Some(e) if e >= eff => {}
                    _ => {
                        inv(&mut issues, "profile.bad_date", "validity.expires");
                        invalid = true;
                    }
                }
            }
        }
        None => {
            inv(&mut issues, "profile.bad_date", "validity.effective_date");
            invalid = true;
        }
    }
    // autoría
    let status = pkg.at("authorship.approval_status").and_then(Json::as_str).unwrap_or("");
    if !APPROVAL_STATES.contains(&status) {
        inv(&mut issues, "profile.bad_approval_status", "authorship.approval_status");
        invalid = true;
    }
    if invalid {
        return Status { level: Level::Invalid, issues, computed_sha256: computed, declared_sha256: declared };
    }

    // ---- verificación (todo lo que sigue solo degrada a "no verificado") ----
    let mut unverified = false;
    let flag = |issues: &mut Vec<Issue>, i: Issue| {
        issues.push(i);
    };

    if pkg.field("demo").as_bool() == Some(true) {
        flag(&mut issues, Issue::warning("profile.demo").module("profile").path("demo"));
        unverified = true;
    }
    if status != "aprobado" {
        let sev = if status == "retirado" { Issue::error("profile.withdrawn") } else { Issue::warning("profile.not_approved") };
        flag(&mut issues, sev.module("profile").path("authorship.approval_status").param("status", status));
        unverified = true;
    }
    for k in ["author", "reviewer", "date"] {
        if pkg.at(&format!("authorship.{}", k)).and_then(Json::as_str).map(|s| s.trim().is_empty()).unwrap_or(true) {
            flag(
                &mut issues,
                Issue::warning("profile.authorship_incomplete").module("profile").path(&format!("authorship.{}", k)).param("field", k),
            );
            unverified = true;
        }
    }
    if status == "aprobado" && pkg.at("authorship.approval_ref").and_then(Json::as_str).map(|s| s.trim().is_empty()).unwrap_or(true) {
        flag(&mut issues, Issue::error("profile.approval_ref_missing").module("profile").path("authorship.approval_ref"));
        unverified = true;
    }
    match &declared {
        None => {
            flag(&mut issues, Issue::warning("profile.hash_missing").module("profile").path("authorship.sha256"));
            unverified = true;
        }
        Some(d) if d != &computed => {
            flag(
                &mut issues,
                Issue::error("profile.hash_mismatch")
                    .module("profile")
                    .path("authorship.sha256")
                    .param("declared", d.as_str())
                    .param("computed", computed.as_str()),
            );
            unverified = true;
        }
        _ => {}
    }
    // Todo parámetro y fila de tabla debe tener referencia e unidad.
    let mut leaves = Vec::new();
    walk_leaves(pkg.field("parameters"), "", &mut leaves);
    for (path, leaf) in &leaves {
        if leaf.field("ref").as_str().map(|s| s.trim().is_empty()).unwrap_or(true) {
            flag(&mut issues, Issue::warning("profile.ref_missing").module("profile").path(&format!("parameters.{}", path)));
            unverified = true;
        }
        if leaf.get("value").is_some() {
            match leaf.field("unit").as_str() {
                Some(u) if units::table().is_unit(u) => {}
                other => {
                    flag(
                        &mut issues,
                        Issue::error("profile.unit_invalid")
                            .module("profile")
                            .path(&format!("parameters.{}.unit", path))
                            .param("unit", other.unwrap_or("")),
                    );
                    unverified = true;
                }
            }
        }
    }
    if let Some(tables) = pkg.field("tables").as_obj() {
        for (tname, t) in tables {
            if let Some(u) = t.field("value_unit").as_str() {
                if !units::table().is_unit(u) {
                    flag(
                        &mut issues,
                        Issue::error("profile.unit_invalid")
                            .module("profile")
                            .path(&format!("tables.{}.value_unit", tname))
                            .param("unit", u),
                    );
                    unverified = true;
                }
            }
            for (ri, row) in t.field("rows").items().iter().enumerate() {
                if row.field("ref").as_str().map(|s| s.trim().is_empty()).unwrap_or(true) {
                    flag(
                        &mut issues,
                        Issue::warning("profile.ref_missing").module("profile").path(&format!("tables.{}.rows.{}", tname, ri)),
                    );
                    unverified = true;
                }
            }
        }
    }
    // Casos de prueba independientes por sistema cubierto.
    let cases = pkg.field("test_cases").items();
    for s in systems {
        let sys = s.as_str().unwrap_or("");
        if !cases.iter().any(|c| c.field("system").as_str() == Some(sys)) {
            flag(&mut issues, Issue::warning("profile.no_test_cases").module("profile").path("test_cases").param("system", sys));
            unverified = true;
        }
    }
    // Métodos declarados y conocidos.
    if let Some(ms) = pkg.field("methods").as_obj() {
        for (k, v) in ms {
            if v.as_str().map(|s| s.trim().is_empty()).unwrap_or(true) {
                flag(&mut issues, Issue::warning("profile.method_empty").module("profile").path(&format!("methods.{}", k)));
                unverified = true;
            }
        }
    }

    Status {
        level: if unverified { Level::Unverified } else { Level::Verified },
        issues,
        computed_sha256: computed,
        declared_sha256: declared,
    }
}

/// Un parámetro numérico con su unidad y referencia (siempre trazable).
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub path: String,
    pub value: f64,
    pub unit: String,
    pub reference: String,
}

impl Param {
    pub fn to_json(&self) -> Json {
        Json::obj()
            .with("path", Json::str(self.path.as_str()))
            .with("value", Json::num(self.value))
            .with("unit", Json::str(self.unit.as_str()))
            .with("ref", Json::str(self.reference.as_str()))
    }
}

/// Perfil cargado: paquete + estado.
#[derive(Clone, Debug)]
pub struct Profile {
    pub raw: Json,
    pub level: Level,
    pub issues: Vec<Issue>,
    pub sha256: String,
}

impl Profile {
    pub fn from_json(pkg: Json) -> Result<Profile, Vec<Issue>> {
        let st = assess(&pkg);
        if st.level == Level::Invalid {
            return Err(st.issues);
        }
        Ok(Profile { raw: pkg, level: st.level, issues: st.issues, sha256: st.computed_sha256 })
    }

    pub fn code(&self) -> &str {
        self.raw.field("profile_code").as_str().unwrap_or("")
    }
    pub fn version(&self) -> &str {
        self.raw.field("profile_version").as_str().unwrap_or("")
    }
    pub fn is_demo(&self) -> bool {
        self.raw.field("demo").as_bool() == Some(true)
    }
    pub fn is_verified(&self) -> bool {
        self.level == Level::Verified
    }
    pub fn unit_system(&self) -> &str {
        self.raw.field("unit_system").as_str().unwrap_or("SI")
    }

    pub fn covers(&self, system: &str) -> bool {
        self.raw.at("scope.systems").map(Json::items).unwrap_or(&[]).iter().any(|s| s.as_str() == Some(system))
    }

    pub fn method(&self, key: &str) -> Option<&str> {
        self.raw.at("methods").and_then(|m| m.get(key)).and_then(Json::as_str).filter(|s| !s.trim().is_empty())
    }

    /// `validity`: devuelve la incidencia que corresponda frente a la **fecha de diseño** del
    /// proyecto (dato de entrada, no el reloj del equipo: contrato 8.3).
    pub fn validity_issue(&self, design_date_days: i64) -> Option<Issue> {
        let eff = self.raw.at("validity.effective_date").and_then(Json::as_str).and_then(parse_date)?;
        if design_date_days < eff {
            return Some(Issue::block("profile.not_yet_effective").module("profile").path("validity.effective_date"));
        }
        if let Some(exp) = self.raw.at("validity.expires").and_then(Json::as_str).and_then(parse_date) {
            if design_date_days > exp {
                return Some(Issue::block("profile.expired").module("profile").path("validity.expires"));
            }
        }
        None
    }

    fn leaf(&self, path: &str) -> Result<&Json, Issue> {
        let full = format!("parameters.{}", path);
        match self.raw.at(&full) {
            Some(j) if is_leaf(j) => Ok(j),
            _ => Err(Issue::block("profile.param_missing").module("profile").path(&full).param("param", path)),
        }
    }

    /// Parámetro numérico en la unidad del perfil.
    pub fn param(&self, path: &str) -> Result<Param, Issue> {
        let leaf = self.leaf(path)?;
        let full = format!("parameters.{}", path);
        let value = leaf
            .field("value")
            .as_f64()
            .filter(|v| v.is_finite())
            .ok_or_else(|| Issue::block("profile.param_missing").module("profile").path(&full).param("param", path))?;
        let unit = leaf.field("unit").as_str().unwrap_or("").to_string();
        let reference = leaf.field("ref").as_str().unwrap_or("").to_string();
        Ok(Param { path: full, value, unit, reference })
    }

    /// Parámetro convertido a la unidad pedida por el motor. Unidad incompatible => bloqueo.
    pub fn param_in(&self, path: &str, want_unit: &str) -> Result<Param, Issue> {
        let p = self.param(path)?;
        match units::convert(p.value, &p.unit, want_unit) {
            Ok(v) => Ok(Param { value: v, unit: want_unit.to_string(), ..p }),
            Err(_) => Err(Issue::block("profile.unit_mismatch")
                .module("profile")
                .path(&p.path)
                .param("have", p.unit.as_str())
                .param("want", want_unit)
                .reference(&p.reference)),
        }
    }

    pub fn flag(&self, path: &str) -> Result<(bool, String), Issue> {
        let leaf = self.leaf(path)?;
        match leaf.field("flag").as_bool() {
            Some(b) => Ok((b, leaf.field("ref").as_str().unwrap_or("").to_string())),
            None => Err(Issue::block("profile.param_missing").module("profile").path(&format!("parameters.{}", path)).param("param", path)),
        }
    }

    pub fn text(&self, path: &str) -> Result<(String, String), Issue> {
        let leaf = self.leaf(path)?;
        match leaf.field("text").as_str() {
            Some(t) => Ok((t.to_string(), leaf.field("ref").as_str().unwrap_or("").to_string())),
            None => Err(Issue::block("profile.param_missing").module("profile").path(&format!("parameters.{}", path)).param("param", path)),
        }
    }

    fn table(&self, name: &str) -> Result<&Json, Issue> {
        match self.raw.at("tables").and_then(|t| t.get(name)) {
            Some(t) if t.field("rows").as_arr().is_some() => Ok(t),
            _ => Err(Issue::block("profile.table_missing").module("profile").path(&format!("tables.{}", name)).param("table", name)),
        }
    }

    fn row_param(&self, tname: &str, ti: usize, table: &Json, row: &Json, rowi: usize) -> Result<Param, Issue> {
        let _ = ti;
        let path = format!("tables.{}.rows.{}", tname, rowi);
        let value = row
            .field("value")
            .as_f64()
            .filter(|v| v.is_finite())
            .ok_or_else(|| Issue::block("profile.param_missing").module("profile").path(&path).param("param", tname))?;
        let unit = row.field("unit").as_str().or_else(|| table.field("value_unit").as_str()).unwrap_or("-").to_string();
        let reference = row.field("ref").as_str().unwrap_or("").to_string();
        Ok(Param { path, value, unit, reference })
    }

    /// Busca la fila cuya(s) clave(s) coinciden exactamente. Sin fila => bloqueo explícito
    /// ("valor fuera de tabla"); varias filas => ambigüedad (bloqueo).
    pub fn lookup(&self, table: &str, keys: &[(&str, &str)]) -> Result<Param, Issue> {
        let t = self.table(table)?;
        let rows = t.field("rows").items();
        let mut hit: Option<usize> = None;
        for (i, r) in rows.iter().enumerate() {
            if keys.iter().all(|(k, v)| r.field(k).as_str() == Some(*v)) {
                if hit.is_some() {
                    return Err(Issue::block("profile.table_ambiguous")
                        .module("profile")
                        .path(&format!("tables.{}", table))
                        .param("table", table));
                }
                hit = Some(i);
            }
        }
        match hit {
            Some(i) => self.row_param(table, 0, t, &rows[i], i),
            None => {
                let mut is =
                    Issue::block("profile.table_no_row").module("profile").path(&format!("tables.{}", table)).param("table", table);
                for (k, v) in keys {
                    is = is.param(k, *v);
                }
                Err(is)
            }
        }
    }

    pub fn lookup_in(&self, table: &str, keys: &[(&str, &str)], want_unit: &str) -> Result<Param, Issue> {
        let p = self.lookup(table, keys)?;
        match units::convert(p.value, &p.unit, want_unit) {
            Ok(v) => Ok(Param { value: v, unit: want_unit.to_string(), ..p }),
            Err(_) => Err(Issue::block("profile.unit_mismatch")
                .module("profile")
                .path(&p.path)
                .param("have", p.unit.as_str())
                .param("want", want_unit)
                .reference(&p.reference)),
        }
    }

    /// Fila por intervalo semiabierto `from <= x < to` (el último intervalo puede declarar
    /// `"to_inclusive": true`). Fuera de todos los intervalos => bloqueo (p. ej. altitud o
    /// temperatura "no soportada", contrato 11.2).
    pub fn lookup_range(&self, table: &str, keys: &[(&str, &str)], x: f64) -> Result<Param, Issue> {
        let t = self.table(table)?;
        let rows = t.field("rows").items();
        let mut hit: Option<usize> = None;
        let mut key_rows = 0usize;
        for (i, r) in rows.iter().enumerate() {
            if !keys.iter().all(|(k, v)| r.field(k).as_str() == Some(*v)) {
                continue;
            }
            key_rows += 1;
            let (Some(lo), Some(hi)) = (r.field("from").as_f64(), r.field("to").as_f64()) else { continue };
            let inclusive = r.field("to_inclusive").as_bool() == Some(true);
            if x >= lo && (x < hi || (inclusive && x <= hi)) {
                if hit.is_some() {
                    return Err(Issue::block("profile.table_ambiguous")
                        .module("profile")
                        .path(&format!("tables.{}", table))
                        .param("table", table));
                }
                hit = Some(i);
            }
        }
        match hit {
            Some(i) => self.row_param(table, 0, t, &rows[i], i),
            None if key_rows == 0 => {
                let mut is =
                    Issue::block("profile.table_no_row").module("profile").path(&format!("tables.{}", table)).param("table", table);
                for (k, v) in keys {
                    is = is.param(k, *v);
                }
                Err(is)
            }
            None => Err(Issue::block("profile.out_of_table_range")
                .module("profile")
                .path(&format!("tables.{}", table))
                .param("table", table)
                .param("x", crate::json::fmt_num(x))),
        }
    }

    pub fn lookup_range_in(&self, table: &str, keys: &[(&str, &str)], x: f64, want_unit: &str) -> Result<Param, Issue> {
        let p = self.lookup_range(table, keys, x)?;
        match units::convert(p.value, &p.unit, want_unit) {
            Ok(v) => Ok(Param { value: v, unit: want_unit.to_string(), ..p }),
            Err(_) => Err(Issue::block("profile.unit_mismatch")
                .module("profile")
                .path(&p.path)
                .param("have", p.unit.as_str())
                .param("want", want_unit)
                .reference(&p.reference)),
        }
    }

    /// Interpolación lineal en una curva `rows: [{x, value}]` **sin extrapolar**.
    pub fn interpolate(&self, table: &str, x: f64) -> Result<Param, Issue> {
        let t = self.table(table)?;
        let mut pts: Vec<(f64, f64, usize)> = Vec::new();
        for (i, r) in t.field("rows").items().iter().enumerate() {
            if let (Some(px), Some(py)) = (r.field("x").as_f64(), r.field("value").as_f64()) {
                pts.push((px, py, i));
            }
        }
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let oor = || {
            Issue::block("profile.out_of_table_range")
                .module("profile")
                .path(&format!("tables.{}", table))
                .param("table", table)
                .param("x", crate::json::fmt_num(x))
        };
        if pts.len() < 2 || !x.is_finite() || x < pts[0].0 || x > pts[pts.len() - 1].0 {
            return Err(oor());
        }
        let mut seg = pts.len() - 2;
        for k in 0..pts.len() - 1 {
            if x <= pts[k + 1].0 {
                seg = k;
                break;
            }
        }
        let (x0, y0, i0) = pts[seg];
        let (x1, y1, _) = pts[seg + 1];
        let y = if x1 == x0 { y0 } else { y0 + (y1 - y0) * (x - x0) / (x1 - x0) };
        let row = &t.field("rows").items()[i0];
        let unit = row.field("unit").as_str().or_else(|| t.field("value_unit").as_str()).unwrap_or("-").to_string();
        let reference = row.field("ref").as_str().unwrap_or("").to_string();
        Ok(Param { path: format!("tables.{}", table), value: y, unit, reference })
    }

    pub fn summary_json(&self) -> Json {
        Json::obj()
            .with("code", Json::str(self.code()))
            .with("version", Json::str(self.version()))
            .with("sha256", Json::str(self.sha256.as_str()))
            .with("level", Json::str(self.level.as_str()))
            .with("demo", Json::Bool(self.is_demo()))
            .with("family", self.raw.field("family").clone())
            .with("country", self.raw.field("country").clone())
            .with("region", self.raw.field("region").clone())
            .with("authority", self.raw.field("authority").clone())
            .with("title", self.raw.field("title").clone())
            .with("language", self.raw.field("language").clone())
            .with("unit_system", self.raw.field("unit_system").clone())
            .with("systems", self.raw.at("scope.systems").cloned().unwrap_or(Json::arr()))
            .with("approval_status", self.raw.at("authorship.approval_status").cloned().unwrap_or(Json::Null))
            .with("effective_date", self.raw.at("validity.effective_date").cloned().unwrap_or(Json::Null))
            .with("expires", self.raw.at("validity.expires").cloned().unwrap_or(Json::Null))
            .with("issues", Json::Arr(self.issues.iter().map(Issue::to_json).collect()))
    }
}

/// Registro de perfiles. Un par (código, versión) es inmutable.
#[derive(Default)]
pub struct Registry {
    profiles: Vec<Profile>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry { profiles: Vec::new() }
    }

    /// Añade un paquete. Mismo (código, versión) con otra huella => rechazo (contrato 3.2:
    /// no se alteran silenciosamente resultados históricos). Misma huella => idempotente.
    pub fn add(&mut self, pkg: Json) -> Result<&Profile, Vec<Issue>> {
        let p = Profile::from_json(pkg)?;
        let (code, version, sha) = (p.code().to_string(), p.version().to_string(), p.sha256.clone());
        if let Some(pos) = self.profiles.iter().position(|q| q.code() == code && q.version() == version) {
            if self.profiles[pos].sha256 == sha {
                return Ok(&self.profiles[pos]);
            }
            return Err(vec![Issue::block("profile.version_conflict")
                .module("profile")
                .param("code", code.as_str())
                .param("version", version.as_str())]);
        }
        self.profiles.push(p);
        self.profiles.sort_by(|a, b| (a.code(), a.version()).cmp(&(b.code(), b.version())));
        self.profiles
            .iter()
            .find(|q| q.code() == code && q.version() == version)
            .ok_or_else(|| vec![Issue::block("profile.internal").module("profile")])
    }

    pub fn get(&self, code: &str, version: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.code() == code && p.version() == version)
    }

    /// Degrada un perfil a "no verificado" añadiendo el motivo (p. ej. fallo del autotest).
    pub fn downgrade(&mut self, code: &str, version: &str, reason: Issue) {
        if let Some(p) = self.profiles.iter_mut().find(|p| p.code() == code && p.version() == version) {
            if p.level == Level::Verified {
                p.level = Level::Unverified;
            }
            p.issues.push(reason);
        }
    }

    pub fn list(&self) -> &[Profile] {
        &self.profiles
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn mini_pkg() -> Json {
        Json::parse(
            r#"{
          "schema":"vigix.profile/1","profile_code":"TEST-1","profile_version":"1.0.0","family":"OTRA",
          "country":"XX","region":"-","authority":"Autoridad de prueba","language":"es","unit_system":"SI",
          "demo":true,
          "title":{"es":"Prueba","en":"Test"},
          "sources":[{"id":"S1","body":"ORG","code":"COD 1","title":"Titulo","edition":"2020","effective_date":"2020-01-01","amendments":[]}],
          "scope":{"systems":["foam"],"exclusions":["nada"]},
          "methods":{"foam.surface":"foam.surface.v1"},
          "parameters":{"foam":{"duration":{"value":15,"unit":"min","ref":"S1:T1"},"flag_x":{"flag":true,"ref":"S1:T2"},"nombre":{"text":"abc","ref":"S1:T3"}}},
          "tables":{
            "rate":{"keys":["hazard"],"value_unit":"L/min/m2","rows":[
              {"hazard":"low","value":4,"ref":"S1:T4"},{"hazard":"high","value":8,"ref":"S1:T4"}]},
            "range":{"keys":["cls"],"value_unit":"-","rows":[
              {"cls":"a","from":0,"to":10,"value":1,"ref":"S1:T5"},
              {"cls":"a","from":10,"to":20,"to_inclusive":true,"value":2,"ref":"S1:T5"}]},
            "curve":{"value_unit":"bar","rows":[{"x":0,"value":10,"ref":"S1:T6"},{"x":10,"value":0,"ref":"S1:T6"}]}
          },
          "validity":{"effective_date":"2020-06-01","expires":"2030-12-31"},
          "authorship":{"author":"A","reviewer":"R","date":"2026-10-28","approval_status":"borrador","approval_ref":"","sha256":""},
          "test_cases":[{"id":"C1","system":"foam","input":{},"expected":{},"tolerance_rel":0.01}],
          "changelog":[{"version":"1.0.0","date":"2026-10-28","change":"inicial"}],
          "compatibility":{"supersedes":null,"min_engine":"0.1.0","notes":""}
        }"#,
        )
        .unwrap()
    }

    #[test]
    fn huella_excluye_su_propio_campo_y_es_estable() {
        let mut p = mini_pkg();
        let h1 = compute_hash(&p);
        p.set_at("authorship.sha256", Json::str("lo-que-sea"));
        assert_eq!(compute_hash(&p), h1);
        // cualquier cambio altera la huella
        p.set_at("parameters.foam.duration.value", Json::num(16.0));
        assert_ne!(compute_hash(&p), h1);
        // el orden de claves en el texto de origen no importa
        let a = Json::parse(r#"{"b":1,"a":2}"#).unwrap();
        let b = Json::parse(r#"{"a":2,"b":1}"#).unwrap();
        assert_eq!(a.canonical(), b.canonical());
    }

    #[test]
    fn demo_nunca_es_verificado_aunque_este_aprobado_y_sellado() {
        let mut p = mini_pkg();
        p.set_at("authorship.approval_status", Json::str("aprobado"));
        p.set_at("authorship.approval_ref", Json::str("ACTA-1"));
        seal(&mut p);
        let st = assess(&p);
        assert_eq!(st.level, Level::Unverified);
        assert!(st.issues.iter().any(|i| i.code == "profile.demo"));
        // sin la marca demo y con casos para todos los sistemas => verificado
        p.set_at("demo", Json::Bool(false));
        seal(&mut p);
        let st = assess(&p);
        assert_eq!(st.level, Level::Verified, "{:?}", st.issues);
    }

    #[test]
    fn borrador_o_hash_alterado_es_no_verificado() {
        let mut p = mini_pkg();
        p.set_at("demo", Json::Bool(false));
        seal(&mut p);
        let st = assess(&p);
        assert_eq!(st.level, Level::Unverified); // borrador
        assert!(st.issues.iter().any(|i| i.code == "profile.not_approved"));
        p.set_at("authorship.approval_status", Json::str("aprobado"));
        p.set_at("authorship.approval_ref", Json::str("ACTA"));
        seal(&mut p);
        assert_eq!(assess(&p).level, Level::Verified);
        // alteración posterior sin resellar
        p.set_at("parameters.foam.duration.value", Json::num(99.0));
        let st = assess(&p);
        assert_eq!(st.level, Level::Unverified);
        assert!(st.issues.iter().any(|i| i.code == "profile.hash_mismatch"));
    }

    #[test]
    fn retirado_y_sin_referencias() {
        let mut p = mini_pkg();
        p.set_at("demo", Json::Bool(false));
        p.set_at("authorship.approval_status", Json::str("retirado"));
        p.set_at("parameters.foam.duration.ref", Json::str(""));
        seal(&mut p);
        let st = assess(&p);
        assert_eq!(st.level, Level::Unverified);
        let codes: Vec<&str> = st.issues.iter().map(|i| i.code.as_str()).collect();
        assert!(codes.contains(&"profile.withdrawn"));
        assert!(codes.contains(&"profile.ref_missing"));
    }

    #[test]
    fn estructura_invalida() {
        let mut p = mini_pkg();
        if let Json::Obj(m) = &mut p {
            m.remove("sources");
        }
        assert_eq!(assess(&p).level, Level::Invalid);
        let mut p = mini_pkg();
        p.set_at("schema", Json::str("otra/9"));
        assert_eq!(assess(&p).level, Level::Invalid);
        let mut p = mini_pkg();
        p.set_at("scope.systems", Json::Arr(vec![Json::str("sistema_inexistente")]));
        let st = assess(&p);
        assert_eq!(st.level, Level::Invalid);
        assert!(st.issues.iter().any(|i| i.code == "profile.unknown_system"));
        let mut p = mini_pkg();
        p.set_at("sources.0.effective_date", Json::str("2020-13-45"));
        assert_eq!(assess(&p).level, Level::Invalid);
        assert!(Profile::from_json(Json::obj()).is_err());
    }

    #[test]
    fn parametros_con_unidad_y_referencia() {
        let p = Profile::from_json(mini_pkg()).unwrap();
        let d = p.param("foam.duration").unwrap();
        assert_eq!((d.value, d.unit.as_str(), d.reference.as_str()), (15.0, "min", "S1:T1"));
        let d = p.param_in("foam.duration", "s").unwrap();
        assert_eq!((d.value, d.unit.as_str()), (900.0, "s"));
        // unidad incompatible => bloqueo, no cero
        let e = p.param_in("foam.duration", "bar").unwrap_err();
        assert_eq!(e.code, "profile.unit_mismatch");
        // parámetro ausente => bloqueo
        assert_eq!(p.param("foam.no_existe").unwrap_err().code, "profile.param_missing");
        assert!(p.flag("foam.flag_x").unwrap().0);
        assert_eq!(p.text("foam.nombre").unwrap().0, "abc");
        // una hoja de otro tipo no se lee como número
        assert!(p.param("foam.flag_x").is_err());
    }

    #[test]
    fn tablas_exactas_rangos_e_interpolacion() {
        let p = Profile::from_json(mini_pkg()).unwrap();
        let r = p.lookup("rate", &[("hazard", "high")]).unwrap();
        assert_eq!((r.value, r.unit.as_str(), r.reference.as_str()), (8.0, "L/min/m2", "S1:T4"));
        assert_eq!(p.lookup("rate", &[("hazard", "nope")]).unwrap_err().code, "profile.table_no_row");
        assert_eq!(p.lookup("nope", &[]).unwrap_err().code, "profile.table_missing");
        // rangos semiabiertos con último inclusivo
        assert_eq!(p.lookup_range("range", &[("cls", "a")], 5.0).unwrap().value, 1.0);
        assert_eq!(p.lookup_range("range", &[("cls", "a")], 10.0).unwrap().value, 2.0);
        assert_eq!(p.lookup_range("range", &[("cls", "a")], 20.0).unwrap().value, 2.0);
        assert_eq!(p.lookup_range("range", &[("cls", "a")], 20.1).unwrap_err().code, "profile.out_of_table_range");
        assert_eq!(p.lookup_range("range", &[("cls", "a")], -1.0).unwrap_err().code, "profile.out_of_table_range");
        // interpolación sin extrapolar
        assert!((p.interpolate("curve", 2.5).unwrap().value - 7.5).abs() < 1e-12);
        assert_eq!(p.interpolate("curve", 10.0).unwrap().value, 0.0);
        assert_eq!(p.interpolate("curve", 10.01).unwrap_err().code, "profile.out_of_table_range");
        assert_eq!(p.interpolate("curve", -0.01).unwrap_err().code, "profile.out_of_table_range");
    }

    #[test]
    fn vigencia_contra_fecha_de_diseno() {
        let p = Profile::from_json(mini_pkg()).unwrap();
        let d = |s: &str| parse_date(s).unwrap();
        assert!(p.validity_issue(d("2025-01-01")).is_none()); // dentro de [2020-06-01, 2030-12-31]
        assert_eq!(p.validity_issue(d("2020-05-31")).unwrap().code, "profile.not_yet_effective");
        assert!(p.validity_issue(d("2020-06-01")).is_none());
        assert_eq!(p.validity_issue(d("2031-01-01")).unwrap().code, "profile.expired");
    }

    #[test]
    fn registro_inmutable() {
        let mut reg = Registry::new();
        let mut a = mini_pkg();
        seal(&mut a);
        assert!(reg.add(a.clone()).is_ok());
        assert!(reg.add(a.clone()).is_ok()); // idempotente
        assert_eq!(reg.list().len(), 1);
        let mut b = a.clone();
        b.set_at("parameters.foam.duration.value", Json::num(20.0));
        seal(&mut b);
        let err = reg.add(b.clone()).unwrap_err();
        assert_eq!(err[0].code, "profile.version_conflict");
        // nueva versión => se conserva la anterior
        b.set_at("profile_version", Json::str("1.1.0"));
        seal(&mut b);
        assert!(reg.add(b).is_ok());
        assert_eq!(reg.list().len(), 2);
        assert_eq!(reg.get("TEST-1", "1.0.0").unwrap().param("foam.duration").unwrap().value, 15.0);
        assert_eq!(reg.get("TEST-1", "1.1.0").unwrap().param("foam.duration").unwrap().value, 20.0);
    }
}
