//! Geometría 2D mínima y robusta para validar dibujos y analizar recorridos (contrato 5.2).
//! Coordenadas en metros. Sin dependencias.

pub type Pt = [f64; 2];

pub const EPS: f64 = 1e-9;

pub fn dist(a: Pt, b: Pt) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Área con signo (positiva en sentido antihorario) por la fórmula de Gauss.
pub fn signed_area(poly: &[Pt]) -> f64 {
    let n = poly.len();
    if n < 3 {
        return 0.0;
    }
    let mut s = 0.0;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        s += a[0] * b[1] - b[0] * a[1];
    }
    0.5 * s
}

pub fn area(poly: &[Pt]) -> f64 {
    signed_area(poly).abs()
}

pub fn bbox(pts: &[Pt]) -> Option<(Pt, Pt)> {
    let first = pts.first()?;
    let (mut lo, mut hi) = (*first, *first);
    for p in pts {
        lo[0] = lo[0].min(p[0]);
        lo[1] = lo[1].min(p[1]);
        hi[0] = hi[0].max(p[0]);
        hi[1] = hi[1].max(p[1]);
    }
    Some((lo, hi))
}

fn cross(o: Pt, a: Pt, b: Pt) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

fn on_segment(a: Pt, b: Pt, p: Pt) -> bool {
    p[0] >= a[0].min(b[0]) - EPS && p[0] <= a[0].max(b[0]) + EPS && p[1] >= a[1].min(b[1]) - EPS && p[1] <= a[1].max(b[1]) + EPS
}

/// ¿Se cortan (o se tocan) los segmentos ab y cd?
pub fn segments_intersect(a: Pt, b: Pt, c: Pt, d: Pt) -> bool {
    let d1 = cross(c, d, a);
    let d2 = cross(c, d, b);
    let d3 = cross(a, b, c);
    let d4 = cross(a, b, d);
    if ((d1 > EPS && d2 < -EPS) || (d1 < -EPS && d2 > EPS)) && ((d3 > EPS && d4 < -EPS) || (d3 < -EPS && d4 > EPS)) {
        return true;
    }
    (d1.abs() <= EPS && on_segment(c, d, a))
        || (d2.abs() <= EPS && on_segment(c, d, b))
        || (d3.abs() <= EPS && on_segment(a, b, c))
        || (d4.abs() <= EPS && on_segment(a, b, d))
}

/// Intersección propia (sin contar vértices compartidos de aristas contiguas).
pub fn polygon_self_intersects(poly: &[Pt]) -> bool {
    let n = poly.len();
    if n < 4 {
        return false;
    }
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        for j in (i + 1)..n {
            // aristas contiguas comparten un vértice: se ignoran
            if j == i + 1 || (i == 0 && j == n - 1) {
                continue;
            }
            let (c, d) = (poly[j], poly[(j + 1) % n]);
            if segments_intersect(a, b, c, d) {
                return true;
            }
        }
    }
    false
}

/// Punto en polígono (regla par-impar). Los puntos del borde cuentan como dentro.
pub fn point_in_polygon(p: Pt, poly: &[Pt]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if dist_point_segment(p, a, b) <= EPS {
            return true;
        }
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0];
            if p[0] < x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

pub fn dist_point_segment(p: Pt, a: Pt, b: Pt) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    if l2 <= EPS * EPS {
        return dist(p, a);
    }
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0);
    dist(p, [a[0] + t * dx, a[1] + t * dy])
}

/// Proyección paramétrica `t` (0..1 dentro del segmento) y distancia perpendicular.
pub fn project_on_segment(p: Pt, a: Pt, b: Pt) -> (f64, f64) {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    if l2 <= EPS * EPS {
        return (0.0, dist(p, a));
    }
    let t = ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2;
    let tc = t.clamp(0.0, 1.0);
    (t, dist(p, [a[0] + tc * dx, a[1] + tc * dy]))
}

/// ¿Se superponen dos polígonos simples (comparten área, no solo borde)?
pub fn polygons_overlap(a: &[Pt], b: &[Pt]) -> bool {
    if a.len() < 3 || b.len() < 3 {
        return false;
    }
    let (Some((alo, ahi)), Some((blo, bhi))) = (bbox(a), bbox(b)) else { return false };
    if ahi[0] <= blo[0] + EPS || bhi[0] <= alo[0] + EPS || ahi[1] <= blo[1] + EPS || bhi[1] <= alo[1] + EPS {
        return false;
    }
    // aristas que se cruzan propiamente (no solo se tocan)
    for i in 0..a.len() {
        let (p1, p2) = (a[i], a[(i + 1) % a.len()]);
        for j in 0..b.len() {
            let (q1, q2) = (b[j], b[(j + 1) % b.len()]);
            let d1 = cross(q1, q2, p1);
            let d2 = cross(q1, q2, p2);
            let d3 = cross(p1, p2, q1);
            let d4 = cross(p1, p2, q2);
            if ((d1 > EPS && d2 < -EPS) || (d1 < -EPS && d2 > EPS)) && ((d3 > EPS && d4 < -EPS) || (d3 < -EPS && d4 > EPS)) {
                return true;
            }
        }
    }
    // uno dentro del otro (con un punto interior estricto del borde desplazado al baricentro)
    let c_a = centroid_of_vertices(a);
    let c_b = centroid_of_vertices(b);
    (point_in_polygon(c_a, b) && !on_boundary(c_a, b))
        || (point_in_polygon(c_b, a) && !on_boundary(c_b, a))
        || strictly_inside_any_vertex(a, b)
        || strictly_inside_any_vertex(b, a)
}

fn centroid_of_vertices(p: &[Pt]) -> Pt {
    let n = p.len() as f64;
    [p.iter().map(|q| q[0]).sum::<f64>() / n, p.iter().map(|q| q[1]).sum::<f64>() / n]
}

fn on_boundary(p: Pt, poly: &[Pt]) -> bool {
    (0..poly.len()).any(|i| dist_point_segment(p, poly[i], poly[(i + 1) % poly.len()]) <= 1e-9)
}

fn strictly_inside_any_vertex(a: &[Pt], b: &[Pt]) -> bool {
    a.iter().any(|v| point_in_polygon(*v, b) && !on_boundary(*v, b))
}

/// Polígono con longitudes mínimas y sin vértices repetidos consecutivos.
pub fn has_degenerate_edges(poly: &[Pt], min_len: f64) -> bool {
    let n = poly.len();
    (0..n).any(|i| dist(poly[i], poly[(i + 1) % n]) < min_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQ: [Pt; 4] = [[0.0, 0.0], [4.0, 0.0], [4.0, 3.0], [0.0, 3.0]];

    #[test]
    fn area_y_orientacion() {
        assert!((signed_area(&SQ) - 12.0).abs() < 1e-12);
        let cw: Vec<Pt> = SQ.iter().rev().cloned().collect();
        assert!((signed_area(&cw) + 12.0).abs() < 1e-12);
        assert!((area(&cw) - 12.0).abs() < 1e-12);
        assert_eq!(area(&SQ[..2]), 0.0);
        // forma en L: 4x4 menos 2x2
        let l = [[0.0, 0.0], [4.0, 0.0], [4.0, 2.0], [2.0, 2.0], [2.0, 4.0], [0.0, 4.0]];
        assert!((area(&l) - 12.0).abs() < 1e-12);
    }

    #[test]
    fn punto_en_poligono_incluye_borde_y_concavo() {
        assert!(point_in_polygon([2.0, 1.0], &SQ));
        assert!(point_in_polygon([0.0, 1.0], &SQ)); // borde
        assert!(point_in_polygon([4.0, 3.0], &SQ)); // vértice
        assert!(!point_in_polygon([4.1, 1.0], &SQ));
        let l = [[0.0, 0.0], [4.0, 0.0], [4.0, 2.0], [2.0, 2.0], [2.0, 4.0], [0.0, 4.0]];
        assert!(point_in_polygon([1.0, 3.0], &l));
        assert!(!point_in_polygon([3.0, 3.0], &l)); // en el hueco de la L
    }

    #[test]
    fn segmentos_y_autointerseccion() {
        assert!(segments_intersect([0.0, 0.0], [2.0, 2.0], [0.0, 2.0], [2.0, 0.0]));
        assert!(!segments_intersect([0.0, 0.0], [1.0, 1.0], [2.0, 2.0], [3.0, 3.0]));
        assert!(segments_intersect([0.0, 0.0], [2.0, 0.0], [1.0, 0.0], [3.0, 0.0])); // colineales solapados
        assert!(segments_intersect([0.0, 0.0], [2.0, 0.0], [2.0, 0.0], [2.0, 2.0])); // se tocan
        assert!(!polygon_self_intersects(&SQ));
        let bow = [[0.0, 0.0], [2.0, 2.0], [2.0, 0.0], [0.0, 2.0]];
        assert!(polygon_self_intersects(&bow));
    }

    #[test]
    fn distancia_punto_segmento_y_proyeccion() {
        assert!((dist_point_segment([1.0, 1.0], [0.0, 0.0], [2.0, 0.0]) - 1.0).abs() < 1e-12);
        assert!((dist_point_segment([3.0, 0.0], [0.0, 0.0], [2.0, 0.0]) - 1.0).abs() < 1e-12);
        let (t, d) = project_on_segment([1.0, 2.0], [0.0, 0.0], [4.0, 0.0]);
        assert!((t - 0.25).abs() < 1e-12 && (d - 2.0).abs() < 1e-12);
        assert_eq!(dist_point_segment([1.0, 1.0], [0.0, 0.0], [0.0, 0.0]), 2f64.sqrt());
    }

    #[test]
    fn superposicion_de_poligonos() {
        let b = [[2.0, 1.0], [6.0, 1.0], [6.0, 4.0], [2.0, 4.0]];
        assert!(polygons_overlap(&SQ, &b));
        // contiguos (comparten solo un borde): NO se superponen
        let c = [[4.0, 0.0], [8.0, 0.0], [8.0, 3.0], [4.0, 3.0]];
        assert!(!polygons_overlap(&SQ, &c));
        // contenido
        let d = [[1.0, 1.0], [2.0, 1.0], [2.0, 2.0], [1.0, 2.0]];
        assert!(polygons_overlap(&SQ, &d));
        // separados
        let e = [[10.0, 10.0], [11.0, 10.0], [11.0, 11.0], [10.0, 11.0]];
        assert!(!polygons_overlap(&SQ, &e));
    }

    #[test]
    fn aristas_degeneradas() {
        assert!(!has_degenerate_edges(&SQ, 0.01));
        let p = [[0.0, 0.0], [0.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        assert!(has_degenerate_edges(&p, 0.01));
        assert_eq!(bbox(&SQ), Some(([0.0, 0.0], [4.0, 3.0])));
        assert_eq!(bbox(&[]), None);
    }
}
