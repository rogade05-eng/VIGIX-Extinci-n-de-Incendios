//! Exportación local: CSV, XLSX, DXF, PDF, JSON e IFC (contrato 5.12 y 9.2).
//!
//! Reglas: ninguna exportación sale sin el estado del documento, el perfil y su verificación, la
//! huella de datos, la versión del motor, el usuario/rol y la fecha del equipo (no certificada).
//! Lo omitido o simplificado se **informa** en `notes` y en el manifiesto.

pub mod dxf;
pub mod ifc;
pub mod pdf;
pub mod report;
pub mod tables;
pub mod xlsx;
pub mod zip;

use crate::csv;
use crate::gates::Gate;
use crate::geometry::Pt;
use crate::hash::sha256_hex;
use crate::i18n::catalog;
use crate::json::Json;
use crate::license;
use crate::profile::Profile;
use std::collections::BTreeMap;

pub struct ExportContext<'a> {
    pub project: &'a Json,
    pub profile: Option<&'a Profile>,
    pub gate: &'a Gate,
    pub lang: &'a str,
    pub user: &'a str,
    pub role: &'a str,
    pub license: &'a license::Status,
    /// Fecha/hora ISO del equipo (informativa).
    pub generated_at: &'a str,
    pub audit_digest: Option<&'a str>,
}

pub struct ExportFile {
    pub filename: String,
    pub mime: &'static str,
    pub bytes: Vec<u8>,
    /// Notas estructuradas `{code, count|detail}` sobre contenido omitido o simplificado.
    pub notes: Vec<Json>,
    pub sha256: String,
}

fn note(code: &str, key: &str, val: Json) -> Json {
    Json::obj().with("code", Json::str(code)).with(key, val)
}

fn finish(ctx: &ExportContext, kind: &str, ext: &str, mime: &'static str, bytes: Vec<u8>, notes: Vec<Json>) -> ExportFile {
    let id: String = ctx
        .project
        .at("meta.id")
        .and_then(Json::as_str)
        .unwrap_or("proyecto")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    let mark = match ctx.gate.status {
        crate::gates::DocStatus::Draft => "_BORRADOR",
        crate::gates::DocStatus::ForReview => "_REVISION",
        crate::gates::DocStatus::PermitPackage => "",
    };
    let sha256 = sha256_hex(&bytes);
    ExportFile {
        filename: format!("{}_{}{}.{}", if id.is_empty() { "proyecto" } else { &id }, kind, mark, ext),
        mime,
        bytes,
        notes,
        sha256,
    }
}

fn sys_decimals(ctx: &ExportContext) -> (String, u32) {
    (
        ctx.project.at("display.unit_system").and_then(Json::as_str).unwrap_or("SI").to_string(),
        ctx.project.at("display.decimals").and_then(Json::as_f64).filter(|d| (0.0..=6.0).contains(d)).unwrap_or(2.0) as u32,
    )
}

/// Metadatos comunes para el manifiesto y el JSON.
pub fn document_meta(ctx: &ExportContext) -> Json {
    Json::obj()
        .with("product", Json::str(crate::PRODUCT_NAME))
        .with("engine_version", Json::str(crate::ENGINE_VERSION))
        .with("generated_at", Json::str(ctx.generated_at))
        .with("clock_note", Json::str("fecha del equipo, no certificada / computer clock, not certified"))
        .with("user", Json::str(ctx.user))
        .with("role", Json::str(ctx.role))
        .with("language", Json::str(ctx.lang))
        .with("unit_system", ctx.project.at("display.unit_system").cloned().unwrap_or(Json::Null))
        .with("document_status", Json::str(ctx.gate.status.as_str()))
        .with("document_status_text", Json::str(catalog().tr(ctx.lang, &format!("status.{}", ctx.gate.status.as_str()))))
        .with("project_id", ctx.project.at("meta.id").cloned().unwrap_or(Json::Null))
        .with("project_name", ctx.project.at("meta.name").cloned().unwrap_or(Json::Null))
        .with("inputs_fingerprint", Json::str(crate::project::inputs_fingerprint(ctx.project)))
        .with("audit_digest", Json::opt_str(ctx.audit_digest))
        .with("license", ctx.license.to_json())
        .with(
            "profile",
            match ctx.profile {
                Some(p) => p.summary_json(),
                None => Json::Null,
            },
        )
}

/// JSON autocontenido: proyecto, perfil completo (para reproducir), estado y metadatos.
pub fn export_json(ctx: &ExportContext) -> ExportFile {
    let doc = Json::obj()
        .with("format", Json::str("vigix.export/1"))
        .with("meta", document_meta(ctx))
        .with("document_status", ctx.gate.to_json())
        .with("profile_package", ctx.profile.map(|p| p.raw.clone()).unwrap_or(Json::Null))
        .with("project", ctx.project.clone());
    finish(ctx, "datos", "json", "application/json", doc.to_pretty().into_bytes(), vec![])
}

/// Paquete ZIP con un CSV por tabla, LEEME y MANIFEST.json (huella SHA-256 de cada archivo).
pub fn export_csv_zip(ctx: &ExportContext) -> ExportFile {
    let mut z = zip::ZipWriter::new();
    let mut manifest_files = Vec::new();
    let (system, decimals) = sys_decimals(ctx);
    let _ = (system, decimals);
    for t in tables::all_tables(ctx) {
        let mut rows: Vec<Vec<String>> = vec![t.headers(ctx.lang, "SI", true)];
        rows.extend(t.text_rows("SI", true, 6, '.'));
        let text = format!("\u{feff}{}", csv::write(&rows, ',', true));
        let name = format!("{}.csv", t.id);
        manifest_files.push((name.clone(), text.len(), sha256_hex(text.as_bytes())));
        z.add(&name, text.as_bytes());
    }
    let readme = format!(
        "{}\r\n\r\n{}\r\n{}\r\n",
        catalog().tr(ctx.lang, "export.readme"),
        catalog().tr(ctx.lang, &format!("status.{}", ctx.gate.status.as_str())),
        catalog().tr(ctx.lang, "app.disclaimer")
    );
    manifest_files.push(("LEEME.txt".to_string(), readme.len(), sha256_hex(readme.as_bytes())));
    z.add("LEEME.txt", readme.as_bytes());
    let notes = vec![note("csv_formula_guard", "count", Json::num(0.0))];
    let mut m = document_meta(ctx);
    m.set(
        "files",
        Json::Arr(
            manifest_files
                .iter()
                .map(|(n, l, h)| {
                    Json::obj()
                        .with("name", Json::str(n.as_str()))
                        .with("bytes", Json::num(*l as f64))
                        .with("sha256", Json::str(h.as_str()))
                })
                .collect(),
        ),
    );
    m.set("notes", Json::Arr(notes.clone()));
    z.add("MANIFEST.json", m.to_pretty().as_bytes());
    finish(ctx, "csv", "zip", "application/zip", z.finish(), notes)
}

pub fn export_xlsx(ctx: &ExportContext) -> ExportFile {
    let sheets: Vec<xlsx::Sheet> = tables::all_tables(ctx).iter().map(|t| t.to_sheet(ctx.lang)).collect();
    let (bytes, removed) = xlsx::build(&sheets);
    let notes = if removed > 0 { vec![note("xlsx_chars_removed", "count", Json::num(removed as f64))] } else { vec![] };
    finish(ctx, "datos", "xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", bytes, notes)
}

pub fn export_pdf(ctx: &ExportContext) -> ExportFile {
    let (bytes, replaced) = report::build_pdf(ctx);
    let notes = if replaced > 0 { vec![note("pdf_chars_replaced", "count", Json::num(replaced as f64))] } else { vec![] };
    finish(ctx, "memoria", "pdf", "application/pdf", bytes, notes)
}

pub fn export_ifc(ctx: &ExportContext) -> ExportFile {
    let meta = ifc::IfcMeta {
        project_name: ctx.project.at("meta.name").and_then(Json::as_str).unwrap_or("").to_string(),
        author: ctx.user.to_string(),
        status_text: catalog().tr(ctx.lang, &format!("status.{}", ctx.gate.status.as_str())),
        profile: ctx.profile.map(|p| format!("{}@{}", p.code(), p.version())).unwrap_or_else(|| "-".into()),
        profile_verified: ctx.profile.map(|p| p.is_verified()).unwrap_or(false),
        fingerprint: crate::project::inputs_fingerprint(ctx.project),
        timestamp: ctx.generated_at.trim_end_matches('Z').to_string(),
    };
    let (s, omitted) = ifc::build(ctx.project, &meta);
    let notes = omitted.into_iter().map(|d| note("ifc_omitted", "detail", Json::str(d))).collect();
    finish(ctx, "bim", "ifc", "application/x-step", s.into_bytes(), notes)
}

fn pts_of(j: &Json) -> Vec<Pt> {
    j.items().iter().filter_map(|p| Some([p.idx(0)?.as_f64()?, p.idx(1)?.as_f64()?])).collect()
}

/// DXF de un nivel (2D). Capas `VIGIX-*`, símbolos genéricos y banner de estado.
pub fn export_dxf(ctx: &ExportContext, level_id: Option<&str>) -> ExportFile {
    let levels = ctx.project.at("site.levels").map(Json::items).unwrap_or(&[]);
    let level = level_id.and_then(|id| levels.iter().find(|l| l.field("id").as_str() == Some(id))).or_else(|| levels.first());
    let lid = level.and_then(|l| l.field("id").as_str()).unwrap_or("");
    let mut d = dxf::Dxf::new();
    let mut notes: Vec<Json> = Vec::new();
    d.comment(&format!("{} {}", crate::PRODUCT_NAME, crate::ENGINE_VERSION));
    d.comment(&catalog().tr(ctx.lang, &format!("status.{}", ctx.gate.status.as_str())));
    d.comment(&format!(
        "Proyecto: {} | Nivel: {} | Unidades: metros",
        ctx.project.at("meta.name").and_then(Json::as_str).unwrap_or(""),
        lid
    ));
    d.comment(&format!(
        "Perfil: {} | Huella de datos: {}",
        ctx.profile
            .map(|p| format!("{}@{} ({})", p.code(), p.version(), if p.is_verified() { "verificado" } else { "NO VERIFICADO" }))
            .unwrap_or_else(|| "-".into()),
        crate::project::inputs_fingerprint(ctx.project)
    ));
    let mut all_pts: Vec<Pt> = Vec::new();
    if let Some(l) = level {
        for e in l.field("entities").items() {
            let id = e.field("id").as_str().unwrap_or("");
            let p = pts_of(e.field("pts"));
            all_pts.extend(p.iter().cloned());
            match e.field("type").as_str() {
                Some("wall") => d.polyline("VIGIX-ARQ-MUROS", &p, false, e.field("thickness_m").as_f64().unwrap_or(0.0)),
                Some("room") => {
                    d.polyline("VIGIX-ARQ-ZONAS", &p, true, 0.0);
                    if !p.is_empty() {
                        let c =
                            [p.iter().map(|v| v[0]).sum::<f64>() / p.len() as f64, p.iter().map(|v| v[1]).sum::<f64>() / p.len() as f64];
                        d.text("VIGIX-TEXTO", c, 0.25, e.field("name").as_str().filter(|s| !s.is_empty()).unwrap_or(id));
                    }
                }
                Some("door") if p.len() == 2 => d.line("VIGIX-ARQ-PUERTAS", p[0], p[1]),
                _ => {}
            }
        }
    }
    for u in ctx.project.at("systems.extinguishers.units").map(Json::items).unwrap_or(&[]) {
        if u.field("level").as_str() != Some(lid) {
            continue;
        }
        match (u.field("x").as_f64(), u.field("y").as_f64()) {
            (Some(x), Some(y)) => {
                d.insert("VIGIX-EXT-PORTATIL", "VIGIX_EXT", [x, y], 1.0);
                d.text("VIGIX-EXT-PORTATIL", [x + 0.25, y + 0.2], 0.15, u.field("id").as_str().unwrap_or(""));
                all_pts.push([x, y]);
            }
            _ => notes.push(note("dxf_no_coords", "detail", Json::str(u.field("id").as_str().unwrap_or("?")))),
        }
    }
    let mut between_levels = false;
    for net in ctx.project.at("systems.water.networks").map(Json::items).unwrap_or(&[]) {
        let mut pos: BTreeMap<String, (Pt, String)> = BTreeMap::new();
        for n in net.field("nodes").items() {
            let id = n.field("id").as_str().unwrap_or("").to_string();
            match (n.field("x").as_f64(), n.field("y").as_f64()) {
                (Some(x), Some(y)) => {
                    let nl = n.field("level").as_str().unwrap_or("").to_string();
                    pos.insert(id.clone(), ([x, y], nl.clone()));
                    if nl != lid {
                        continue;
                    }
                    all_pts.push([x, y]);
                    let kind = n.field("kind").as_str().unwrap_or("");
                    let (block, layer): (Option<&'static str>, &str) = match kind {
                        "sprinkler" => (Some("VIGIX_SPR"), "VIGIX-AGUA-DISPOSITIVO"),
                        "nozzle" => (Some("VIGIX_NOZ"), "VIGIX-AGUA-DISPOSITIVO"),
                        "hydrant" | "hose_valve" => (Some("VIGIX_HYD"), "VIGIX-AGUA-DISPOSITIVO"),
                        "supply" => (Some("VIGIX_SRC"), "VIGIX-AGUA-FUENTE"),
                        "tank" | "reservoir" => (Some("VIGIX_TNK"), "VIGIX-AGUA-FUENTE"),
                        _ => (None, ""),
                    };
                    if let Some(b) = block {
                        d.insert(layer, b, [x, y], 1.0);
                        d.text("VIGIX-TEXTO", [x + 0.25, y + 0.2], 0.15, &id);
                    }
                }
                _ => notes.push(note("dxf_no_coords", "detail", Json::str(id))),
            }
        }
        for k in net.field("links").items() {
            let (Some(a), Some(b)) = (k.field("from").as_str().and_then(|f| pos.get(f)), k.field("to").as_str().and_then(|t| pos.get(t)))
            else {
                continue;
            };
            if a.1 != b.1 {
                between_levels = true;
                continue;
            }
            if a.1 == lid {
                d.line("VIGIX-AGUA-TUBERIA", a.0, b.0);
                let mid = [(a.0[0] + b.0[0]) / 2.0, (a.0[1] + b.0[1]) / 2.0];
                if let Some(dm) = k.field("diameter_mm").as_f64() {
                    d.text("VIGIX-TEXTO", mid, 0.12, &format!("{} mm", crate::json::fmt_num(dm)));
                }
            }
        }
    }
    if between_levels {
        notes.push(Json::obj().with("code", Json::str("dxf_between_levels")));
    }
    // banner de estado sobre el dibujo
    let (lo_y, hi_y, lo_x) = all_pts.iter().fold((f64::MAX, f64::MIN, f64::MAX), |a, p| (a.0.min(p[1]), a.1.max(p[1]), a.2.min(p[0])));
    let (bx, by) = if all_pts.is_empty() { (0.0, 0.0) } else { (lo_x, hi_y + 1.0) };
    let _ = lo_y;
    d.text("VIGIX-ESTADO", [bx, by + 0.6], 0.4, &catalog().tr(ctx.lang, &format!("status.{}", ctx.gate.status.as_str())));
    d.text(
        "VIGIX-ESTADO",
        [bx, by],
        0.25,
        &format!(
            "{} - {} - {}",
            crate::PRODUCT_NAME,
            ctx.project.at("meta.name").and_then(Json::as_str).unwrap_or(""),
            ctx.profile.map(|p| format!("{}@{}", p.code(), p.version())).unwrap_or_default()
        ),
    );
    let (s, enc) = d.finish();
    if enc > 0 {
        notes.push(note("dxf_chars_encoded", "count", Json::num(enc as f64)));
    }
    finish(ctx, &format!("plano_{}", lid), "dxf", "application/dxf", s.into_bytes(), notes)
}

/// Manifiesto de una exportación: metadatos + huella de cada archivo.
pub fn manifest(ctx: &ExportContext, files: &[&ExportFile]) -> Json {
    let mut m = document_meta(ctx);
    m.set(
        "files",
        Json::Arr(
            files
                .iter()
                .map(|f| {
                    Json::obj()
                        .with("name", Json::str(f.filename.as_str()))
                        .with("bytes", Json::num(f.bytes.len() as f64))
                        .with("sha256", Json::str(f.sha256.as_str()))
                })
                .collect(),
        ),
    );
    m.set(
        "notes",
        Json::Arr(
            files.iter().flat_map(|f| f.notes.iter().cloned().map(move |n| n.with("file", Json::str(f.filename.as_str())))).collect(),
        ),
    );
    m
}
