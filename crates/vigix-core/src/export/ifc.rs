//! Exportación IFC4 (STEP, ISO 10303-21) de la geometría y los objetos del proyecto (BIM básico).
//!
//! Contenido: `IfcProject > IfcSite > IfcBuilding > IfcBuildingStorey`; zonas como `IfcSpace` con
//! huella extruida; rociadores/boquillas/hidrantes/tomas como `IfcFireSuppressionTerminal`;
//! extintores portátiles como terminal de tipo definido por el usuario; tuberías como
//! `IfcPipeSegment` con perfil circular extruido; conjuntos de propiedades `Pset_VIGIX` con id,
//! sistema, estado del documento, perfil y huella de datos.
//!
//! Alcance honesto: es un intercambio **geométrico y de atributos** para coordinación; no
//! incluye tipos de producto del fabricante, conexiones de puertos (`IfcRelConnectsPorts`) ni
//! sistemas (`IfcSystem`). Los GlobalId son deterministas (derivados de SHA-256).

use crate::geometry::Pt;
use crate::hash::sha256;
use crate::json::{fmt_num, Json};

pub struct IfcMeta {
    pub project_name: String,
    pub author: String,
    pub status_text: String,
    pub profile: String,
    pub profile_verified: bool,
    pub fingerprint: String,
    /// `AAAA-MM-DDThh:mm:ss` (UTC) para `FILE_NAME`.
    pub timestamp: String,
}

/// Escapa texto STEP: apóstrofo duplicado, barra invertida duplicada y no-ASCII como `\X2\....\X0\`.
pub fn step_str(s: &str) -> String {
    let mut o = String::from("'");
    let mut pending: Vec<u16> = Vec::new();
    let flush = |o: &mut String, pending: &mut Vec<u16>| {
        if !pending.is_empty() {
            o.push_str("\\X2\\");
            for u in pending.iter() {
                o.push_str(&format!("{:04X}", u));
            }
            o.push_str("\\X0\\");
            pending.clear();
        }
    };
    for c in s.chars() {
        if (c as u32) < 0x20 || (c as u32) > 0x7E {
            let mut b = [0u16; 2];
            for u in c.encode_utf16(&mut b) {
                pending.push(*u);
            }
            continue;
        }
        flush(&mut o, &mut pending);
        match c {
            '\'' => o.push_str("''"),
            '\\' => o.push_str("\\\\"),
            c => o.push(c),
        }
    }
    flush(&mut o, &mut pending);
    o.push('\'');
    o
}

/// Número real STEP (siempre con punto decimal).
fn r(x: f64) -> String {
    let x = if x.is_finite() { x } else { 0.0 };
    let s = fmt_num(x);
    if s.contains('.') || s.contains('e') || s.contains('E') {
        if s.contains('e') {
            // notación científica de IFC: 1.E-5
            let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
            let m = if m.contains('.') { m.to_string() } else { format!("{}.", m) };
            return format!("{}E{}", m, e);
        }
        s
    } else {
        format!("{}.", s)
    }
}

/// GlobalId de IFC (22 caracteres) derivado de forma determinista de `seed`.
pub fn guid(seed: &str) -> String {
    const A: &[u8; 64] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_$";
    let h = sha256(seed.as_bytes());
    let mut n: u128 = 0;
    for b in &h[..16] {
        n = (n << 8) | *b as u128;
    }
    let mut s = [0u8; 22];
    // 128 bits = 2 bits + 21 * 6 bits
    s[0] = A[((n >> 126) & 0x3) as usize];
    for (i, slot) in s.iter_mut().enumerate().skip(1) {
        let shift = 126 - 6 * i as u32;
        *slot = A[((n >> shift) & 0x3F) as usize];
    }
    String::from_utf8_lossy(&s).into_owned()
}

struct B {
    lines: Vec<String>,
}

impl B {
    fn e(&mut self, body: &str) -> usize {
        self.lines.push(body.to_string());
        self.lines.len()
    }
    fn pt3(&mut self, x: f64, y: f64, z: f64) -> usize {
        self.e(&format!("IFCCARTESIANPOINT(({},{},{}))", r(x), r(y), r(z)))
    }
    fn pt2(&mut self, x: f64, y: f64) -> usize {
        self.e(&format!("IFCCARTESIANPOINT(({},{}))", r(x), r(y)))
    }
    fn dir3(&mut self, x: f64, y: f64, z: f64) -> usize {
        self.e(&format!("IFCDIRECTION(({},{},{}))", r(x), r(y), r(z)))
    }
    fn axis3(&mut self, loc: (f64, f64, f64), z: (f64, f64, f64), xdir: (f64, f64, f64)) -> usize {
        let p = self.pt3(loc.0, loc.1, loc.2);
        let zd = self.dir3(z.0, z.1, z.2);
        let xd = self.dir3(xdir.0, xdir.1, xdir.2);
        self.e(&format!("IFCAXIS2PLACEMENT3D(#{},#{},#{})", p, zd, xd))
    }
    fn local_placement(&mut self, loc: (f64, f64, f64), z: (f64, f64, f64), xdir: (f64, f64, f64), rel: Option<usize>) -> usize {
        let a = self.axis3(loc, z, xdir);
        match rel {
            Some(p) => self.e(&format!("IFCLOCALPLACEMENT(#{},#{})", p, a)),
            None => self.e(&format!("IFCLOCALPLACEMENT($,#{})", a)),
        }
    }
}

fn norm(v: (f64, f64, f64)) -> (f64, f64, f64) {
    let l = (v.0 * v.0 + v.1 * v.1 + v.2 * v.2).sqrt();
    if l < 1e-12 {
        (0.0, 0.0, 1.0)
    } else {
        (v.0 / l, v.1 / l, v.2 / l)
    }
}

fn perpendicular(z: (f64, f64, f64)) -> (f64, f64, f64) {
    // un vector unitario no paralelo a z, proyectado
    let a = if z.2.abs() < 0.9 { (0.0, 0.0, 1.0) } else { (1.0, 0.0, 0.0) };
    let dot = a.0 * z.0 + a.1 * z.1 + a.2 * z.2;
    norm((a.0 - dot * z.0, a.1 - dot * z.1, a.2 - dot * z.2))
}

fn pts_of(j: &Json) -> Vec<Pt> {
    j.items().iter().filter_map(|p| Some([p.idx(0)?.as_f64()?, p.idx(1)?.as_f64()?])).collect()
}

/// Construye el IFC. Devuelve (texto, elementos omitidos con motivo).
pub fn build(project: &Json, meta: &IfcMeta) -> (String, Vec<String>) {
    let mut b = B { lines: Vec::new() };
    let mut omitted: Vec<String> = Vec::new();
    let person = b.e(&format!("IFCPERSON($,$,{},$,$,$,$,$)", step_str(&meta.author)));
    let org = b.e("IFCORGANIZATION($,'VIGIX',$,$,$)");
    let po = b.e(&format!("IFCPERSONANDORGANIZATION(#{},#{},$)", person, org));
    let app = b.e(&format!("IFCAPPLICATION(#{},{},'VIGIX EXTINCION FIJA','VIGIX')", org, step_str(crate::ENGINE_VERSION)));
    let owner = b.e(&format!("IFCOWNERHISTORY(#{},#{},$,.NOCHANGE.,$,$,$,0)", po, app));
    let origin = b.axis3((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), (1.0, 0.0, 0.0));
    let ctx = b.e(&format!("IFCGEOMETRICREPRESENTATIONCONTEXT($,'Model',3,1.E-05,#{},$)", origin));
    let u1 = b.e("IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.)");
    let u2 = b.e("IFCSIUNIT(*,.AREAUNIT.,$,.SQUARE_METRE.)");
    let u3 = b.e("IFCSIUNIT(*,.VOLUMEUNIT.,$,.CUBIC_METRE.)");
    let u4 = b.e("IFCSIUNIT(*,.PLANEANGLEUNIT.,$,.RADIAN.)");
    let units = b.e(&format!("IFCUNITASSIGNMENT((#{},#{},#{},#{}))", u1, u2, u3, u4));
    let pid = project.at("meta.id").and_then(Json::as_str).unwrap_or("P");
    let proj = b.e(&format!(
        "IFCPROJECT({},#{},{},$,$,$,$,(#{}),#{})",
        step_str(&guid(&format!("{}|project", pid))),
        owner,
        step_str(&meta.project_name),
        ctx,
        units
    ));
    let site_pl = b.local_placement((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), (1.0, 0.0, 0.0), None);
    let site =
        b.e(&format!("IFCSITE({},#{},'Sitio',$,$,#{},$,$,.ELEMENT.,$,$,$,$,$)", step_str(&guid(&format!("{}|site", pid))), owner, site_pl));
    let bld_pl = b.local_placement((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), (1.0, 0.0, 0.0), Some(site_pl));
    let bld = b.e(&format!(
        "IFCBUILDING({},#{},'Edificio',$,$,#{},$,$,.ELEMENT.,$,$,$)",
        step_str(&guid(&format!("{}|building", pid))),
        owner,
        bld_pl
    ));
    let mut rel_agg: Vec<(usize, Vec<usize>)> = vec![(proj, vec![site]), (site, vec![bld])];

    // niveles
    let mut storeys: Vec<(String, usize, f64, f64)> = Vec::new(); // (id, ifc id, elevación, altura)
    for l in project.at("site.levels").map(Json::items).unwrap_or(&[]) {
        let lid = l.field("id").as_str().unwrap_or("L");
        let z = l.field("elevation_m").as_f64().unwrap_or(0.0);
        let h = l.field("height_m").as_f64().unwrap_or(3.0);
        let pl = b.local_placement((0.0, 0.0, z), (0.0, 0.0, 1.0), (1.0, 0.0, 0.0), Some(bld_pl));
        let s = b.e(&format!(
            "IFCBUILDINGSTOREY({},#{},{},$,$,#{},$,$,.ELEMENT.,{})",
            step_str(&guid(&format!("{}|storey|{}", pid, lid))),
            owner,
            step_str(l.field("name").as_str().unwrap_or(lid)),
            pl,
            r(z)
        ));
        storeys.push((lid.to_string(), s, z, h));
    }
    rel_agg.push((bld, storeys.iter().map(|s| s.1).collect()));

    let status = step_str(&meta.status_text);
    let mut contained: Vec<(usize, Vec<usize>)> = storeys.iter().map(|s| (s.1, vec![])).collect();
    let mut psets: Vec<(usize, Vec<(String, String)>)> = Vec::new(); // elemento -> (nombre, valor IFC)
    let storey_ix = |id: &str| storeys.iter().position(|s| s.0 == id);
    let props = |id: &str, system: &str, extra: Vec<(String, String)>| -> Vec<(String, String)> {
        let mut v = vec![
            ("VigixId".to_string(), format!("IFCLABEL({})", step_str(id))),
            ("System".to_string(), format!("IFCLABEL({})", step_str(system))),
            ("DocumentStatus".to_string(), format!("IFCTEXT({})", status)),
            ("Profile".to_string(), format!("IFCLABEL({})", step_str(&meta.profile))),
            ("ProfileVerified".to_string(), format!("IFCBOOLEAN({})", if meta.profile_verified { ".T." } else { ".F." })),
            ("DataFingerprintSHA256".to_string(), format!("IFCLABEL({})", step_str(&meta.fingerprint))),
        ];
        v.extend(extra);
        v
    };

    // zonas (IfcSpace) y muros (IfcWall con huella extruida)
    for l in project.at("site.levels").map(Json::items).unwrap_or(&[]) {
        let lid = l.field("id").as_str().unwrap_or("L");
        let Some(si) = storey_ix(lid) else { continue };
        let (_, _, z0, height) = storeys[si].clone();
        for e in l.field("entities").items() {
            let id = e.field("id").as_str().unwrap_or("?");
            match e.field("type").as_str() {
                Some("room") => {
                    let pts = pts_of(e.field("pts"));
                    if pts.len() < 3 {
                        omitted.push(format!("room {}: menos de 3 puntos", id));
                        continue;
                    }
                    let mut ids: Vec<usize> = pts.iter().map(|p| b.pt2(p[0], p[1])).collect();
                    ids.push(ids[0]);
                    let poly = b.e(&format!("IFCPOLYLINE(({}))", ids.iter().map(|i| format!("#{}", i)).collect::<Vec<_>>().join(",")));
                    let prof = b.e(&format!("IFCARBITRARYCLOSEDPROFILEDEF(.AREA.,$,#{})", poly));
                    let pos = b.axis3((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), (1.0, 0.0, 0.0));
                    let dir = b.dir3(0.0, 0.0, 1.0);
                    let solid = b.e(&format!("IFCEXTRUDEDAREASOLID(#{},#{},#{},{})", prof, pos, dir, r(height)));
                    let rep = b.e(&format!("IFCSHAPEREPRESENTATION(#{},'Body','SweptSolid',(#{}))", ctx, solid));
                    let pds = b.e(&format!("IFCPRODUCTDEFINITIONSHAPE($,$,(#{}))", rep));
                    let name = e.field("name").as_str().filter(|s| !s.is_empty()).unwrap_or(id);
                    // placement relativo al nivel
                    let spl = b.local_placement((0.0, 0.0, z0), (0.0, 0.0, 1.0), (1.0, 0.0, 0.0), Some(bld_pl));
                    let sp = b.e(&format!(
                        "IFCSPACE({},#{},{},{},$,#{},#{},$,.ELEMENT.,.INTERNAL.,$)",
                        step_str(&guid(&format!("{}|room|{}", pid, id))),
                        owner,
                        step_str(name),
                        step_str(&format!("hazard={}", e.field("hazard_class").as_str().unwrap_or("-"))),
                        spl,
                        pds
                    ));
                    contained[si].1.push(sp);
                    psets.push((
                        sp,
                        props(
                            id,
                            "zone",
                            vec![(
                                "HazardClass".to_string(),
                                format!("IFCLABEL({})", step_str(e.field("hazard_class").as_str().unwrap_or(""))),
                            )],
                        ),
                    ));
                }
                Some("wall") => {
                    let pts = pts_of(e.field("pts"));
                    let t = e.field("thickness_m").as_f64().unwrap_or(0.2).max(0.01);
                    for (k, seg) in pts.windows(2).enumerate() {
                        let (a, c) = (seg[0], seg[1]);
                        let len = ((c[0] - a[0]).powi(2) + (c[1] - a[1]).powi(2)).sqrt();
                        if len < 1e-6 {
                            continue;
                        }
                        let (dx, dy) = ((c[0] - a[0]) / len, (c[1] - a[1]) / len);
                        // rectángulo len x t centrado en el eje
                        let corners = [(0.0, -t / 2.0), (len, -t / 2.0), (len, t / 2.0), (0.0, t / 2.0), (0.0, -t / 2.0)];
                        let ids: Vec<usize> = corners.iter().map(|p| b.pt2(p.0, p.1)).collect();
                        let poly = b.e(&format!("IFCPOLYLINE(({}))", ids.iter().map(|i| format!("#{}", i)).collect::<Vec<_>>().join(",")));
                        let prof = b.e(&format!("IFCARBITRARYCLOSEDPROFILEDEF(.AREA.,$,#{})", poly));
                        let pos = b.axis3((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), (1.0, 0.0, 0.0));
                        let dir = b.dir3(0.0, 0.0, 1.0);
                        let solid = b.e(&format!("IFCEXTRUDEDAREASOLID(#{},#{},#{},{})", prof, pos, dir, r(height)));
                        let rep = b.e(&format!("IFCSHAPEREPRESENTATION(#{},'Body','SweptSolid',(#{}))", ctx, solid));
                        let pds = b.e(&format!("IFCPRODUCTDEFINITIONSHAPE($,$,(#{}))", rep));
                        let wpl = b.local_placement((a[0], a[1], z0), (0.0, 0.0, 1.0), (dx, dy, 0.0), Some(bld_pl));
                        let wid = format!("{}#{}", id, k + 1);
                        let w = b.e(&format!(
                            "IFCWALL({},#{},{},$,$,#{},#{},$,.NOTDEFINED.)",
                            step_str(&guid(&format!("{}|wall|{}", pid, wid))),
                            owner,
                            step_str(&wid),
                            wpl,
                            pds
                        ));
                        contained[si].1.push(w);
                        psets.push((
                            w,
                            props(&wid, "architecture", vec![("ThicknessM".to_string(), format!("IFCLENGTHMEASURE({})", r(t)))]),
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    // extintores portátiles
    for u in project.at("systems.extinguishers.units").map(Json::items).unwrap_or(&[]) {
        let id = u.field("id").as_str().unwrap_or("?");
        let (Some(si), Some(x), Some(y)) = (u.field("level").as_str().and_then(storey_ix), u.field("x").as_f64(), u.field("y").as_f64())
        else {
            omitted.push(format!("extintor {}: sin nivel o coordenadas", id));
            continue;
        };
        let z = storeys[si].2 + u.field("mount_height_m").as_f64().unwrap_or(1.0);
        let ent = terminal(&mut b, ctx, owner, bld_pl, pid, id, (x, y, z), "USERDEFINED", "FIRE_EXTINGUISHER_PORTABLE");
        contained[si].1.push(ent);
        psets.push((
            ent,
            props(
                id,
                "portable_extinguishers",
                vec![("Agent".to_string(), format!("IFCLABEL({})", step_str(u.field("agent").as_str().unwrap_or(""))))],
            ),
        ));
    }

    // redes de agua: dispositivos y tuberías
    for net in project.at("systems.water.networks").map(Json::items).unwrap_or(&[]) {
        let st = net.field("system_type").as_str().unwrap_or("water");
        let mut pos: std::collections::BTreeMap<String, (usize, f64, f64, f64)> = std::collections::BTreeMap::new();
        for n in net.field("nodes").items() {
            let id = n.field("id").as_str().unwrap_or("?");
            let (Some(x), Some(y), Some(z)) = (n.field("x").as_f64(), n.field("y").as_f64(), n.field("elevation_m").as_f64()) else {
                omitted.push(format!("nodo {}: sin coordenadas x,y o cota", id));
                continue;
            };
            let si = n.field("level").as_str().and_then(storey_ix).unwrap_or(0);
            if storeys.is_empty() {
                continue;
            }
            pos.insert(id.to_string(), (si, x, y, z));
            let kind = n.field("kind").as_str().unwrap_or("junction");
            let (ptype, otype) = match kind {
                "sprinkler" => ("SPRINKLER", ""),
                "hose_valve" => ("HOSEREEL", ""),
                "hydrant" => ("FIREHYDRANT", ""),
                "nozzle" => ("USERDEFINED", "NOZZLE"),
                "supply" => ("USERDEFINED", "WATER_SUPPLY_CONNECTION"),
                "tank" | "reservoir" => ("USERDEFINED", "WATER_TANK"),
                _ => continue,
            };
            let ent = terminal(&mut b, ctx, owner, bld_pl, pid, id, (x, y, z), ptype, otype);
            contained[si].1.push(ent);
            let mut extra = vec![("NodeKind".to_string(), format!("IFCLABEL({})", step_str(kind)))];
            if let Some(k) = n.field("k_lpm_bar05").as_f64() {
                extra.push(("KFactor_Lpm_per_sqrt_bar".to_string(), format!("IFCREAL({})", r(k))));
            }
            psets.push((ent, props(id, st, extra)));
        }
        for l in net.field("links").items() {
            let id = l.field("id").as_str().unwrap_or("?");
            if !matches!(l.field("kind").as_str(), Some("pipe") | Some("valve")) {
                continue;
            }
            let (Some(a), Some(c)) = (l.field("from").as_str().and_then(|f| pos.get(f)), l.field("to").as_str().and_then(|t| pos.get(t)))
            else {
                omitted.push(format!("tramo {}: extremos sin coordenadas", id));
                continue;
            };
            let d = l.field("diameter_mm").as_f64().unwrap_or(0.0);
            let v = (c.1 - a.1, c.2 - a.2, c.3 - a.3);
            let len = (v.0 * v.0 + v.1 * v.1 + v.2 * v.2).sqrt();
            if len < 1e-6 || d <= 0.0 {
                omitted.push(format!("tramo {}: longitud o diámetro nulos", id));
                continue;
            }
            let z = norm(v);
            let xd = perpendicular(z);
            let o2 = b.pt2(0.0, 0.0);
            let ap2 = b.e(&format!("IFCAXIS2PLACEMENT2D(#{},$)", o2));
            let prof = b.e(&format!("IFCCIRCLEPROFILEDEF(.AREA.,$,#{},{})", ap2, r(d / 2000.0)));
            let pos3 = b.axis3((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), (1.0, 0.0, 0.0));
            let dir = b.dir3(0.0, 0.0, 1.0);
            let solid = b.e(&format!("IFCEXTRUDEDAREASOLID(#{},#{},#{},{})", prof, pos3, dir, r(len)));
            let rep = b.e(&format!("IFCSHAPEREPRESENTATION(#{},'Body','SweptSolid',(#{}))", ctx, solid));
            let pds = b.e(&format!("IFCPRODUCTDEFINITIONSHAPE($,$,(#{}))", rep));
            let pl = b.local_placement((a.1, a.2, a.3), z, xd, Some(bld_pl));
            let pipe = b.e(&format!(
                "IFCPIPESEGMENT({},#{},{},$,$,#{},#{},$,.RIGIDSEGMENT.)",
                step_str(&guid(&format!("{}|pipe|{}", pid, id))),
                owner,
                step_str(id),
                pl,
                pds
            ));
            contained[a.0].1.push(pipe);
            let extra = vec![
                ("NominalDiameterMm".to_string(), format!("IFCLENGTHMEASURE({})", r(d))),
                ("LengthM".to_string(), format!("IFCLENGTHMEASURE({})", r(len))),
                ("CFactor".to_string(), format!("IFCREAL({})", r(l.field("c_factor").as_f64().unwrap_or(0.0)))),
            ];
            psets.push((pipe, props(id, st, extra)));
        }
    }

    // relaciones
    for (parent, kids) in &rel_agg {
        if kids.is_empty() {
            continue;
        }
        b.e(&format!(
            "IFCRELAGGREGATES({},#{},$,$,#{},({}))",
            step_str(&guid(&format!("{}|agg|{}", pid, parent))),
            owner,
            parent,
            kids.iter().map(|k| format!("#{}", k)).collect::<Vec<_>>().join(",")
        ));
    }
    for (storey, els) in &contained {
        if els.is_empty() {
            continue;
        }
        b.e(&format!(
            "IFCRELCONTAINEDINSPATIALSTRUCTURE({},#{},$,$,({}),#{})",
            step_str(&guid(&format!("{}|cont|{}", pid, storey))),
            owner,
            els.iter().map(|k| format!("#{}", k)).collect::<Vec<_>>().join(","),
            storey
        ));
    }
    for (el, ps) in &psets {
        let mut ids = Vec::new();
        for (name, val) in ps {
            ids.push(b.e(&format!("IFCPROPERTYSINGLEVALUE({},$,{},$)", step_str(name), val)));
        }
        let pset = b.e(&format!(
            "IFCPROPERTYSET({},#{},'Pset_VIGIX',$,({}))",
            step_str(&guid(&format!("{}|pset|{}", pid, el))),
            owner,
            ids.iter().map(|i| format!("#{}", i)).collect::<Vec<_>>().join(",")
        ));
        b.e(&format!("IFCRELDEFINESBYPROPERTIES({},#{},$,$,(#{}),#{})", step_str(&guid(&format!("{}|rel|{}", pid, el))), owner, el, pset));
    }

    let mut out = String::new();
    out.push_str("ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('ViewDefinition [CoordinationView]'),'2;1');\n");
    out.push_str(&format!(
        "FILE_NAME({},{},({}),('VIGIX'),'VIGIX EXTINCION FIJA {}','VIGIX EXTINCION FIJA','');\n",
        step_str(&format!("{}.ifc", pid)),
        step_str(&meta.timestamp),
        step_str(&meta.author),
        crate::ENGINE_VERSION
    ));
    out.push_str("FILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n");
    for (i, l) in b.lines.iter().enumerate() {
        out.push_str(&format!("#{}={};\n", i + 1, l));
    }
    out.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    (out, omitted)
}

#[allow(clippy::too_many_arguments)]
fn terminal(
    b: &mut B,
    ctx: usize,
    owner: usize,
    bld_pl: usize,
    pid: &str,
    id: &str,
    p: (f64, f64, f64),
    ptype: &str,
    otype: &str,
) -> usize {
    // cilindro pequeño de 0,05 m de radio y 0,12 m de alto
    let o2 = b.pt2(0.0, 0.0);
    let ap2 = b.e(&format!("IFCAXIS2PLACEMENT2D(#{},$)", o2));
    let prof = b.e(&format!("IFCCIRCLEPROFILEDEF(.AREA.,$,#{},0.05)", ap2));
    let pos3 = b.axis3((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), (1.0, 0.0, 0.0));
    let dir = b.dir3(0.0, 0.0, -1.0);
    let solid = b.e(&format!("IFCEXTRUDEDAREASOLID(#{},#{},#{},0.12)", prof, pos3, dir));
    let rep = b.e(&format!("IFCSHAPEREPRESENTATION(#{},'Body','SweptSolid',(#{}))", ctx, solid));
    let pds = b.e(&format!("IFCPRODUCTDEFINITIONSHAPE($,$,(#{}))", rep));
    let pl = b.local_placement(p, (0.0, 0.0, 1.0), (1.0, 0.0, 0.0), Some(bld_pl));
    let ot = if otype.is_empty() { "$".to_string() } else { step_str(otype) };
    b.e(&format!(
        "IFCFIRESUPPRESSIONTERMINAL({},#{},{},$,{},#{},#{},$,.{}.)",
        step_str(&guid(&format!("{}|term|{}", pid, id))),
        owner,
        step_str(id),
        ot,
        pl,
        pds,
        ptype
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globalid_determinista_de_22_caracteres_validos() {
        let g = guid("semilla");
        assert_eq!(g.len(), 22);
        assert_eq!(g, guid("semilla"));
        assert_ne!(g, guid("otra"));
        assert!(g.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$'));
        assert!("0123".contains(g.chars().next().unwrap()), "el primer carácter codifica solo 2 bits");
    }

    #[test]
    fn escape_step_y_reales() {
        assert_eq!(step_str("a'b\\c"), "'a''b\\\\c'");
        assert_eq!(step_str("Ñandú"), "'\\X2\\00D1\\X0\\and\\X2\\00FA\\X0\\'");
        assert_eq!(r(1.0), "1.");
        assert_eq!(r(0.05), "0.05");
        assert_eq!(r(-2.5), "-2.5");
        assert_eq!(r(1e-5), "0.00001");
        assert_eq!(r(1e-12), "1.E-12");
        assert_eq!(r(f64::NAN), "0.");
    }

    #[test]
    fn modelo_vacio_es_un_ifc_valido_en_forma() {
        let p = crate::project::new_project("P1", "Vacío", "es", "SI", "2026-10-28T00:00:00Z");
        let meta = IfcMeta {
            project_name: "Vacío".into(),
            author: "ana".into(),
            status_text: "BORRADOR".into(),
            profile: "DEMO@0.1.0".into(),
            profile_verified: false,
            fingerprint: "ab".repeat(32),
            timestamp: "2026-10-28T00:00:00".into(),
        };
        let (s, om) = build(&p, &meta);
        assert!(om.is_empty());
        assert!(s.starts_with("ISO-10303-21;") && s.trim_end().ends_with("END-ISO-10303-21;"));
        assert!(s.contains("IFCPROJECT(") && s.contains("IFCBUILDINGSTOREY("));
        assert_eq!(build(&p, &meta).0, s, "determinista");
    }
}
