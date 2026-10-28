//! Solver hidráulico de régimen permanente: método del gradiente global (Todini–Pilati).
//!
//! Unidades internas: **presión/carga en bar**, **caudal en L/min**, elevación en m.
//! La carga total de un nodo es `H = P + gamma * z`, con `gamma = rho*g/1e5` (bar/m).
//!
//! * Tramos de ley de potencia: `h = r * q*|q|^(n-1)` (Hazen–Williams y curva de suministro).
//! * Dispositivos activos: `Q = K * sqrt(P)`; con transición cúbica C1 cerca de P = 0 y descarga
//!   nula si P <= 0 (si la solución final queda con P < 0 la capa superior lo bloquea).
//! * Bombas: curva lineal por tramos (caudal, ganancia de presión), extrapolada solo para
//!   que la iteración continúe; el estado "fuera de curva" lo informa la capa superior.
//! * Válvulas de retención: se cierran con flujo inverso y se reabren con diferencia directa.
//!
//! Determinista: sin números aleatorios, sin recorrido de `HashMap`.

use super::linalg::{cholesky_solve, pcg, SymSparse};
use std::sync::atomic::{AtomicBool, Ordering};

/// Resistencia de un tramo cerrado (bar por L/min).
pub const G_CLOSED: f64 = 1.0e8;
/// Por debajo de este caudal (L/min) el tramo se linealiza para evitar derivada nula.
pub const Q_LIN: f64 = 1.0e-4;
/// Regularización de la descarga del dispositivo cerca de presión cero (bar).
pub const P_REG: f64 = 1.0e-4;
const G_MIN: f64 = 1.0e-12;

#[derive(Clone, Debug)]
pub struct Node {
    pub id: String,
    pub elevation_m: f64,
    /// `Some(P)`: carga fija; P es la presión (bar) a la elevación del nodo.
    pub fixed_pressure_bar: Option<f64>,
    /// Demanda fija (L/min), no negativa.
    pub demand_lpm: f64,
    /// Dispositivo activo `Q = K sqrt(P)` (L/min/bar^0.5).
    pub device_k: Option<f64>,
}

#[derive(Clone, Debug)]
pub enum LinkKind {
    /// `h = r * q|q|^(n-1)`
    Power { r: f64, n: f64 },
    /// Bomba: puntos `(caudal L/min, ganancia bar)` con caudal creciente y ganancia decreciente.
    Pump { curve: Vec<(f64, f64)> },
    /// Cerrado por el usuario.
    Closed,
}

#[derive(Clone, Debug)]
pub struct Link {
    pub id: String,
    pub from: usize,
    pub to: usize,
    pub kind: LinkKind,
    /// Válvula de retención (impide flujo inverso). Las bombas siempre la llevan.
    pub check_valve: bool,
    /// Caudal inicial sugerido (L/min); el constructor lo fija con ~1 m/s.
    pub q0_lpm: f64,
}

#[derive(Clone, Debug)]
pub struct Network {
    pub nodes: Vec<Node>,
    pub links: Vec<Link>,
    /// bar por metro de columna de fluido.
    pub gamma: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Variación relativa de caudales para dar por convergido.
    pub tol_rel: f64,
    /// Desequilibrio nodal relativo máximo para dar por convergido.
    pub tol_imbalance: f64,
    pub max_iter: usize,
    /// Hasta este número de incógnitas se usa Cholesky denso (directo).
    pub dense_limit: usize,
}

impl Default for Options {
    fn default() -> Self {
        // Muy por debajo del residuo del 0,1 % que exige el contrato (11.2); con convergencia
        // cuadrática el error real es mucho menor que `tol_rel`.
        Options { tol_rel: 1e-7, tol_imbalance: 1e-6, max_iter: 200, dense_limit: 300 }
    }
}

#[derive(Clone, Debug)]
pub struct Solution {
    pub head_bar: Vec<f64>,
    pub pressure_bar: Vec<f64>,
    pub flow_lpm: Vec<f64>,
    /// Descarga total en cada nodo (demanda fija + dispositivo), L/min.
    pub outflow_lpm: Vec<f64>,
    /// `false` si el tramo quedó cerrado (por el usuario o por retención/bomba apagada).
    pub link_open: Vec<bool>,
    pub iterations: usize,
    pub converged: bool,
    pub rel_change: f64,
    /// Máximo desequilibrio de continuidad en nodos libres (L/min).
    pub max_imbalance_lpm: f64,
    /// Desequilibrio máximo relativo al caudal total.
    pub rel_imbalance: f64,
    /// Máximo error de la ecuación de pérdida en los tramos abiertos (bar).
    pub max_link_error_bar: f64,
    /// Caudal neto que entregan los nodos de carga fija (L/min).
    pub source_flow_lpm: f64,
    pub linear_solver: &'static str,
    pub linear_iterations: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SolveError {
    Cancelled,
    NoFixedNode,
    /// El sistema lineal es singular (componente sin nodo de carga fija).
    Singular,
    Numerical(String),
}

/// Descarga del dispositivo y su derivada respecto a la presión.
///
/// * `P >= P_REG`:      `Q = K sqrt(P)`
/// * `0 < P < P_REG`:   transición cúbica (Hermite, C1) que parte de `Q = 0` con pendiente 0,
///   para evitar la derivada infinita de la raíz en P = 0;
/// * `P <= 0`:          `Q = 0` (sin presión no hay descarga; jamás caudal negativo).
pub fn device(k: f64, p: f64) -> (f64, f64) {
    if p >= P_REG {
        let s = p.sqrt();
        (k * s, k / (2.0 * s))
    } else if p > 0.0 {
        let dr = k * P_REG.sqrt();
        let t = p / P_REG;
        (dr * (2.5 * t * t - 1.5 * t * t * t), dr * (5.0 * t - 4.5 * t * t) / P_REG)
    } else {
        (0.0, 0.0)
    }
}

fn power(r: f64, n: f64, q: f64) -> (f64, f64) {
    let a = q.abs();
    if a < Q_LIN {
        let g = r * Q_LIN.powf(n - 1.0);
        (g * q, g)
    } else {
        let t = r * a.powf(n - 1.0);
        (t * q, n * t)
    }
}

/// Ganancia de la bomba y su pendiente en `q` (interpolación lineal; extrapola con el tramo extremo).
pub fn pump_gain(curve: &[(f64, f64)], q: f64) -> (f64, f64) {
    let m = curve.len();
    if m < 2 {
        return (curve.first().map(|c| c.1).unwrap_or(0.0), 0.0);
    }
    let mut seg = m - 2;
    for k in 0..m - 1 {
        if q <= curve[k + 1].0 {
            seg = k;
            break;
        }
    }
    let (x0, y0) = curve[seg];
    let (x1, y1) = curve[seg + 1];
    let slope = if x1 > x0 { (y1 - y0) / (x1 - x0) } else { 0.0 };
    (y0 + slope * (q - x0), slope)
}

/// Pérdida `h = H_from - H_to` y `g = dh/dq` de un tramo abierto.
pub fn link_headloss(kind: &LinkKind, q: f64) -> (f64, f64) {
    match kind {
        LinkKind::Power { r, n } => {
            let (h, g) = power(*r, *n, q);
            (h, g.max(G_MIN))
        }
        LinkKind::Pump { curve } => {
            let (gain, slope) = pump_gain(curve, q);
            (-gain, (-slope).max(G_MIN))
        }
        LinkKind::Closed => (G_CLOSED * q, G_CLOSED),
    }
}

fn is_finite_all(v: &[f64]) -> bool {
    v.iter().all(|x| x.is_finite())
}

pub fn solve(net: &Network, init: Option<&Solution>, opts: &Options, cancel: Option<&AtomicBool>) -> Result<Solution, SolveError> {
    let nn = net.nodes.len();
    let nl = net.links.len();
    if !net.nodes.iter().any(|n| n.fixed_pressure_bar.is_some()) {
        return Err(SolveError::NoFixedNode);
    }
    // índices de incógnitas
    let mut uidx: Vec<Option<usize>> = vec![None; nn];
    let mut nu = 0usize;
    for (i, n) in net.nodes.iter().enumerate() {
        if n.fixed_pressure_bar.is_none() {
            uidx[i] = Some(nu);
            nu += 1;
        }
    }
    // cargas iniciales
    let mut h = vec![0.0; nn];
    let max_fixed = net.nodes.iter().filter_map(|n| n.fixed_pressure_bar.map(|p| p + net.gamma * n.elevation_m)).fold(f64::MIN, f64::max);
    for (i, n) in net.nodes.iter().enumerate() {
        h[i] = match n.fixed_pressure_bar {
            Some(p) => p + net.gamma * n.elevation_m,
            None => match init {
                Some(s) if s.head_bar.len() == nn => s.head_bar[i],
                _ => max_fixed,
            },
        };
    }
    let mut q: Vec<f64> = match init {
        Some(s) if s.flow_lpm.len() == nl => s.flow_lpm.clone(),
        _ => net.links.iter().map(|l| l.q0_lpm).collect(),
    };
    let mut open: Vec<bool> = match init {
        Some(s) if s.link_open.len() == nl => s.link_open.clone(),
        _ => net.links.iter().map(|l| !matches!(l.kind, LinkKind::Closed)).collect(),
    };
    for (k, l) in net.links.iter().enumerate() {
        if matches!(l.kind, LinkKind::Closed) {
            open[k] = false;
        }
    }

    let mut hl = vec![0.0; nl];
    let mut pk = vec![0.0; nl];
    let mut x_u = vec![0.0; nu];
    for (i, n) in net.nodes.iter().enumerate() {
        let _ = n;
        if let Some(u) = uidx[i] {
            x_u[u] = h[i];
        }
    }
    let mut theta = 1.0f64;
    let mut prev_rel = f64::INFINITY;
    let mut converged = false;
    let mut rel = f64::INFINITY;
    let mut iterations = 0usize;
    let mut solver_used = "cholesky";
    let mut lin_iters = 0usize;

    for it in 0..opts.max_iter {
        if let Some(c) = cancel {
            if c.load(Ordering::Relaxed) {
                return Err(SolveError::Cancelled);
            }
        }
        iterations = it + 1;
        // 1. linealización de tramos
        for (k, l) in net.links.iter().enumerate() {
            let (hk, gk) = if open[k] { link_headloss(&l.kind, q[k]) } else { (G_CLOSED * q[k], G_CLOSED) };
            hl[k] = hk;
            pk[k] = 1.0 / gk;
        }
        // 2. ensamblado
        let mut a = SymSparse::new(nu);
        let mut rhs = vec![0.0; nu];
        for (k, l) in net.links.iter().enumerate() {
            let y = q[k] - pk[k] * hl[k];
            let (ua, ub) = (uidx[l.from], uidx[l.to]);
            if let Some(b) = ub {
                rhs[b] += y;
                a.diag[b] += pk[k];
            }
            if let Some(aa) = ua {
                rhs[aa] -= y;
                a.diag[aa] += pk[k];
            }
            match (ua, ub) {
                (Some(i), Some(j)) => a.off.push((i, j, pk[k])),
                (Some(i), None) => rhs[i] += pk[k] * h[l.to],
                (None, Some(j)) => rhs[j] += pk[k] * h[l.from],
                (None, None) => {}
            }
        }
        for (i, n) in net.nodes.iter().enumerate() {
            if let Some(u) = uidx[i] {
                rhs[u] -= n.demand_lpm;
                if let Some(kf) = n.device_k {
                    let p = h[i] - net.gamma * n.elevation_m;
                    let (d0, dp) = device(kf, p);
                    a.diag[u] += dp;
                    rhs[u] += -d0 + dp * h[i];
                }
            }
        }
        // 3. resolución lineal
        let mut sol = rhs.clone();
        let mut ok = false;
        if nu > 0 {
            if nu <= opts.dense_limit {
                let mut d = a.to_dense();
                ok = cholesky_solve(&mut d, nu, &mut sol);
                solver_used = "cholesky";
            }
            if !ok {
                let mut x = x_u.clone();
                let tol = 1e-12;
                let rep = pcg(&a, &rhs, &mut x, tol, (10 * nu).max(2000));
                lin_iters += rep.iterations;
                solver_used = "pcg-jacobi";
                if !rep.converged && rep.rel_residual > 1e-8 {
                    return Err(SolveError::Singular);
                }
                sol = x;
                ok = true;
            }
        } else {
            ok = true;
        }
        if !ok || !is_finite_all(&sol) {
            return Err(SolveError::Singular);
        }
        // 4. actualización de cargas y caudales
        let h_old = h.clone();
        for (i, _n) in net.nodes.iter().enumerate() {
            if let Some(u) = uidx[i] {
                h[i] = sol[u];
                x_u[u] = sol[u];
            }
        }
        // `sum_dq` mide el paso de Newton COMPLETO (distancia real a la solución), aunque luego
        // se aplique relajado.
        let mut sum_dq = 0.0;
        let mut sum_q = 0.0;
        let mut q_new = vec![0.0; nl];
        for (k, l) in net.links.iter().enumerate() {
            let dq = pk[k] * (h[l.from] - h[l.to] - hl[k]);
            let mut qn = q[k] + dq;
            if !open[k] {
                qn = 0.0;
            }
            q_new[k] = q[k] + theta * (qn - q[k]);
            sum_dq += (qn - q[k]).abs();
            sum_q += qn.abs();
        }
        if theta < 1.0 {
            // con relajación, las cargas también se relajan para mantener coherencia
            for (i, _n) in net.nodes.iter().enumerate() {
                if uidx[i].is_some() {
                    h[i] = h_old[i] + theta * (h[i] - h_old[i]);
                    if let Some(u) = uidx[i] {
                        x_u[u] = h[i];
                    }
                }
            }
        }
        q = q_new;
        if !is_finite_all(&q) || !is_finite_all(&h) {
            return Err(SolveError::Numerical("valores no finitos durante la iteración".to_string()));
        }
        rel = sum_dq / (sum_q + 1e-12);

        // 5. estado de válvulas de retención y bombas
        let mut changed = false;
        if it < 40 {
            for (k, l) in net.links.iter().enumerate() {
                if matches!(l.kind, LinkKind::Closed) {
                    continue;
                }
                let is_pump = matches!(l.kind, LinkKind::Pump { .. });
                if !(l.check_valve || is_pump) {
                    continue;
                }
                let dh = h[l.from] - h[l.to];
                if open[k] {
                    if q[k] < -1e-9 {
                        open[k] = false;
                        q[k] = 0.0;
                        changed = true;
                    }
                } else {
                    let reopen = match &l.kind {
                        LinkKind::Pump { curve } => {
                            // se reabre si la carga requerida es inferior a la de cierre de la bomba
                            let shutoff = curve.first().map(|c| c.1).unwrap_or(0.0);
                            -dh < shutoff - 1e-9
                        }
                        _ => dh > 1e-9,
                    };
                    if reopen {
                        open[k] = true;
                        q[k] = if l.q0_lpm > 0.0 { l.q0_lpm * 1e-3 } else { 1e-3 };
                        changed = true;
                    }
                }
            }
        }

        // 6. adaptación del factor de relajación: solo si el error es grande y no decrece
        //    (el ruido numérico a nivel 1e-9 no debe activar el amortiguamiento).
        if it >= 3 && rel > 1e-6 && rel > 0.98 * prev_rel {
            theta = (theta * 0.6).max(0.25);
        } else {
            theta = (theta * 1.25).min(1.0);
        }
        prev_rel = rel;

        if !changed && rel <= opts.tol_rel {
            // comprobación del desequilibrio nodal real (no linealizado)
            let (imb, _, _) = imbalance(net, &h, &q);
            let total = sum_q.max(1e-9);
            if imb <= opts.tol_imbalance * total.max(1.0) {
                converged = true;
                break;
            }
        }
    }

    // diagnósticos finales con las funciones no lineales reales
    let (max_imb, outflow, source_flow) = imbalance(net, &h, &q);
    let total_flow = outflow.iter().map(|v| v.abs()).sum::<f64>().max(source_flow.abs()).max(1e-12);
    let mut max_link_err = 0.0f64;
    for (k, l) in net.links.iter().enumerate() {
        if open[k] {
            let (hk, _) = link_headloss(&l.kind, q[k]);
            max_link_err = max_link_err.max(((h[l.from] - h[l.to]) - hk).abs());
        }
    }
    let pressure: Vec<f64> = net.nodes.iter().enumerate().map(|(i, n)| h[i] - net.gamma * n.elevation_m).collect();
    Ok(Solution {
        head_bar: h,
        pressure_bar: pressure,
        flow_lpm: q,
        outflow_lpm: outflow,
        link_open: open,
        iterations,
        converged,
        rel_change: rel,
        max_imbalance_lpm: max_imb,
        rel_imbalance: max_imb / total_flow,
        max_link_error_bar: max_link_err,
        source_flow_lpm: source_flow,
        linear_solver: solver_used,
        linear_iterations: lin_iters,
    })
}

/// Desequilibrio de continuidad en nodos libres, descarga por nodo y caudal neto de las fuentes.
fn imbalance(net: &Network, h: &[f64], q: &[f64]) -> (f64, Vec<f64>, f64) {
    let nn = net.nodes.len();
    let mut net_in = vec![0.0; nn];
    for (k, l) in net.links.iter().enumerate() {
        net_in[l.to] += q[k];
        net_in[l.from] -= q[k];
    }
    let mut outflow = vec![0.0; nn];
    let mut max_imb = 0.0f64;
    let mut source = 0.0;
    for (i, n) in net.nodes.iter().enumerate() {
        if n.fixed_pressure_bar.is_some() {
            source += -net_in[i];
            continue;
        }
        let mut d = n.demand_lpm;
        if let Some(kf) = n.device_k {
            d += device(kf, h[i] - net.gamma * n.elevation_m).0;
        }
        outflow[i] = d;
        max_imb = max_imb.max((net_in[i] - d).abs());
    }
    (max_imb, outflow, source)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAMMA: f64 = 0.0980665;

    fn node(id: &str, z: f64) -> Node {
        Node { id: id.into(), elevation_m: z, fixed_pressure_bar: None, demand_lpm: 0.0, device_k: None }
    }
    fn pipe(id: &str, a: usize, b: usize, r: f64) -> Link {
        Link { id: id.into(), from: a, to: b, kind: LinkKind::Power { r, n: 1.85 }, check_valve: false, q0_lpm: 50.0 }
    }

    /// r de Hazen–Williams (bar/(L/min)^1.85) para L [m], d [mm], C con K = 6.05e5.
    fn hw_r(len: f64, d_mm: f64, c: f64) -> f64 {
        6.05e5 * len / (c.powf(1.85) * d_mm.powf(4.87))
    }

    /// Referencia independiente: resolver un solo tramo + dispositivo por bisección escalar.
    fn single_pipe_device(p_src: f64, z: f64, r: f64, k: f64) -> f64 {
        // Q = K sqrt(P_n), P_src - r Q^1.85 - gamma z = P_n
        let f = |q: f64| {
            let pn = p_src - r * q.powf(1.85) - GAMMA * z;
            if pn <= 0.0 {
                -q
            } else {
                k * pn.sqrt() - q
            }
        };
        let (mut lo, mut hi) = (0.0, 1.0e5);
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

    #[test]
    fn un_tramo_y_un_dispositivo_coincide_con_biseccion() {
        let r = hw_r(100.0, 50.0, 120.0);
        let mut src = node("R", 0.0);
        src.fixed_pressure_bar = Some(7.0);
        let mut dev = node("S", 3.0);
        dev.device_k = Some(80.0);
        let net = Network { nodes: vec![src, dev], links: vec![pipe("P1", 0, 1, r)], gamma: GAMMA };
        let s = solve(&net, None, &Options::default(), None).unwrap();
        assert!(s.converged, "iter={} rel={}", s.iterations, s.rel_change);
        let qref = single_pipe_device(7.0, 3.0, r, 80.0);
        assert!((s.flow_lpm[0] - qref).abs() < 1e-7 * qref, "{} vs {}", s.flow_lpm[0], qref);
        assert!((s.outflow_lpm[1] - qref).abs() < 1e-7 * qref);
        assert!(s.max_imbalance_lpm < 1e-7);
        assert!(s.rel_imbalance < 1e-9);
        // P en el dispositivo coherente con Q = K sqrt(P)
        let pn = s.pressure_bar[1];
        assert!((80.0 * pn.sqrt() - s.flow_lpm[0]).abs() < 1e-6);
    }

    #[test]
    fn dos_tramos_en_paralelo_reparten_segun_resistencia() {
        // R -> A (dos tuberías en paralelo) -> dispositivo. Q1/Q2 = (r2/r1)^(1/n)
        let r1 = hw_r(80.0, 50.0, 120.0);
        let r2 = hw_r(120.0, 40.0, 120.0);
        let mut src = node("R", 0.0);
        src.fixed_pressure_bar = Some(8.0);
        let mut dev = node("S", 0.0);
        dev.device_k = Some(115.0);
        let net = Network {
            nodes: vec![src, node("A", 0.0), dev],
            links: vec![pipe("P1", 0, 1, r1), pipe("P2", 0, 1, r2), pipe("P3", 1, 2, hw_r(10.0, 65.0, 120.0))],
            gamma: GAMMA,
        };
        let s = solve(&net, None, &Options::default(), None).unwrap();
        assert!(s.converged);
        let ratio = s.flow_lpm[0] / s.flow_lpm[1];
        let expect = (r2 / r1).powf(1.0 / 1.85);
        assert!((ratio - expect).abs() < 1e-7 * expect, "{} vs {}", ratio, expect);
        assert!((s.flow_lpm[0] + s.flow_lpm[1] - s.flow_lpm[2]).abs() < 1e-7);
    }

    #[test]
    fn punto_de_curva_de_suministro() {
        // Fuente estática 6 bar, residual 4 bar a 1000 L/min: con demanda fija 1000 L/min en el
        // nodo de conexión, la presión es exactamente la residual.
        let (ps, pr, qt, ns): (f64, f64, f64, f64) = (6.0, 4.0, 1000.0, 1.85);
        let r = (ps - pr) / qt.powf(ns);
        let mut src = node("SRC", 0.0);
        src.fixed_pressure_bar = Some(ps);
        let mut con = node("CON", 0.0);
        con.demand_lpm = qt;
        let net = Network {
            nodes: vec![src, con],
            links: vec![Link { id: "SUP".into(), from: 0, to: 1, kind: LinkKind::Power { r, n: ns }, check_valve: true, q0_lpm: 500.0 }],
            gamma: GAMMA,
        };
        let s = solve(&net, None, &Options::default(), None).unwrap();
        assert!(s.converged);
        assert!((s.pressure_bar[1] - pr).abs() < 1e-9, "{}", s.pressure_bar[1]);
        assert!((s.source_flow_lpm - qt).abs() < 1e-7);
    }

    #[test]
    fn bomba_en_el_punto_de_operacion() {
        // Depósito (presión 0) -> bomba -> tubería -> demanda fija. Cargas: gain(q) = r q^1.85 + gamma*dz
        let curve = vec![(0.0, 10.0), (500.0, 9.0), (1000.0, 7.0), (1500.0, 3.0)];
        let mut tank = node("T", 0.0);
        tank.fixed_pressure_bar = Some(0.0);
        let mut out = node("O", 10.0);
        out.demand_lpm = 600.0;
        let r = hw_r(200.0, 100.0, 120.0);
        let net = Network {
            nodes: vec![tank, node("D", 0.0), out],
            links: vec![
                Link { id: "PU".into(), from: 0, to: 1, kind: LinkKind::Pump { curve: curve.clone() }, check_valve: true, q0_lpm: 600.0 },
                pipe("P", 1, 2, r),
            ],
            gamma: GAMMA,
        };
        let s = solve(&net, None, &Options::default(), None).unwrap();
        assert!(s.converged);
        assert!((s.flow_lpm[0] - 600.0).abs() < 1e-7);
        // La ganancia de la bomba a 600 L/min (interpolada) debe igualar la pérdida + elevación.
        let (gain, _) = pump_gain(&curve, 600.0);
        let need = r * 600f64.powf(1.85) + GAMMA * 10.0;
        // presión en el nodo de salida = ganancia - pérdida - elevación
        let p_out = gain - r * 600f64.powf(1.85) - GAMMA * 10.0;
        assert!((s.pressure_bar[2] - p_out).abs() < 1e-8, "{} vs {}", s.pressure_bar[2], p_out);
        assert!(gain > need - 10.0); // coherencia de magnitudes
    }

    #[test]
    fn valvula_de_retencion_se_cierra_con_flujo_inverso() {
        // Dos fuentes: A (5 bar) y B (3 bar) unidas por un tramo con retención A->B... flujo directo
        // posible (A>B) => abierta. Invertimos: retención B->A, con B<A => debe cerrarse.
        let mut a = node("A", 0.0);
        a.fixed_pressure_bar = Some(5.0);
        let mut b = node("B", 0.0);
        b.fixed_pressure_bar = Some(3.0);
        let mut m = node("M", 0.0);
        m.demand_lpm = 10.0;
        let r = hw_r(10.0, 50.0, 120.0);
        let net = Network {
            nodes: vec![a, b, m],
            links: vec![
                pipe("AM", 0, 2, r),
                Link { id: "BM".into(), from: 1, to: 2, kind: LinkKind::Power { r, n: 1.85 }, check_valve: true, q0_lpm: 5.0 },
            ],
            gamma: GAMMA,
        };
        let s = solve(&net, None, &Options::default(), None).unwrap();
        assert!(s.converged);
        // B (3 bar) está por debajo de la presión en M (~5 bar): la retención BM permanece cerrada
        assert!(!s.link_open[1], "la retención debía cerrarse");
        assert_eq!(s.flow_lpm[1], 0.0);
        assert!((s.flow_lpm[0] - 10.0).abs() < 1e-7);
    }

    #[test]
    fn descarga_del_dispositivo_es_continua_monotona_y_nunca_negativa() {
        let k = 80.0;
        assert_eq!(device(k, -5.0), (0.0, 0.0));
        assert_eq!(device(k, 0.0), (0.0, 0.0));
        let mut prev = 0.0;
        let mut p = -0.01;
        while p < 0.05 {
            let (q, dq) = device(k, p);
            assert!(q >= 0.0 && dq >= 0.0 && q >= prev - 1e-15, "p={} q={} prev={}", p, q, prev);
            prev = q;
            p += 1e-6;
        }
        // continuidad C1 en P_REG
        let (q1, d1) = device(k, P_REG * (1.0 - 1e-9));
        let (q2, d2) = device(k, P_REG);
        assert!((q1 - q2).abs() < 1e-6 && (d1 - d2).abs() < 1e-2 * d2);
        // derivada coherente con diferencias finitas
        for p in [0.2 * P_REG, 0.7 * P_REG, 2.0 * P_REG, 1.0, 9.0] {
            let h = p * 1e-6;
            let num = (device(k, p + h).0 - device(k, p - h).0) / (2.0 * h);
            assert!((num - device(k, p).1).abs() < 1e-4 * num.abs().max(1.0), "p={} num={} an={}", p, num, device(k, p).1);
        }
    }

    #[test]
    fn sin_nodo_fijo_es_error_explicito() {
        let net = Network { nodes: vec![node("A", 0.0), node("B", 0.0)], links: vec![pipe("P", 0, 1, 1e-4)], gamma: GAMMA };
        assert_eq!(solve(&net, None, &Options::default(), None).unwrap_err(), SolveError::NoFixedNode);
    }

    #[test]
    fn componente_aislada_es_singular() {
        let mut a = node("A", 0.0);
        a.fixed_pressure_bar = Some(5.0);
        let mut c = node("C", 0.0);
        c.demand_lpm = 10.0;
        // C-D forman una isla sin fuente
        let net = Network {
            nodes: vec![a, node("B", 0.0), c, node("D", 0.0)],
            links: vec![pipe("P1", 0, 1, 1e-4), pipe("P2", 2, 3, 1e-4)],
            gamma: GAMMA,
        };
        assert_eq!(solve(&net, None, &Options::default(), None).unwrap_err(), SolveError::Singular);
    }

    #[test]
    fn cancelacion_cooperativa() {
        let mut src = node("R", 0.0);
        src.fixed_pressure_bar = Some(7.0);
        let mut dev = node("S", 0.0);
        dev.device_k = Some(80.0);
        let net = Network { nodes: vec![src, dev], links: vec![pipe("P1", 0, 1, 1e-4)], gamma: GAMMA };
        let flag = AtomicBool::new(true);
        assert_eq!(solve(&net, None, &Options::default(), Some(&flag)).unwrap_err(), SolveError::Cancelled);
    }

    #[test]
    fn dispositivo_con_presion_insuficiente_se_marca_por_presion_negativa() {
        // Elevación del dispositivo mayor que la carga disponible => P < 0 en la solución.
        let mut src = node("R", 0.0);
        src.fixed_pressure_bar = Some(0.5);
        let mut dev = node("S", 30.0); // 30 m => 2.94 bar de elevación
        dev.device_k = Some(80.0);
        let net = Network { nodes: vec![src, dev], links: vec![pipe("P1", 0, 1, 1e-4)], gamma: GAMMA };
        let s = solve(&net, None, &Options::default(), None).unwrap();
        assert!(s.pressure_bar[1] < 0.0, "debía quedar presión negativa: {}", s.pressure_bar[1]);
    }

    #[test]
    fn determinismo_bit_a_bit() {
        let r = hw_r(100.0, 50.0, 120.0);
        let mut src = node("R", 0.0);
        src.fixed_pressure_bar = Some(7.0);
        let mut d1 = node("S1", 3.0);
        d1.device_k = Some(80.0);
        let mut d2 = node("S2", 3.0);
        d2.device_k = Some(115.0);
        let net = Network {
            nodes: vec![src, node("J", 0.0), d1, d2],
            links: vec![pipe("P1", 0, 1, r), pipe("P2", 1, 2, r * 0.5), pipe("P3", 1, 3, r * 0.7)],
            gamma: GAMMA,
        };
        let a = solve(&net, None, &Options::default(), None).unwrap();
        let b = solve(&net, None, &Options::default(), None).unwrap();
        assert_eq!(a.flow_lpm, b.flow_lpm);
        assert_eq!(a.head_bar, b.head_bar);
    }
}
