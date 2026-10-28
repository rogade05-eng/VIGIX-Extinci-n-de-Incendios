//! Extintores portátiles (contrato 5.3).
//!
//! Qué calcula el motor (todo valor numérico sale del perfil normativo):
//!  * compatibilidad agente/clase de fuego (tabla del perfil; sin fila => bloqueo, nunca inferencia);
//!  * clasificación mínima por riesgo y clase;
//!  * cobertura por **distancia de recorrido real** con obstáculos: muros y puertas se rasterizan
//!    y se aplica Dijkstra de 8 vecinos sin atravesar esquinas. La métrica octil sobrestima la
//!    distancia euclídea hasta ~8 %, por lo que el resultado es **conservador** (cubre menos);
//!  * criterio de área (por unidad o por clasificación) si el perfil lo declara, contando solo
//!    unidades situadas **dentro** de la zona (criterio conservador);
//!  * altura de montaje si el perfil define un límite;
//!  * propuesta de ubicaciones por cobertura voraz (determinista), siempre "requiere revisión".

use crate::calc::{fingerprint, trace_step, CalcOutput, Status};
use crate::geometry::{self, Pt};
use crate::issues::{Issue, Severity};
use crate::json::Json;
use crate::profile::{Param, Profile};
use crate::reader::{Reader, Scope};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

pub const MODULE: &str = "extinguishers";
pub const METHOD_TRAVEL: &str = "travel_distance_only.v1";
pub const METHOD_AREA_UNIT: &str = "travel_distance_and_area_per_unit.v1";
pub const METHOD_AREA_RATING: &str = "travel_distance_and_area_per_rating.v1";
pub const MAX_CELLS: usize = 4_000_000;
const MAX_RECTS_PER_ZONE: usize = 3000;

#[derive(Clone, Copy, PartialEq, Debug)]
enum AreaBasis {
    None,
    PerUnit,
    PerRating,
}

struct Room {
    id: String,
    pts: Vec<Pt>,
    hazard: String,
    classes: Vec<String>,
}

struct Unit {
    id: String,
    level: String,
    pos: Pt,
    agent: String,
    ratings: BTreeMap<String, f64>,
    mount_height_m: Option<f64>,
}

struct LevelGeo {
    id: String,
    walls: Vec<(Vec<Pt>, f64)>,
    doors: Vec<(Pt, Pt)>,
    rooms: Vec<usize>,
}

struct Grid {
    x0: f64,
    y0: f64,
    step: f64,
    nx: usize,
    ny: usize,
    blocked: Vec<bool>,
}

impl Grid {
    fn center(&self, i: usize, j: usize) -> Pt {
        [self.x0 + (i as f64 + 0.5) * self.step, self.y0 + (j as f64 + 0.5) * self.step]
    }
    fn cell_of(&self, p: Pt) -> Option<(usize, usize)> {
        let (fx, fy) = ((p[0] - self.x0) / self.step, (p[1] - self.y0) / self.step);
        if fx < 0.0 || fy < 0.0 {
            return None;
        }
        let (i, j) = (fx as usize, fy as usize);
        if i < self.nx && j < self.ny {
            Some((i, j))
        } else {
            None
        }
    }
    fn free(&self, i: usize, j: usize) -> bool {
        !self.blocked[j * self.nx + i]
    }
    /// Celda libre más cercana (búsqueda en anillos) hasta `max_r` celdas.
    fn nearest_free(&self, p: Pt, max_r: usize) -> Option<(usize, usize)> {
        let (ci, cj) = self.cell_of(p)?;
        if self.free(ci, cj) {
            return Some((ci, cj));
        }
        let mut best: Option<((usize, usize), f64)> = None;
        for r in 1..=max_r as i64 {
            for dj in -r..=r {
                for di in -r..=r {
                    if di.abs().max(dj.abs()) != r {
                        continue;
                    }
                    let (i, j) = (ci as i64 + di, cj as i64 + dj);
                    if i < 0 || j < 0 || i >= self.nx as i64 || j >= self.ny as i64 {
                        continue;
                    }
                    let (i, j) = (i as usize, j as usize);
                    if self.free(i, j) {
                        let d = geometry::dist(p, self.center(i, j));
                        if best.map(|b| d < b.1).unwrap_or(true) {
                            best = Some(((i, j), d));
                        }
                    }
                }
            }
            if best.is_some() {
                break;
            }
        }
        best.map(|b| b.0)
    }

    /// Dijkstra multifuente de 8 vecinos sin cortar esquinas, con corte `cutoff` (m).
    fn dijkstra(&self, sources: &[(usize, usize)], cutoff: f64) -> Vec<f64> {
        let mut dist = vec![f64::INFINITY; self.nx * self.ny];
        let mut heap: BinaryHeap<Reverse<(u64, u32)>> = BinaryHeap::new();
        for &(i, j) in sources {
            let k = j * self.nx + i;
            if dist[k] > 0.0 {
                dist[k] = 0.0;
                heap.push(Reverse((0f64.to_bits(), k as u32)));
            }
        }
        let diag = std::f64::consts::SQRT_2 * self.step;
        while let Some(Reverse((db, k))) = heap.pop() {
            let d = f64::from_bits(db);
            let k = k as usize;
            if d > dist[k] {
                continue;
            }
            let (i, j) = ((k % self.nx) as i64, (k / self.nx) as i64);
            for dj in -1i64..=1 {
                for di in -1i64..=1 {
                    if di == 0 && dj == 0 {
                        continue;
                    }
                    let (ni, nj) = (i + di, j + dj);
                    if ni < 0 || nj < 0 || ni >= self.nx as i64 || nj >= self.ny as i64 {
                        continue;
                    }
                    let (niu, nju) = (ni as usize, nj as usize);
                    if !self.free(niu, nju) {
                        continue;
                    }
                    let c = if di != 0 && dj != 0 {
                        // sin cortar esquinas: ambos vecinos ortogonales deben estar libres
                        if !self.free(niu, j as usize) || !self.free(i as usize, nju) {
                            continue;
                        }
                        diag
                    } else {
                        self.step
                    };
                    let nd = d + c;
                    if nd > cutoff + 1e-9 {
                        continue;
                    }
                    let nk = nju * self.nx + niu;
                    if nd < dist[nk] {
                        dist[nk] = nd;
                        heap.push(Reverse((nd.to_bits(), nk as u32)));
                    }
                }
            }
        }
        dist
    }
}

fn read_pts(rd: &mut Reader, s: &Scope, key: &str) -> Option<Vec<Pt>> {
    let arr = rd.arr(s, key)?;
    let mut out = Vec::with_capacity(arr.len());
    for p in arr {
        match (p.idx(0).and_then(Json::as_f64), p.idx(1).and_then(Json::as_f64)) {
            (Some(x), Some(y)) if x.is_finite() && y.is_finite() => out.push([x, y]),
            _ => {
                rd.push(Issue::block("data.type").module(MODULE).path(&s.key_path(key)).opt_object(s.id.as_deref()).param("field", key));
                return None;
            }
        }
    }
    Some(out)
}

fn read_pt(rd: &mut Reader, s: &Scope) -> Option<Pt> {
    let x = rd.f64(s, "x")?;
    let y = rd.f64(s, "y")?;
    Some([x, y])
}

fn rasterize(level: &LevelGeo, units: &[&Unit], rooms: &[Room], step: f64, rd: &mut Reader) -> Option<Grid> {
    let mut pts: Vec<Pt> = Vec::new();
    for (w, _) in &level.walls {
        pts.extend(w.iter().cloned());
    }
    for (a, b) in &level.doors {
        pts.push(*a);
        pts.push(*b);
    }
    for &ri in &level.rooms {
        pts.extend(rooms[ri].pts.iter().cloned());
    }
    for u in units {
        pts.push(u.pos);
    }
    let (lo, hi) = geometry::bbox(&pts)?;
    let margin = 1.0;
    let (x0, y0) = (lo[0] - margin, lo[1] - margin);
    let nx = ((hi[0] + margin - x0) / step).ceil().max(1.0) as usize;
    let ny = ((hi[1] + margin - y0) / step).ceil().max(1.0) as usize;
    if nx.saturating_mul(ny) > MAX_CELLS {
        rd.push(
            Issue::block("ext.grid_too_large")
                .module(MODULE)
                .path(&format!("site.levels.{}", level.id))
                .object(&level.id)
                .param("cells", nx.saturating_mul(ny))
                .param("max", MAX_CELLS),
        );
        return None;
    }
    let mut g = Grid { x0, y0, step, nx, ny, blocked: vec![false; nx * ny] };
    let mut max_t: f64 = 0.0;
    for (poly, t) in &level.walls {
        let half = (t / 2.0).max(step * 0.75);
        max_t = max_t.max(*t);
        for seg in poly.windows(2) {
            let (a, b) = (seg[0], seg[1]);
            let (lx, hx) = (a[0].min(b[0]) - half, a[0].max(b[0]) + half);
            let (ly, hy) = (a[1].min(b[1]) - half, a[1].max(b[1]) + half);
            let i0 = (((lx - x0) / step).floor().max(0.0)) as usize;
            let i1 = ((((hx - x0) / step).ceil()).max(0.0) as usize).min(nx - 1);
            let j0 = (((ly - y0) / step).floor().max(0.0)) as usize;
            let j1 = ((((hy - y0) / step).ceil()).max(0.0) as usize).min(ny - 1);
            for j in j0..=j1 {
                for i in i0..=i1 {
                    if geometry::dist_point_segment(g.center(i, j), a, b) <= half {
                        g.blocked[j * nx + i] = true;
                    }
                }
            }
        }
    }
    // Las puertas abren el muro: se liberan las celdas dentro del tramo de la puerta.
    let clear = (0.5 * max_t + step).max(step * 1.5);
    let mut door_cell = vec![false; nx * ny];
    for (a, b) in &level.doors {
        let (lx, hx) = (a[0].min(b[0]) - clear, a[0].max(b[0]) + clear);
        let (ly, hy) = (a[1].min(b[1]) - clear, a[1].max(b[1]) + clear);
        let i0 = (((lx - x0) / step).floor().max(0.0)) as usize;
        let i1 = ((((hx - x0) / step).ceil()).max(0.0) as usize).min(nx - 1);
        let j0 = (((ly - y0) / step).floor().max(0.0)) as usize;
        let j1 = ((((hy - y0) / step).ceil()).max(0.0) as usize).min(ny - 1);
        for j in j0..=j1 {
            for i in i0..=i1 {
                let (t, d) = geometry::project_on_segment(g.center(i, j), *a, *b);
                if (0.0..=1.0).contains(&t) && d <= clear {
                    g.blocked[j * nx + i] = false;
                    door_cell[j * nx + i] = true;
                }
            }
        }
    }
    // Solo es transitable el interior de las zonas y las puertas: sin esto, un muro sin cierre
    // exterior permitiría rodearlo por el "exterior" abierto (fuga del modelo, no conservadora).
    let mut walk = door_cell;
    for &ri in &level.rooms {
        let poly = &rooms[ri].pts;
        if let Some((rlo, rhi)) = geometry::bbox(poly) {
            let i0 = (((rlo[0] - x0) / step).floor().max(0.0)) as usize;
            let j0 = (((rlo[1] - y0) / step).floor().max(0.0)) as usize;
            let i1 = ((((rhi[0] - x0) / step).ceil()).max(0.0) as usize).min(nx - 1);
            let j1 = ((((rhi[1] - y0) / step).ceil()).max(0.0) as usize).min(ny - 1);
            for j in j0..=j1 {
                for i in i0..=i1 {
                    if geometry::point_in_polygon(g.center(i, j), poly) {
                        walk[j * nx + i] = true;
                    }
                }
            }
        }
    }
    for (k, w) in walk.iter().enumerate() {
        if !*w {
            g.blocked[k] = true;
        }
    }
    Some(g)
}

fn zone_cells(g: &Grid, poly: &[Pt]) -> Vec<(usize, usize)> {
    let Some((lo, hi)) = geometry::bbox(poly) else { return vec![] };
    let i0 = (((lo[0] - g.x0) / g.step).floor().max(0.0)) as usize;
    let j0 = (((lo[1] - g.y0) / g.step).floor().max(0.0)) as usize;
    let i1 = ((((hi[0] - g.x0) / g.step).ceil()).max(0.0) as usize).min(g.nx - 1);
    let j1 = ((((hi[1] - g.y0) / g.step).ceil()).max(0.0) as usize).min(g.ny - 1);
    let mut out = Vec::new();
    for j in j0..=j1 {
        for i in i0..=i1 {
            if g.free(i, j) && geometry::point_in_polygon(g.center(i, j), poly) {
                out.push((i, j));
            }
        }
    }
    out
}

/// Agrupa celdas no cubiertas en rectángulos por filas (para dibujarlas).
fn rects_of(g: &Grid, cells: &[(usize, usize)]) -> Vec<Json> {
    let mut sorted: Vec<(usize, usize)> = cells.to_vec();
    sorted.sort_by_key(|c| (c.1, c.0));
    let mut out: Vec<Json> = Vec::new();
    let mut k = 0;
    while k < sorted.len() && out.len() < MAX_RECTS_PER_ZONE {
        let (i0, j) = sorted[k];
        let mut m = k;
        while m + 1 < sorted.len() && sorted[m + 1].1 == j && sorted[m + 1].0 == sorted[m].0 + 1 {
            m += 1;
        }
        let w = (sorted[m].0 - i0 + 1) as f64 * g.step;
        out.push(Json::Arr(vec![
            Json::num(g.x0 + i0 as f64 * g.step),
            Json::num(g.y0 + j as f64 * g.step),
            Json::num(w),
            Json::num(g.step),
        ]));
        k = m + 1;
    }
    out
}

type Candidate = ((usize, usize), Vec<(usize, usize)>);

struct Req {
    max_travel: Param,
    min_rating: Option<Param>,
    area: Option<Param>,
}

/// Huella de entrada (zonas, geometría y unidades + perfil + versión del motor).
pub fn input_fingerprint(project: &Json, profile: &Profile, suggest: Option<&Json>) -> String {
    fingerprint(
        "extinguishers",
        &[project.field("site"), project.field("systems").field("extinguishers"), suggest.unwrap_or(&Json::Null)],
        Some(profile),
    )
}

#[allow(clippy::too_many_lines)]
pub fn run(project: &Json, profile: &Profile, suggest: Option<&Json>) -> CalcOutput {
    let fp = input_fingerprint(project, profile, suggest);
    let mut rd = Reader::new(MODULE);
    let mut used: Vec<Param> = Vec::new();
    let mut trace: Vec<Json> = Vec::new();

    if !profile.covers("portable_extinguishers") {
        rd.push(Issue::block("gate.profile_not_covering").module(MODULE).path("scope.systems").param("system", "portable_extinguishers"));
        return CalcOutput::blocked("extinguishers", rd.issues, fp);
    }
    let basis = match profile.method("extinguishers.coverage") {
        Some(METHOD_TRAVEL) => AreaBasis::None,
        Some(METHOD_AREA_UNIT) => AreaBasis::PerUnit,
        Some(METHOD_AREA_RATING) => AreaBasis::PerRating,
        other => {
            rd.push(
                Issue::block("profile.method_unsupported")
                    .module(MODULE)
                    .path("methods.extinguishers.coverage")
                    .param("method", other.unwrap_or(""))
                    .param("module", "extinguishers.coverage"),
            );
            return CalcOutput::blocked("extinguishers", rd.issues, fp);
        }
    };
    let max_mount = match profile.param_in("extinguishers.max_mount_height", "m") {
        Ok(p) => {
            used.push(p.clone());
            Some(p)
        }
        Err(i) if i.code == "profile.param_missing" => None,
        Err(i) => {
            rd.push(i);
            None
        }
    };

    let root = Scope::new(project, "");
    let site = Scope::new(project.field("site"), "site");
    let ext_root = Scope::new(project.field("systems").field("extinguishers"), "systems.extinguishers");
    let analysis = rd.obj(&ext_root, "analysis");
    let step = analysis.as_ref().and_then(|a| rd.f64_pos(a, "grid_m"));
    let levels_j = rd.arr(&site, "levels");
    let units_j = rd.arr(&ext_root, "units");
    let _ = root;
    let (Some(step), Some(levels_j), Some(units_j)) = (step, levels_j, units_j) else {
        return CalcOutput::blocked("extinguishers", rd.issues, fp);
    };

    // ---- niveles y geometría
    let mut levels: Vec<LevelGeo> = Vec::new();
    let mut rooms: Vec<Room> = Vec::new();
    let mut seen_ids: BTreeMap<String, usize> = BTreeMap::new();
    let levels_scope = site.child("levels");
    for (li, lj) in levels_j.iter().enumerate() {
        let ls = levels_scope.item(li, lj);
        let Some(lid) = rd.str(&ls, "id") else { continue };
        let ls = ls.with_id(lid);
        let mut geo = LevelGeo { id: lid.to_string(), walls: vec![], doors: vec![], rooms: vec![] };
        let ents = rd.arr(&ls, "entities");
        let ents_scope = ls.child("entities");
        for (ei, ej) in ents.unwrap_or(&[]).iter().enumerate() {
            let es = ents_scope.item(ei, ej);
            let id = ej.field("id").as_str().unwrap_or("?");
            let es = es.with_id(id);
            match ej.field("type").as_str() {
                Some("wall") => {
                    if let (Some(pts), Some(t)) = (read_pts(&mut rd, &es, "pts"), rd.f64_pos(&es, "thickness_m")) {
                        geo.walls.push((pts, t));
                    }
                }
                Some("door") => {
                    if let Some(a) = read_pts(&mut rd, &es, "pts") {
                        if a.len() == 2 {
                            geo.doors.push((a[0], a[1]));
                        } else {
                            rd.push(Issue::block("data.type").module(MODULE).path(&es.key_path("pts")).object(id).param("field", "pts"));
                        }
                    }
                }
                Some("room") => {
                    let Some(pts) = read_pts(&mut rd, &es, "pts") else { continue };
                    if pts.len() < 3 {
                        rd.push(Issue::block("geo.polygon_too_few_points").module(MODULE).path(&es.key_path("pts")).object(id));
                        continue;
                    }
                    let hazard = rd.str(&es, "hazard_class");
                    let classes = rd.arr(&es, "fire_classes");
                    let (Some(hazard), Some(classes)) = (hazard, classes) else { continue };
                    if classes.is_empty() {
                        rd.push(Issue::block("ext.no_fire_classes").module(MODULE).path(&es.key_path("fire_classes")).object(id));
                        continue;
                    }
                    let cl: Vec<String> = classes.iter().filter_map(|c| c.as_str().map(|s| s.to_string())).collect();
                    if cl.len() != classes.len() {
                        rd.push(
                            Issue::block("data.type")
                                .module(MODULE)
                                .path(&es.key_path("fire_classes"))
                                .object(id)
                                .param("field", "fire_classes"),
                        );
                        continue;
                    }
                    if seen_ids.insert(id.to_string(), ei).is_some() {
                        rd.push(Issue::block("geo.duplicate_id").module(MODULE).path(&es.key_path("id")).object(id).param("id", id));
                        continue;
                    }
                    geo.rooms.push(rooms.len());
                    rooms.push(Room { id: id.to_string(), pts, hazard: hazard.to_string(), classes: cl });
                }
                _ => {}
            }
        }
        levels.push(geo);
    }

    // ---- unidades
    let mut units: Vec<Unit> = Vec::new();
    let units_scope = ext_root.child("units");
    let mut unit_ids: BTreeMap<String, ()> = BTreeMap::new();
    for (ui, uj) in units_j.iter().enumerate() {
        let us = units_scope.item(ui, uj);
        let Some(id) = rd.str(&us, "id") else { continue };
        let us = us.with_id(id);
        if unit_ids.insert(id.to_string(), ()).is_some() {
            rd.push(Issue::block("geo.duplicate_id").module(MODULE).path(&us.key_path("id")).object(id).param("id", id));
            continue;
        }
        let level = rd.str(&us, "level");
        let agent = rd.str(&us, "agent");
        let pos = read_pt(&mut rd, &us);
        let ratings_s = rd.obj(&us, "ratings");
        let (Some(level), Some(agent), Some(pos), Some(rs)) = (level, agent, pos, ratings_s) else { continue };
        if !levels.iter().any(|l| l.id == level) {
            rd.push(Issue::block("ext.unit_level_unknown").module(MODULE).path(&us.key_path("level")).object(id).param("level", level));
            continue;
        }
        let mut ratings = BTreeMap::new();
        if let Some(o) = rs.j.as_obj() {
            for (k, v) in o {
                match v.as_f64() {
                    Some(x) if x.is_finite() && x >= 0.0 => {
                        ratings.insert(k.clone(), x);
                    }
                    _ => rd.push(Issue::block("data.type").module(MODULE).path(&rs.key_path(k)).object(id).param("field", k.as_str())),
                }
            }
        }
        let mount = rd.opt_f64(&us, "mount_height_m");
        units.push(Unit { id: id.to_string(), level: level.to_string(), pos, agent: agent.to_string(), ratings, mount_height_m: mount });
    }
    if rd.has_block() {
        return CalcOutput::blocked("extinguishers", rd.issues, fp);
    }
    if rooms.is_empty() {
        rd.push(Issue::block("ext.no_zones").module(MODULE).path("site.levels"));
        return CalcOutput::blocked("extinguishers", rd.issues, fp);
    }

    // ---- análisis por nivel
    let mut issues: Vec<Issue> = Vec::new();
    let mut zones_json: Vec<Json> = Vec::new();
    let mut units_json: BTreeMap<String, Json> = BTreeMap::new();
    let mut suggestions_json: Vec<Json> = Vec::new();
    let mut total_uncovered = 0.0;
    let mut zones_ok = 0usize;
    let mut zones_bad = 0usize;
    let mut compat_cache: BTreeMap<(String, String), Result<bool, Issue>> = BTreeMap::new();
    let mut req_cache: BTreeMap<(String, String), Result<Req, Issue>> = BTreeMap::new();
    let mut n_cells_total = 0usize;

    for level in &levels {
        if level.rooms.is_empty() {
            continue;
        }
        let lunits: Vec<&Unit> = units.iter().filter(|u| u.level == level.id).collect();
        let Some(grid) = rasterize(level, &lunits, &rooms, step, &mut rd) else { continue };
        n_cells_total += grid.nx * grid.ny;
        // posición de cada unidad (con ajuste a celda libre)
        let mut snapped: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for u in &lunits {
            match grid.nearest_free(u.pos, 8) {
                Some(c) => {
                    if !grid.free(grid.cell_of(u.pos).map(|c| c.0).unwrap_or(0), grid.cell_of(u.pos).map(|c| c.1).unwrap_or(0)) {
                        issues.push(
                            Issue::warning("ext.unit_in_obstacle")
                                .module(MODULE)
                                .path(&format!("systems.extinguishers.units.{}", u.id))
                                .object(&u.id),
                        );
                    }
                    snapped.insert(u.id.clone(), c);
                }
                None => issues.push(
                    Issue::block("ext.unit_unreachable")
                        .module(MODULE)
                        .path(&format!("systems.extinguishers.units.{}", u.id))
                        .object(&u.id),
                ),
            }
            if let (Some(h), Some(mx)) = (u.mount_height_m, &max_mount) {
                if h > mx.value * (1.0 + 1e-9) {
                    issues.push(
                        Issue::error("ext.mount_height_exceeds")
                            .module(MODULE)
                            .path(&format!("systems.extinguishers.units.{}.mount_height_m", u.id))
                            .object(&u.id)
                            .param("height_m", crate::json::fmt_num(h))
                            .param("limit_m", crate::json::fmt_num(mx.value))
                            .reference(&mx.reference),
                    );
                }
            }
        }
        for &ri in &level.rooms {
            let room = &rooms[ri];
            let cells = zone_cells(&grid, &room.pts);
            let zone_area = geometry::area(&room.pts);
            let mut classes_json: Vec<Json> = Vec::new();
            let mut zone_ok = true;
            if cells.is_empty() {
                issues.push(
                    Issue::block("ext.zone_without_free_cells")
                        .module(MODULE)
                        .path(&format!("site.levels.{}.entities", level.id))
                        .object(&room.id),
                );
                continue;
            }
            for class in &room.classes {
                // requisitos del perfil
                let key = (room.hazard.clone(), class.clone());
                let req = req_cache.entry(key.clone()).or_insert_with(|| {
                    let k = [("hazard_class", key.0.as_str()), ("fire_class", key.1.as_str())];
                    let max_travel = profile.lookup_in("extinguishers.max_travel_distance", &k, "m")?;
                    let min_rating = match profile.lookup("extinguishers.min_rating", &k) {
                        Ok(p) => Some(p),
                        Err(i) if i.code == "profile.table_missing" => None,
                        Err(i) => return Err(i),
                    };
                    let area = match basis {
                        AreaBasis::None => None,
                        AreaBasis::PerUnit => Some(profile.lookup_in("extinguishers.max_area_per_unit", &k, "m2")?),
                        AreaBasis::PerRating => Some(profile.lookup_in("extinguishers.max_area_per_rating", &k, "m2")?),
                    };
                    Ok(Req { max_travel, min_rating, area })
                });
                let req = match req {
                    Ok(r) => r,
                    Err(i) => {
                        issues.push(i.clone().object(&room.id));
                        zone_ok = false;
                        continue;
                    }
                };
                for p in [Some(&req.max_travel), req.min_rating.as_ref(), req.area.as_ref()].into_iter().flatten() {
                    if !used.iter().any(|u| u.path == p.path) {
                        used.push(p.clone());
                    }
                }
                // unidades que sirven a esta clase
                let mut serving: Vec<&Unit> = Vec::new();
                for u in &lunits {
                    let ck = (u.agent.clone(), class.clone());
                    let compat = compat_cache.entry(ck.clone()).or_insert_with(|| {
                        let p = profile.lookup("extinguishers.compat", &[("agent", ck.0.as_str()), ("fire_class", ck.1.as_str())])?;
                        if !used.iter().any(|x| x.path == p.path) {
                            used.push(p.clone());
                        }
                        Ok(p.value > 0.5)
                    });
                    match compat {
                        Ok(true) => {
                            let r = u.ratings.get(class).copied();
                            let min = req.min_rating.as_ref().map(|p| p.value);
                            let ok = match (r, min) {
                                (Some(r), Some(m)) => r >= m,
                                (Some(_), None) => true,
                                (None, _) => false,
                            };
                            if ok {
                                serving.push(u);
                            }
                        }
                        Ok(false) => {
                            // Seguridad: agente no apto para una clase presente en la zona de la propia unidad.
                            let inside = geometry::point_in_polygon(u.pos, &room.pts);
                            if inside {
                                let w = Issue::warning("ext.unsuitable_agent_present")
                                    .module(MODULE)
                                    .path(&format!("systems.extinguishers.units.{}.agent", u.id))
                                    .object(&u.id)
                                    .param("agent", u.agent.as_str())
                                    .param("fire_class", class.as_str())
                                    .param("zone", room.id.as_str());
                                if !issues.contains(&w) {
                                    issues.push(w);
                                }
                            }
                        }
                        Err(i) => {
                            let ii = i.clone().object(&u.id);
                            if !issues.contains(&ii) {
                                issues.push(ii);
                            }
                        }
                    }
                }
                let sources: Vec<(usize, usize)> = serving.iter().filter_map(|u| snapped.get(&u.id).copied()).collect();
                let dist = grid.dijkstra(&sources, req.max_travel.value);
                let mut covered = 0usize;
                let mut uncovered_cells: Vec<(usize, usize)> = Vec::new();
                for &(i, j) in &cells {
                    if dist[j * grid.nx + i].is_finite() {
                        covered += 1;
                    } else {
                        uncovered_cells.push((i, j));
                    }
                }
                let cell_area = grid.step * grid.step;
                let unc_area = uncovered_cells.len() as f64 * cell_area;
                let frac = covered as f64 / cells.len() as f64;
                let mut ok = uncovered_cells.is_empty();
                if serving.is_empty() {
                    issues.push(
                        Issue::error("ext.no_serving_unit")
                            .module(MODULE)
                            .path(&format!("site.levels.{}.entities", level.id))
                            .object(&room.id)
                            .param("fire_class", class.as_str())
                            .param("hazard_class", room.hazard.as_str()),
                    );
                } else if !uncovered_cells.is_empty() {
                    issues.push(
                        Issue::error("ext.uncovered_area")
                            .module(MODULE)
                            .path(&format!("site.levels.{}.entities", level.id))
                            .object(&room.id)
                            .param("fire_class", class.as_str())
                            .param("uncovered_m2", crate::json::fmt_num(crate::units::round_half_away(unc_area, 2)))
                            .param("max_travel_m", crate::json::fmt_num(req.max_travel.value))
                            .reference(&req.max_travel.reference),
                    );
                }
                total_uncovered += unc_area;
                // criterio de área: solo unidades dentro de la zona
                let inside: Vec<&&Unit> = serving.iter().filter(|u| geometry::point_in_polygon(u.pos, &room.pts)).collect();
                let mut area_json = Json::Null;
                if let Some(a) = &req.area {
                    let (required, have) = match basis {
                        AreaBasis::PerUnit => ((zone_area / a.value - 1e-9).ceil().max(1.0), inside.len() as f64),
                        _ => {
                            let sum: f64 = inside.iter().filter_map(|u| u.ratings.get(class)).sum();
                            ((zone_area / a.value - 1e-9).ceil().max(1.0), sum)
                        }
                    };
                    let area_ok = have + 1e-9 >= required;
                    if !area_ok {
                        ok = false;
                        issues.push(
                            Issue::error("ext.insufficient_by_area")
                                .module(MODULE)
                                .path(&format!("site.levels.{}.entities", level.id))
                                .object(&room.id)
                                .param("fire_class", class.as_str())
                                .param("required", crate::json::fmt_num(required))
                                .param("have", crate::json::fmt_num(have))
                                .reference(&a.reference),
                        );
                    }
                    area_json = Json::obj()
                        .with("basis", Json::str(if basis == AreaBasis::PerUnit { "per_unit" } else { "per_rating" }))
                        .with("limit_m2", Json::num(a.value))
                        .with("required", Json::num(required))
                        .with("have", Json::num(have))
                        .with("ok", Json::Bool(area_ok));
                }
                if serving.is_empty() {
                    ok = false;
                }
                // propuesta voraz de ubicaciones
                let mut sugg_positions: Vec<Json> = Vec::new();
                if let Some(sg) = suggest {
                    if !uncovered_cells.is_empty() && suggest_applies(sg, class) {
                        let positions = greedy_suggest(
                            &grid,
                            room,
                            &cells,
                            &uncovered_cells,
                            &dist,
                            req.max_travel.value,
                            sg.field("max_units").as_f64().unwrap_or(20.0) as usize,
                        );
                        for p in positions {
                            sugg_positions.push(Json::obj().with("x", Json::num(p[0])).with("y", Json::num(p[1])));
                        }
                    }
                }
                if !sugg_positions.is_empty() {
                    suggestions_json.push(
                        Json::obj()
                            .with("zone", Json::str(room.id.as_str()))
                            .with("level", Json::str(level.id.as_str()))
                            .with("fire_class", Json::str(class.as_str()))
                            .with("positions", Json::Arr(sugg_positions))
                            .with("status", Json::str("proposal_requires_review")),
                    );
                }
                classes_json.push(
                    Json::obj()
                        .with("fire_class", Json::str(class.as_str()))
                        .with("max_travel_m", Json::num(req.max_travel.value))
                        .with("min_rating", Json::opt_num(req.min_rating.as_ref().map(|p| p.value)))
                        .with("serving_units", Json::Arr(serving.iter().map(|u| Json::str(u.id.as_str())).collect()))
                        .with("zone_cells", Json::num(cells.len() as f64))
                        .with("covered_fraction", Json::num(frac))
                        .with("uncovered_area_m2", Json::num(unc_area))
                        .with("uncovered_rects", Json::Arr(rects_of(&grid, &uncovered_cells)))
                        .with("area_check", area_json)
                        .with("ok", Json::Bool(ok)),
                );
                if !ok {
                    zone_ok = false;
                }
                for u in &serving {
                    let e = units_json
                        .entry(u.id.clone())
                        .or_insert_with(|| Json::obj().with("id", Json::str(u.id.as_str())).with("serves", Json::arr()));
                    if let Json::Obj(m) = e {
                        if let Some(Json::Arr(a)) = m.get_mut("serves") {
                            a.push(Json::obj().with("zone", Json::str(room.id.as_str())).with("fire_class", Json::str(class.as_str())));
                        }
                    }
                }
            }
            if zone_ok {
                zones_ok += 1;
            } else {
                zones_bad += 1;
            }
            zones_json.push(
                Json::obj()
                    .with("id", Json::str(room.id.as_str()))
                    .with("level", Json::str(level.id.as_str()))
                    .with("hazard_class", Json::str(room.hazard.as_str()))
                    .with("area_m2", Json::num(zone_area))
                    .with("classes", Json::Arr(classes_json))
                    .with("ok", Json::Bool(zone_ok)),
            );
        }
        // unidades fuera de toda zona
        for u in &lunits {
            let inside_any = level.rooms.iter().any(|&ri| geometry::point_in_polygon(u.pos, &rooms[ri].pts));
            if !inside_any {
                issues.push(
                    Issue::warning("ext.unit_outside_zones")
                        .module(MODULE)
                        .path(&format!("systems.extinguishers.units.{}", u.id))
                        .object(&u.id),
                );
            }
        }
    }
    issues.append(&mut rd.issues);
    trace.push(trace_step(
        "travel_distance",
        "d(x) = camino mínimo en rejilla (8 vecinos, sin cortar esquinas, muros bloqueados, puertas abiertas); cubierta si d <= D_max",
        Json::obj().with("grid_m", Json::num(step)).with("method", Json::str(profile.method("extinguishers.coverage").unwrap_or(""))),
        Json::obj().with("cells", Json::num(n_cells_total as f64)).with("conservative", Json::Bool(true)),
    ));
    let blocked = issues.iter().any(|i| i.severity == Severity::Block);
    if blocked {
        issues.sort_by(|a, b| b.severity.cmp(&a.severity).then_with(|| a.code.cmp(&b.code)).then_with(|| a.object.cmp(&b.object)));
        return CalcOutput {
            kind: "extinguishers".into(),
            status: Status::Blocked,
            values: Json::Null,
            trace,
            params_used: used,
            issues,
            fingerprint: fp,
        };
    }
    let values = Json::obj()
        .with(
            "analysis",
            Json::obj()
                .with("grid_m", Json::num(step))
                .with("conservative_octile_metric", Json::Bool(true))
                .with("area_counts_only_units_inside_zone", Json::Bool(true)),
        )
        .with("zones", Json::Arr(zones_json))
        .with("units", Json::Arr(units_json.into_values().collect()))
        .with("suggestions", Json::Arr(suggestions_json))
        .with(
            "summary",
            Json::obj()
                .with("zones_ok", Json::num(zones_ok as f64))
                .with("zones_with_findings", Json::num(zones_bad as f64))
                .with("units_total", Json::num(units.len() as f64))
                .with("uncovered_area_total_m2", Json::num(total_uncovered)),
        );
    issues.sort_by(|a, b| b.severity.cmp(&a.severity).then_with(|| a.code.cmp(&b.code)).then_with(|| a.object.cmp(&b.object)));
    CalcOutput { kind: "extinguishers".into(), status: Status::Ok, values, trace, params_used: used, issues, fingerprint: fp }
}

fn suggest_applies(sg: &Json, class: &str) -> bool {
    match sg.field("fire_classes").as_arr() {
        Some(a) => a.iter().any(|c| c.as_str() == Some(class)),
        None => true,
    }
}

/// Cobertura voraz: candidatos = celdas libres de la zona pegadas a un obstáculo o al borde.
/// Determinista (desempate por y, x).
fn greedy_suggest(
    g: &Grid,
    room: &Room,
    zone: &[(usize, usize)],
    uncovered: &[(usize, usize)],
    base: &[f64],
    dmax: f64,
    max_units: usize,
) -> Vec<Pt> {
    let in_zone: std::collections::BTreeSet<(usize, usize)> = zone.iter().cloned().collect();
    let mut cands: Vec<(usize, usize)> = Vec::new();
    for &(i, j) in zone {
        let mut near_wall = false;
        for (di, dj) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
            let (ni, nj) = (i as i64 + di, j as i64 + dj);
            if ni < 0
                || nj < 0
                || ni >= g.nx as i64
                || nj >= g.ny as i64
                || !g.free(ni as usize, nj as usize)
                || !in_zone.contains(&(ni as usize, nj as usize))
            {
                near_wall = true;
            }
        }
        if near_wall {
            cands.push((i, j));
        }
    }
    let _ = room;
    // limitar el número de candidatos para acotar el coste
    if cands.len() > 600 {
        let stride = cands.len().div_ceil(600);
        cands = cands.into_iter().step_by(stride).collect();
    }
    let mut remaining: std::collections::BTreeSet<(usize, usize)> = uncovered.iter().cloned().collect();
    let _ = base;
    let mut chosen: Vec<Pt> = Vec::new();
    while !remaining.is_empty() && chosen.len() < max_units {
        let mut best: Option<Candidate> = None;
        for &c in &cands {
            let d = g.dijkstra(&[c], dmax);
            let newly: Vec<(usize, usize)> = remaining.iter().filter(|&&(i, j)| d[j * g.nx + i].is_finite()).cloned().collect();
            let better = match &best {
                None => !newly.is_empty(),
                Some((bc, bn)) => newly.len() > bn.len() || (newly.len() == bn.len() && (c.1, c.0) < (bc.1, bc.0)),
            };
            if better {
                best = Some((c, newly));
            }
        }
        match best {
            Some((c, newly)) => {
                for n in newly {
                    remaining.remove(&n);
                }
                chosen.push(g.center(c.0, c.1));
            }
            None => break,
        }
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    pub fn demo_profile() -> Profile {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../profiles/demo/DEMO-NO-NORMATIVO_0.1.0.json");
        Profile::from_json(Json::parse(&fs::read_to_string(p).unwrap()).unwrap()).unwrap()
    }

    fn project(entities: &str, units: &str) -> Json {
        Json::parse(&format!(
            r#"{{"site":{{"levels":[{{"id":"L1","entities":[{e}]}}]}},
                 "systems":{{"extinguishers":{{"analysis":{{"grid_m":0.25}},"units":[{u}]}}}}}}"#,
            e = entities,
            u = units
        ))
        .unwrap()
    }

    fn room(id: &str, x0: f64, y0: f64, x1: f64, y1: f64, haz: &str, classes: &str) -> String {
        format!(
            r#"{{"id":"{id}","type":"room","pts":[[{x0},{y0}],[{x1},{y0}],[{x1},{y1}],[{x0},{y1}]],"hazard_class":"{haz}","fire_classes":{classes}}}"#
        )
    }

    fn unit(id: &str, x: f64, y: f64, agent: &str, ratings: &str) -> String {
        format!(r#"{{"id":"{id}","level":"L1","x":{x},"y":{y},"agent":"{agent}","ratings":{ratings}}}"#)
    }

    fn zone<'a>(v: &'a Json, id: &str) -> &'a Json {
        v.field("zones").items().iter().find(|z| z.field("id").as_str() == Some(id)).unwrap()
    }

    #[test]
    fn region_cubierta_coincide_con_el_octogono_octil_analitico() {
        // sala 20x20, una unidad en el centro, D_max (demo_high/A) = 10 m
        let p = project(&room("R1", 0.0, 0.0, 20.0, 20.0, "demo_high", r#"["A"]"#), &unit("E1", 10.0, 10.0, "water", r#"{"A":5}"#));
        let out = run(&p, &demo_profile(), None);
        assert_eq!(out.status, Status::Ok, "{:?}", out.issues);
        let z = zone(&out.values, "R1");
        let c = &z.field("classes").items()[0];
        let area = z.field("area_m2").as_f64().unwrap();
        let covered = (1.0 - c.field("uncovered_area_m2").as_f64().unwrap() / area) * area;
        // octógono octil: 2*sqrt(2)*R^2 = 282.8 m2 (R=10) frente al disco 314.2; cuantización 0.25 m
        let expect = 2.0 * 2f64.sqrt() * 100.0;
        assert!((covered - expect).abs() / expect < 0.03, "cubierta {} vs {}", covered, expect);
        assert!(covered < std::f64::consts::PI * 100.0, "debe ser conservador (menos que el disco)");
        assert_eq!(c.field("ok").as_bool(), Some(false));
        assert!(out.issues.iter().any(|i| i.code == "ext.uncovered_area" && i.object.as_deref() == Some("R1")));
    }

    #[test]
    fn el_recorrido_real_respeta_muros_y_puertas() {
        // Dos salas 10x10 unidas por una puerta alejada; unidad en la sala A cerca del muro común.
        let ents = format!(
            "{},{},{},{}",
            room("A", 0.0, 0.0, 10.0, 10.0, "demo_medium", r#"["A"]"#),
            room("B", 10.0, 0.0, 20.0, 10.0, "demo_medium", r#"["A"]"#),
            r#"{"id":"W1","type":"wall","pts":[[10,0],[10,10]],"thickness_m":0.2}"#,
            r#"{"id":"D1","type":"door","pts":[[10,8],[10,9]]}"#
        );
        // demo_medium/A: 15 m. Unidad en (9,1): B(11,1) está a 2 m en línea recta, pero por la puerta a ~16+ m.
        let p = project(&ents, &unit("E1", 9.0, 1.0, "water", r#"{"A":5}"#));
        let out = run(&p, &demo_profile(), None);
        assert_eq!(out.status, Status::Ok, "{:?}", out.issues);
        let zb = zone(&out.values, "B");
        let cb = &zb.field("classes").items()[0];
        assert!(
            cb.field("uncovered_area_m2").as_f64().unwrap() > 20.0,
            "la sala B debe quedar en gran parte sin cubrir por el recorrido real"
        );
        // Sin muro, la misma unidad cubriría (11,1): comprobación de contraste
        let ents2 = format!(
            "{},{}",
            room("A", 0.0, 0.0, 10.0, 10.0, "demo_medium", r#"["A"]"#),
            room("B", 10.0, 0.0, 20.0, 10.0, "demo_medium", r#"["A"]"#)
        );
        let out2 = run(&project(&ents2, &unit("E1", 9.0, 1.0, "water", r#"{"A":5}"#)), &demo_profile(), None);
        let ub = zone(&out2.values, "B").field("classes").items()[0].field("uncovered_area_m2").as_f64().unwrap();
        assert!(ub < cb.field("uncovered_area_m2").as_f64().unwrap() - 15.0);
        // el rectángulo de la puerta está cubierto en la sala A (unidad a ~7 m de la puerta)
        let za = zone(&out.values, "A");
        assert_eq!(za.field("classes").items()[0].field("uncovered_area_m2").as_f64(), Some(0.0));
    }

    #[test]
    fn compatibilidad_agente_clase_viene_de_la_tabla_del_perfil() {
        // water no sirve a C en el perfil de demostración
        let p = project(&room("R1", 0.0, 0.0, 6.0, 6.0, "demo_low", r#"["C"]"#), &unit("E1", 3.0, 3.0, "water", r#"{"C":5}"#));
        let out = run(&p, &demo_profile(), None);
        assert_eq!(out.status, Status::Ok, "{:?}", out.issues);
        assert!(out.issues.iter().any(|i| i.code == "ext.no_serving_unit" && i.object.as_deref() == Some("R1")));
        assert!(out.issues.iter().any(|i| i.code == "ext.unsuitable_agent_present"
            && i.object.as_deref() == Some("E1")
            && i.params.get("fire_class").map(String::as_str) == Some("C")));
        // con polvo ABC sí sirve
        let p = project(&room("R1", 0.0, 0.0, 6.0, 6.0, "demo_low", r#"["C"]"#), &unit("E1", 3.0, 3.0, "powder_abc", r#"{"C":5}"#));
        let out = run(&p, &demo_profile(), None);
        assert!(!out.issues.iter().any(|i| i.severity >= Severity::Error), "{:?}", out.issues);
    }

    #[test]
    fn agente_sin_fila_de_compatibilidad_bloquea() {
        let p = project(&room("R1", 0.0, 0.0, 6.0, 6.0, "demo_low", r#"["A"]"#), &unit("E1", 3.0, 3.0, "agente_raro", r#"{"A":5}"#));
        let out = run(&p, &demo_profile(), None);
        assert_eq!(out.status, Status::Blocked);
        assert!(out.issues.iter().any(|i| i.code == "profile.table_no_row" && i.object.as_deref() == Some("E1")));
    }

    #[test]
    fn la_clasificacion_minima_excluye_unidades_insuficientes() {
        // demo_high/A exige min_rating mayor que 1 (ver perfil): una unidad con A=1 no sirve
        let p = project(&room("R1", 0.0, 0.0, 6.0, 6.0, "demo_high", r#"["A"]"#), &unit("E1", 3.0, 3.0, "water", r#"{"A":1}"#));
        let out = run(&p, &demo_profile(), None);
        assert!(out.issues.iter().any(|i| i.code == "ext.no_serving_unit"), "{:?}", out.issues);
    }

    #[test]
    fn datos_faltantes_bloquean() {
        // sala sin clase de riesgo
        let e = r#"{"id":"R1","type":"room","pts":[[0,0],[5,0],[5,5],[0,5]],"fire_classes":["A"]}"#;
        let out = run(&project(e, &unit("E1", 2.0, 2.0, "water", r#"{"A":5}"#)), &demo_profile(), None);
        assert_eq!(out.status, Status::Blocked);
        let i = out.issues.iter().find(|i| i.code == "data.missing").unwrap();
        assert_eq!(i.object.as_deref(), Some("R1"));
        assert!(i.path.ends_with("hazard_class"));
        // sala sin clases de fuego
        let e = r#"{"id":"R1","type":"room","pts":[[0,0],[5,0],[5,5],[0,5]],"hazard_class":"demo_low","fire_classes":[]}"#;
        let out = run(&project(e, ""), &demo_profile(), None);
        assert!(out.issues.iter().any(|i| i.code == "ext.no_fire_classes"));
        // sin la rejilla de análisis
        let mut p = project(&room("R1", 0.0, 0.0, 6.0, 6.0, "demo_low", r#"["A"]"#), "");
        p.set_at("systems.extinguishers.analysis", Json::obj());
        let out = run(&p, &demo_profile(), None);
        assert!(out.issues.iter().any(|i| i.code == "data.missing" && i.path.ends_with("grid_m")));
    }

    #[test]
    fn criterio_de_area_por_unidad() {
        // demo_low/A: 100 m2 por unidad según el perfil de demostración; sala de 250 m2 => 3 unidades
        let r = room("R1", 0.0, 0.0, 25.0, 10.0, "demo_low", r#"["A"]"#);
        let units = format!("{},{}", unit("E1", 5.0, 5.0, "water", r#"{"A":5}"#), unit("E2", 20.0, 5.0, "water", r#"{"A":5}"#));
        let out = run(&project(&r, &units), &demo_profile(), None);
        assert_eq!(out.status, Status::Ok, "{:?}", out.issues);
        let c = &zone(&out.values, "R1").field("classes").items()[0];
        let a = c.field("area_check");
        assert_eq!(a.field("required").as_f64(), Some(3.0));
        assert_eq!(a.field("have").as_f64(), Some(2.0));
        let i = out.issues.iter().find(|i| i.code == "ext.insufficient_by_area").expect("debe indicar el faltante");
        assert_eq!(i.params.get("required").map(String::as_str), Some("3"));
        assert!(i.reference.as_deref().unwrap_or("").starts_with("DEMO"));
    }

    #[test]
    fn la_propuesta_voraz_logra_cobertura_total_y_es_determinista() {
        let r = room("R1", 0.0, 0.0, 24.0, 16.0, "demo_medium", r#"["A"]"#);
        let sg = Json::parse(r#"{"fire_classes":["A"],"max_units":20}"#).unwrap();
        let p = project(&r, "");
        let out = run(&p, &demo_profile(), Some(&sg));
        assert_eq!(out.status, Status::Ok, "{:?}", out.issues);
        let s = out.values.field("suggestions").items();
        assert_eq!(s.len(), 1);
        let pos = s[0].field("positions").items();
        assert!(!pos.is_empty() && pos.len() <= 6, "{} unidades propuestas", pos.len());
        assert_eq!(s[0].field("status").as_str(), Some("proposal_requires_review"));
        // aplicar la propuesta debe cubrir toda la sala
        let mut units = String::new();
        for (k, q) in pos.iter().enumerate() {
            if k > 0 {
                units.push(',');
            }
            units.push_str(&unit(
                &format!("S{}", k),
                q.field("x").as_f64().unwrap(),
                q.field("y").as_f64().unwrap(),
                "water",
                r#"{"A":5}"#,
            ));
        }
        let out2 = run(&project(&r, &units), &demo_profile(), None);
        let unc = zone(&out2.values, "R1").field("classes").items()[0].field("uncovered_area_m2").as_f64().unwrap();
        assert_eq!(unc, 0.0, "la propuesta no cubrió toda la sala");
        // determinismo
        let again = run(&p, &demo_profile(), Some(&sg));
        assert_eq!(out.values.canonical(), again.values.canonical());
        assert_eq!(out.fingerprint, again.fingerprint);
    }

    #[test]
    fn altura_de_montaje_y_unidad_fuera_de_zona() {
        let r = room("R1", 0.0, 0.0, 6.0, 6.0, "demo_low", r#"["A"]"#);
        let units = format!(
            r#"{{"id":"E1","level":"L1","x":3,"y":3,"agent":"water","ratings":{{"A":5}},"mount_height_m":9}},{}"#,
            unit("E2", 30.0, 30.0, "water", r#"{"A":5}"#)
        );
        let out = run(&project(&r, &units), &demo_profile(), None);
        assert!(out.issues.iter().any(|i| i.code == "ext.mount_height_exceeds" && i.object.as_deref() == Some("E1")), "{:?}", out.issues);
        assert!(out.issues.iter().any(|i| i.code == "ext.unit_outside_zones" && i.object.as_deref() == Some("E2")));
    }

    #[test]
    fn rejilla_demasiado_grande_se_rechaza() {
        let r = room("R1", 0.0, 0.0, 5000.0, 5000.0, "demo_low", r#"["A"]"#);
        let out = run(&project(&r, ""), &demo_profile(), None);
        assert!(out.issues.iter().any(|i| i.code == "ext.grid_too_large"), "{:?}", out.issues);
        assert_eq!(out.status, Status::Blocked);
    }
}
