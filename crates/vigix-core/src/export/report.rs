//! Memoria de cálculo en PDF y planos vectoriales a escala (contrato 5.12).
//!
//! Cada página lleva el banner de estado del documento ("BORRADOR / NO APTO PARA CONSTRUCCIÓN"
//! mientras haya bloqueos), marca de agua de borrador y/o de evaluación (Freemium) y pie con
//! huella de datos. El documento **nunca** afirma conformidad: como máximo "paquete de
//! presentación sujeto a firma y aprobación externa".

use super::pdf::*;
use super::tables::{self, value_rows};
use super::ExportContext;
use crate::gates::DocStatus;
use crate::geometry::{self, Pt};
use crate::i18n::{catalog, fill};
use crate::json::{fmt_num, Json};
use crate::units;
use std::collections::BTreeMap;

const MAX_ROWS_PDF: usize = 80;

fn pts_of(j: &Json) -> Vec<Pt> {
    j.items().iter().filter_map(|p| Some([p.idx(0)?.as_f64()?, p.idx(1)?.as_f64()?])).collect()
}

pub fn build_pdf(ctx: &ExportContext) -> (Vec<u8>, usize) {
    let cat = catalog();
    let l = ctx.lang;
    let system = ctx.project.at("display.unit_system").and_then(Json::as_str).unwrap_or("SI");
    let decimals = ctx.project.at("display.decimals").and_then(Json::as_f64).filter(|d| (0.0..=6.0).contains(d)).unwrap_or(2.0) as u32;
    let pname = ctx.project.at("meta.name").and_then(Json::as_str).unwrap_or("");
    let fp = crate::project::inputs_fingerprint(ctx.project);
    let (banner, bcolor) = match ctx.gate.status {
        DocStatus::Draft => (cat.tr(l, "status.draft"), RED),
        DocStatus::ForReview => (cat.tr(l, "status.for_review"), AMBER),
        DocStatus::PermitPackage => (cat.tr(l, "status.permit_package"), BLUE),
    };
    let profile_label = match ctx.profile {
        Some(p) if p.is_demo() => cat.tr(l, "status.profile_demo"),
        Some(p) if p.is_verified() => cat.tr(l, "status.profile_verified"),
        _ => cat.tr(l, "status.profile_unverified"),
    };
    let frame = Frame {
        banner,
        banner_color: bcolor,
        footer_left: format!("{} | {} | {} | {}…", cat.tr(l, "app.name"), pname, profile_label, &fp[..12]),
        page_word: cat.tr(l, "doc.page"),
        of_word: cat.tr(l, "doc.of"),
        watermark: if ctx.gate.status == DocStatus::Draft {
            Some((if l == "en" { "DRAFT".to_string() } else { "BORRADOR".to_string() }, RED))
        } else {
            None
        },
        eval_mark: if ctx.license.watermark { Some(cat.tr(l, "status.eval_watermark")) } else { None },
    };
    let mut lay = Layout::new(frame);
    let tr = |k: &str| cat.tr(l, k);

    // ---------------------------------------------------------------- portada
    lay.spacer(40.0);
    lay.paragraph(&tr("app.name"), 26.0, true, BLACK);
    lay.paragraph(&tr("doc.report_title"), 18.0, true, GRAY);
    lay.spacer(10.0);
    lay.paragraph(pname, 15.0, true, BLACK);
    lay.spacer(12.0);
    lay.kv_table(&tables::metadata_rows(ctx), 9.0);
    lay.spacer(8.0);
    lay.paragraph(&format!("{}", cat.tr(l, &format!("status.{}", ctx.gate.status.as_str()))), 12.0, true, bcolor);
    lay.paragraph(&profile_label, 11.0, true, if ctx.profile.map(|p| p.is_verified()).unwrap_or(false) { GREEN } else { AMBER });
    lay.spacer(6.0);
    lay.paragraph(&tr("app.disclaimer"), 9.0, false, GRAY);
    lay.paragraph(&tr("doc.sec.cover_note"), 8.0, false, GRAY);

    // ---------------------------------------------------------------- estado y bloqueos
    lay.new_page(A4);
    lay.heading(&tr("doc.sec.status"), 1);
    let gate_rows: Vec<Vec<String>> = ctx
        .gate
        .issues
        .0
        .iter()
        .filter(|i| i.severity >= crate::issues::Severity::Info)
        .map(|i| {
            let (m, a) = cat.issue_text(l, i);
            vec![cat.tr(l, &format!("sev.{}", i.severity.as_str())), m, a]
        })
        .collect();
    if gate_rows.is_empty() {
        lay.paragraph(&tr("doc.none"), 10.0, false, BLACK);
    } else {
        let shown: Vec<Vec<String>> = gate_rows.iter().take(MAX_ROWS_PDF).cloned().collect();
        lay.table(&[tr("doc.col.severity"), tr("doc.col.message"), tr("doc.col.action")], &shown, 8.0);
        if gate_rows.len() > shown.len() {
            lay.paragraph(
                &fill(
                    &tr("doc.rows_truncated"),
                    &BTreeMap::from([("shown".to_string(), shown.len().to_string()), ("total".to_string(), gate_rows.len().to_string())]),
                ),
                8.0,
                false,
                GRAY,
            );
        }
    }

    // ---------------------------------------------------------------- base de diseño
    lay.heading(&tr("doc.sec.basis"), 1);
    if let Some(p) = ctx.profile {
        lay.kv_table(
            &[
                (tr("doc.profile"), format!("{} v{}", p.code(), p.version())),
                (tr("doc.profile_state"), profile_label.clone()),
                (tr("doc.meta.profile_sha"), p.sha256.clone()),
                (tr("doc.meta.approval"), p.raw.at("authorship.approval_status").and_then(Json::as_str).unwrap_or("").to_string()),
                (tr("doc.meta.units"), system.to_string()),
            ],
            9.0,
        );
        lay.heading(&tr("doc.sec.sources"), 2);
        let src: Vec<Vec<String>> = p
            .raw
            .field("sources")
            .items()
            .iter()
            .map(|s| {
                vec![
                    s.field("body").as_str().unwrap_or("").to_string(),
                    s.field("code").as_str().unwrap_or("").to_string(),
                    s.field("title").as_str().unwrap_or("").to_string(),
                    s.field("edition").as_str().unwrap_or("").to_string(),
                    s.field("effective_date").as_str().unwrap_or("").to_string(),
                ]
            })
            .collect();
        if src.is_empty() {
            lay.paragraph(&tr("doc.sources_none"), 9.0, false, BLACK);
        } else {
            lay.table(
                &[tr("doc.col.body"), tr("doc.col.code"), tr("doc.col.name"), tr("doc.col.edition"), tr("doc.col.effective")],
                &src,
                8.0,
            );
        }
    } else {
        lay.paragraph("-", 10.0, false, BLACK);
    }
    lay.heading(&tr("doc.sec.assumptions"), 2);
    let ass: Vec<Vec<String>> = ctx
        .project
        .field("assumptions")
        .items()
        .iter()
        .map(|a| {
            vec![
                a.field("id").as_str().unwrap_or("").to_string(),
                a.field("text").as_str().unwrap_or("").to_string(),
                tr(if a.field("confirmed").as_bool() == Some(true) { "doc.yes" } else { "doc.no" }),
            ]
        })
        .collect();
    if ass.is_empty() {
        lay.paragraph(&tr("doc.none"), 9.0, false, BLACK);
    } else {
        lay.table(&[tr("doc.col.id"), tr("doc.col.description"), tr("doc.col.confirmed")], &ass, 8.0);
    }

    // ---------------------------------------------------------------- datos del emplazamiento
    lay.heading(&tr("doc.sec.site"), 1);
    let lv: Vec<Vec<String>> = ctx
        .project
        .at("site.levels")
        .map(Json::items)
        .unwrap_or(&[])
        .iter()
        .map(|x| {
            vec![
                x.field("id").as_str().unwrap_or("").to_string(),
                x.field("name").as_str().unwrap_or("").to_string(),
                x.field("elevation_m")
                    .as_f64()
                    .map(|v| fmt_num(units::round_half_away(conv(v, "m", "length", system), decimals)))
                    .unwrap_or_default(),
                x.field("height_m")
                    .as_f64()
                    .map(|v| fmt_num(units::round_half_away(conv(v, "m", "length", system), decimals)))
                    .unwrap_or_default(),
            ]
        })
        .collect();
    let ulen = unit_label("length", system);
    lay.table(
        &[
            tr("doc.col.id"),
            tr("doc.col.name"),
            format!("{} [{}]", tr("doc.col.elevation"), ulen),
            format!("{} [{}]", tr("doc.col.description"), ulen),
        ],
        &lv,
        8.5,
    );

    // ---------------------------------------------------------------- resultados por sistema
    lay.heading(&tr("doc.sec.results"), 1);
    let mut any = false;
    if let Some(env) = ctx.project.at("results.extinguishers") {
        any = true;
        result_header(&mut lay, ctx, "extinguishers", "extinguishers", env);
        lay.kv_table(&value_rows("extinguishers", env.field("values"), l, system, decimals).into_iter().collect::<Vec<_>>(), 9.0);
        let zt = &tables::ext_tables(ctx)[0];
        let rows = zt.text_rows(system, false, decimals);
        let shown: Vec<Vec<String>> = rows.iter().take(MAX_ROWS_PDF).cloned().collect();
        lay.table(&zt.headers(l, system, false), &shown, 8.0);
        trunc_note(&mut lay, l, shown.len(), rows.len());
    }
    for (label, env) in tables::hydraulic_results(ctx) {
        any = true;
        result_header(&mut lay, ctx, "hydraulic", &label, env);
        let mut kv = value_rows("hydraulic", env.field("values"), l, system, decimals);
        if let Some(sup) = env.at("values.supplies").and_then(|s| s.idx(0)) {
            for (k, canon, role, key) in [
                ("doc.sum.available", "bar", "pressure", "available_pressure_at_flow_bar"),
                ("doc.sum.margin", "bar", "pressure", "margin_bar"),
            ] {
                if let Some(v) = sup.field(key).as_f64() {
                    kv.push((
                        tr(k),
                        format!("{} {}", fmt_num(units::round_half_away(conv(v, canon, role, system), decimals)), unit_label(role, system)),
                    ));
                }
            }
        }
        lay.kv_table(&kv, 9.0);
        if env.field("status").as_str() == Some("ok") {
            let (nt, kt) = tables::hydraulic_tables(ctx);
            for (t, only) in [(&nt, &label), (&kt, &label)] {
                let rows: Vec<Vec<String>> = t
                    .rows
                    .iter()
                    .zip(t.text_rows(system, false, decimals))
                    .filter(|(r, _)| matches!(&r[0], tables::V::S(s) if s == only))
                    .map(|(_, tx)| tx[1..].to_vec())
                    .collect();
                let hd: Vec<String> = t.headers(l, system, false).into_iter().skip(1).collect();
                let shown: Vec<Vec<String>> = rows.iter().take(MAX_ROWS_PDF).cloned().collect();
                lay.table(&hd, &shown, 7.5);
                trunc_note(&mut lay, l, shown.len(), rows.len());
            }
        }
    }
    for kind in ["foam", "gas", "dry_chemical", "wet_chemical", "aerosol"] {
        if let Some(m) = ctx.project.at(&format!("results.{}", kind)).and_then(Json::as_obj) {
            for (sid, env) in m {
                any = true;
                result_header(&mut lay, ctx, kind, sid, env);
                lay.kv_table(&value_rows(kind, env.field("values"), l, system, decimals), 9.0);
                if kind == "gas" {
                    let ne: Vec<&str> =
                        env.at("values.not_evaluated").map(Json::items).unwrap_or(&[]).iter().filter_map(Json::as_str).collect();
                    if !ne.is_empty() {
                        lay.paragraph(&format!("{}: {}", tr("doc.not_evaluated"), ne.join(", ")), 8.5, true, AMBER);
                    }
                }
            }
        }
    }
    if let Some(env) = ctx.project.at("results.cause_effect") {
        any = true;
        result_header(&mut lay, ctx, "cause_effect", "cause_effect", env);
        let t = tables::cause_effect_table(ctx);
        if !t.rows.is_empty() {
            lay.paragraph(&tr("doc.informative_interface"), 8.5, true, AMBER);
            let rows = t.text_rows(system, true, decimals);
            lay.table(&t.headers(l, system, true), &rows, 7.5);
        }
    }
    if !any {
        lay.paragraph(&tr("doc.no_result"), 10.0, false, GRAY);
    }

    // ---------------------------------------------------------------- presupuesto
    if ctx.project.at("budget.lines").map(|x| !x.items().is_empty()).unwrap_or(false) {
        lay.heading(&tr("doc.sec.budget"), 1);
        let t = tables::budget_table(ctx);
        let rows = t.text_rows(system, true, decimals);
        lay.table(&t.headers(l, system, true), &rows, 8.0);
    }

    // ---------------------------------------------------------------- planos
    for lv in ctx.project.at("site.levels").map(Json::items).unwrap_or(&[]) {
        draw_plan(&mut lay, ctx, lv);
    }

    // ---------------------------------------------------------------- incidencias, parámetros, rastro
    lay.new_page(A4);
    lay.heading(&tr("doc.sec.issues"), 1);
    let it = tables::issues_table(ctx);
    let irows = it.text_rows(system, true, decimals);
    // columnas del PDF: severidad, objeto, mensaje, acción, referencia
    let keep = [0usize, 3, 4, 5, 6];
    let hd: Vec<String> = it.headers(l, system, true).into_iter().enumerate().filter(|(i, _)| keep.contains(i)).map(|(_, h)| h).collect();
    let shown: Vec<Vec<String>> = irows.iter().take(150).map(|r| keep.iter().map(|&i| r[i].clone()).collect()).collect();
    if shown.is_empty() {
        lay.paragraph(&tr("doc.none"), 9.0, false, BLACK);
    } else {
        lay.table(&hd, &shown, 7.5);
    }
    trunc_note(&mut lay, l, shown.len(), irows.len());
    lay.heading(&tr("doc.sec.params"), 1);
    let pt = tables::params_table(ctx);
    let prows = pt.text_rows(system, true, decimals);
    if prows.is_empty() {
        lay.paragraph(&tr("doc.none"), 9.0, false, BLACK);
    } else {
        let shown: Vec<Vec<String>> = prows.iter().take(150).cloned().collect();
        lay.table(&pt.headers(l, system, true), &shown, 7.5);
        trunc_note(&mut lay, l, shown.len(), prows.len());
    }
    lay.heading(&tr("doc.sec.trace"), 1);
    let mut trows: Vec<Vec<String>> = Vec::new();
    if let Some(Json::Obj(res)) = ctx.project.get("results") {
        for (kind, node) in res {
            let envs: Vec<(String, &Json)> = if node.get("trace").is_some() {
                vec![(kind.clone(), node)]
            } else {
                node.as_obj().map(|m| m.iter().map(|(k, v)| (format!("{}:{}", kind, k), v)).collect()).unwrap_or_default()
            };
            for (label, env) in envs {
                for t in env.field("trace").items() {
                    trows.push(vec![
                        label.clone(),
                        t.field("step").as_str().unwrap_or("").to_string(),
                        t.field("equation").as_str().unwrap_or("").to_string(),
                    ]);
                }
            }
        }
    }
    if trows.is_empty() {
        lay.paragraph(&tr("doc.none"), 9.0, false, BLACK);
    } else {
        let shown: Vec<Vec<String>> = trows.iter().take(150).cloned().collect();
        lay.table(&[tr("doc.col.source"), tr("doc.col.step"), tr("doc.col.equation")], &shown, 7.5);
    }

    // ---------------------------------------------------------------- firmas y alcance
    lay.heading(&tr("doc.sec.signatures"), 1);
    let revs: Vec<Vec<String>> = ctx
        .project
        .at("meta.revisions")
        .map(Json::items)
        .unwrap_or(&[])
        .iter()
        .map(|r| {
            vec![
                r.field("rev").as_f64().map(fmt_num).unwrap_or_default(),
                r.field("date").as_str().unwrap_or("").to_string(),
                r.field("author").as_str().unwrap_or("").to_string(),
                r.field("description").as_str().unwrap_or("").to_string(),
            ]
        })
        .collect();
    if !revs.is_empty() {
        lay.table(&[tr("doc.revision"), tr("doc.sign.date"), tr("doc.author"), tr("doc.col.description")], &revs, 8.0);
    }
    for role in ["doc.sign.designer", "doc.sign.reviewer", "doc.sign.approver"] {
        lay.spacer(18.0);
        let y = lay.cursor();
        let (x0, w) = (lay.margin, lay.content_width());
        let pg = lay.page_index();
        lay.pdf.text(pg, x0, y, 9.0, true, BLACK, &tr(role));
        lay.pdf.text(
            pg,
            x0 + 120.0,
            y,
            8.0,
            false,
            GRAY,
            &format!(
                "{}: ____________________   {}: ____________________   {}: ________",
                tr("doc.sign.name"),
                tr("doc.sign.signature"),
                tr("doc.sign.date")
            ),
        );
        let _ = w;
        lay.set_cursor(y - 10.0);
    }
    lay.paragraph(&tr("doc.sign.note"), 8.0, false, GRAY);
    lay.heading(&tr("doc.sec.limits"), 1);
    lay.paragraph(&tr("app.disclaimer"), 9.0, false, BLACK);
    lay.paragraph(&tr("app.scope_limits"), 9.0, false, BLACK);

    let subject = format!("{} | {} | {}", cat.tr(l, &format!("status.{}", ctx.gate.status.as_str())), profile_label, fp);
    lay.finish(
        &format!("{} - {}", tr("doc.report_title"), pname),
        &format!("{} {}", tr("app.name"), crate::ENGINE_VERSION),
        &subject,
        &format!("VIGIX;{};{}", ctx.profile.map(|p| format!("{}@{}", p.code(), p.version())).unwrap_or_default(), fp),
    )
}

fn conv(v: f64, canon: &str, role: &str, system: &str) -> f64 {
    let to = units::table().display_unit(system, role).map(|(_, u)| u.to_string()).unwrap_or_else(|| canon.to_string());
    units::convert(v, canon, &to).unwrap_or(v)
}

fn unit_label(role: &str, system: &str) -> String {
    units::table().display_unit(system, role).map(|(_, u)| tables::pretty_unit(u)).unwrap_or_default()
}

fn trunc_note(lay: &mut Layout, lang: &str, shown: usize, total: usize) {
    if total > shown {
        let t = fill(
            &catalog().tr(lang, "doc.rows_truncated"),
            &BTreeMap::from([("shown".to_string(), shown.to_string()), ("total".to_string(), total.to_string())]),
        );
        lay.paragraph(&t, 8.0, false, GRAY);
    }
}

/// Encabezado de un resultado con su estado (calculado / obsoleto / bloqueado).
fn result_header(lay: &mut Layout, ctx: &ExportContext, kind: &str, label: &str, env: &Json) {
    let cat = catalog();
    let l = ctx.lang;
    let mut title = format!("{} — {}", cat.tr(l, &format!("res.{}", kind)), label);
    let mut color = BLACK;
    if env.field("status").as_str() != Some("ok") {
        title.push_str(&format!("  [{}]", cat.tr(l, "res.blocked")));
        color = RED;
    } else if ctx.gate.issues.0.iter().any(|i| {
        i.code == "gate.result_stale" && i.params.get("result").map(|r| r == label || r.ends_with(&format!(":{}", label))).unwrap_or(false)
    }) {
        title.push_str(&format!("  [{}]", cat.tr(l, "res.stale")));
        color = AMBER;
    }
    lay.heading(&title, 2);
    if color == RED {
        lay.paragraph(&cat.tr(l, "doc.blocked_label"), 9.0, true, RED);
    } else if color == AMBER {
        lay.paragraph(&cat.tr(l, "doc.stale_label"), 9.0, true, AMBER);
    }
}

// ============================================================================= planos
fn draw_plan(lay: &mut Layout, ctx: &ExportContext, level: &Json) {
    let cat = catalog();
    let l = ctx.lang;
    let lid = level.field("id").as_str().unwrap_or("");
    let mut pts: Vec<Pt> = Vec::new();
    let ents = level.field("entities").items();
    for e in ents {
        pts.extend(pts_of(e.field("pts")));
    }
    let units_j = ctx.project.at("systems.extinguishers.units").map(Json::items).unwrap_or(&[]);
    for u in units_j.iter().filter(|u| u.field("level").as_str() == Some(lid)) {
        if let (Some(x), Some(y)) = (u.field("x").as_f64(), u.field("y").as_f64()) {
            pts.push([x, y]);
        }
    }
    let mut nodes: BTreeMap<String, (f64, f64, String)> = BTreeMap::new();
    let mut links: Vec<(String, String, f64)> = Vec::new();
    for net in ctx.project.at("systems.water.networks").map(Json::items).unwrap_or(&[]) {
        for n in net.field("nodes").items() {
            if n.field("level").as_str() == Some(lid) {
                if let (Some(x), Some(y)) = (n.field("x").as_f64(), n.field("y").as_f64()) {
                    pts.push([x, y]);
                    nodes.insert(
                        n.field("id").as_str().unwrap_or("").to_string(),
                        (x, y, n.field("kind").as_str().unwrap_or("").to_string()),
                    );
                }
            }
        }
        for k in net.field("links").items() {
            links.push((
                k.field("from").as_str().unwrap_or("").to_string(),
                k.field("to").as_str().unwrap_or("").to_string(),
                k.field("diameter_mm").as_f64().unwrap_or(0.0),
            ));
        }
    }
    let Some((lo, hi)) = geometry::bbox(&pts) else { return };
    let page = lay.new_page(A4_LANDSCAPE);
    let (pw, ph) = lay.size();
    lay.heading(
        &format!("{} — {} {} ({})", cat.tr(l, "doc.plan"), cat.tr(l, "doc.level"), level.field("name").as_str().unwrap_or(lid), lid),
        2,
    );
    let (x0, y0) = (lay.margin + 10.0, lay.margin + 52.0);
    let (w, h) = (pw - 2.0 * lay.margin - 20.0, lay.cursor() - y0 - 4.0);
    let (bw, bh) = ((hi[0] - lo[0]).max(1.0), (hi[1] - lo[1]).max(1.0));
    let s = (w / bw).min(h / bh);
    let (ox, oy) = (x0 + (w - bw * s) / 2.0, y0 + (h - bh * s) / 2.0);
    let tx = |p: Pt| (ox + (p[0] - lo[0]) * s, oy + (p[1] - lo[1]) * s);
    let _ = ph;
    // zonas sin cubrir (resultado de extintores)
    if let Some(env) = ctx.project.at("results.extinguishers") {
        for z in env.at("values.zones").map(Json::items).unwrap_or(&[]) {
            if z.field("level").as_str() != Some(lid) {
                continue;
            }
            for c in z.field("classes").items() {
                for r in c.field("uncovered_rects").items() {
                    if let (Some(rx), Some(ry), Some(rw), Some(rh)) = (
                        r.idx(0).and_then(Json::as_f64),
                        r.idx(1).and_then(Json::as_f64),
                        r.idx(2).and_then(Json::as_f64),
                        r.idx(3).and_then(Json::as_f64),
                    ) {
                        let (a, b) = tx([rx, ry]);
                        lay.pdf.rect(page, a, b, rw * s, rh * s + 0.2, None, Some((1.0, 0.82, 0.82)));
                    }
                }
            }
        }
    }
    for e in ents {
        let p = pts_of(e.field("pts"));
        match e.field("type").as_str() {
            Some("room") if p.len() >= 3 => {
                let q: Vec<(f64, f64)> = p.iter().map(|v| tx(*v)).collect();
                lay.pdf.polyline(page, &q, true, 0.5, GRAY, None);
                let c = [p.iter().map(|v| v[0]).sum::<f64>() / p.len() as f64, p.iter().map(|v| v[1]).sum::<f64>() / p.len() as f64];
                let (cx, cy) = tx(c);
                let name = e.field("name").as_str().filter(|n| !n.is_empty()).unwrap_or_else(|| e.field("id").as_str().unwrap_or(""));
                let tw = text_width(name, 7.0, false);
                lay.pdf.text(page, cx - tw / 2.0, cy, 7.0, false, GRAY, name);
            }
            Some("wall") if p.len() >= 2 => {
                let q: Vec<(f64, f64)> = p.iter().map(|v| tx(*v)).collect();
                let t = e.field("thickness_m").as_f64().unwrap_or(0.2);
                lay.pdf.polyline(page, &q, false, (t * s).clamp(0.6, 6.0), BLACK, None);
            }
            Some("door") if p.len() == 2 => {
                let (a, b) = (tx(p[0]), tx(p[1]));
                lay.pdf.line(page, a.0, a.1, b.0, b.1, 2.4, (0.6, 0.4, 0.2));
            }
            _ => {}
        }
    }
    for (a, b, d) in &links {
        if let (Some(na), Some(nb)) = (nodes.get(a), nodes.get(b)) {
            let (p, q) = (tx([na.0, na.1]), tx([nb.0, nb.1]));
            lay.pdf.line(page, p.0, p.1, q.0, q.1, 0.8 + (d / 50.0).min(2.5), (0.15, 0.4, 0.8));
        }
    }
    let label_ids = nodes.len() + units_j.len() <= 80;
    for (id, (x, y, kind)) in &nodes {
        let (cx, cy) = tx([*x, *y]);
        match kind.as_str() {
            "sprinkler" | "nozzle" => {
                lay.pdf.circle(page, cx, cy, 3.0, 0.8, (0.1, 0.3, 0.7), Some(WHITE));
                lay.pdf.line(page, cx - 3.0, cy, cx + 3.0, cy, 0.5, (0.1, 0.3, 0.7));
                lay.pdf.line(page, cx, cy - 3.0, cx, cy + 3.0, 0.5, (0.1, 0.3, 0.7));
            }
            "hydrant" | "hose_valve" => lay.pdf.polyline(
                page,
                &[(cx - 4.0, cy - 3.5), (cx + 4.0, cy - 3.5), (cx, cy + 4.0)],
                true,
                0.8,
                (0.1, 0.3, 0.7),
                Some(WHITE),
            ),
            "supply" | "tank" | "reservoir" => {
                lay.pdf.rect(page, cx - 4.5, cy - 4.5, 9.0, 9.0, Some((1.0, (0.1, 0.3, 0.7))), Some((0.8, 0.9, 1.0)))
            }
            _ => lay.pdf.circle(page, cx, cy, 1.2, 0.5, (0.15, 0.4, 0.8), Some((0.15, 0.4, 0.8))),
        }
        if label_ids && !matches!(kind.as_str(), "junction") {
            lay.pdf.text(page, cx + 4.5, cy + 3.0, 6.0, false, (0.1, 0.3, 0.7), id);
        }
    }
    for u in units_j.iter().filter(|u| u.field("level").as_str() == Some(lid)) {
        if let (Some(x), Some(y)) = (u.field("x").as_f64(), u.field("y").as_f64()) {
            let (cx, cy) = tx([x, y]);
            lay.pdf.rect(page, cx - 4.0, cy - 4.0, 8.0, 8.0, Some((0.8, RED)), Some(WHITE));
            lay.pdf.text(page, cx - 2.2, cy - 2.2, 6.5, true, RED, "E");
            if label_ids {
                lay.pdf.text(page, cx + 5.0, cy + 3.0, 6.0, false, RED, u.field("id").as_str().unwrap_or(""));
            }
        }
    }
    // escala aproximada y leyenda
    let denom = (1.0 / (s * 0.000_352_78)).round();
    let denom = if denom > 50.0 { (denom / 5.0).round() * 5.0 } else { denom };
    let bar_m = nice_bar(bw / 4.0);
    lay.pdf.line(page, x0, y0 - 14.0, x0 + bar_m * s, y0 - 14.0, 2.0, BLACK);
    lay.pdf.text(
        page,
        x0,
        y0 - 24.0,
        7.5,
        false,
        BLACK,
        &format!("{} m  —  {} 1:{}", fmt_num(bar_m), cat.tr(l, "doc.scale"), fmt_num(denom)),
    );
    lay.pdf.text(page, x0 + 220.0, y0 - 24.0, 7.5, false, GRAY, &cat.tr(l, "doc.plan_legend"));
}

fn nice_bar(target: f64) -> f64 {
    let mut b = 1.0;
    for cand in [0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0] {
        if cand <= target.max(0.5) {
            b = cand;
        }
    }
    b
}
