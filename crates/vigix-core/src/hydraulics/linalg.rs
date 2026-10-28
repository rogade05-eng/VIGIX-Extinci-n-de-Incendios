//! Álgebra lineal mínima para el solver hidráulico (sin dependencias).
//!
//! Sistema simétrico definido positivo `A = diag(d) - sum_k w_k (e_i e_j' + e_j e_i')`
//! (matriz tipo Laplaciano con condición de Dirichlet en los nodos de carga fija).

/// Matriz simétrica dispersa en forma "diagonal + aristas".
pub struct SymSparse {
    pub n: usize,
    pub diag: Vec<f64>,
    /// `(i, j, w)` con `w > 0` representa A[i][j] = A[j][i] = -w.
    pub off: Vec<(usize, usize, f64)>,
}

impl SymSparse {
    pub fn new(n: usize) -> Self {
        SymSparse { n, diag: vec![0.0; n], off: Vec::new() }
    }

    pub fn matvec(&self, x: &[f64], y: &mut [f64]) {
        for i in 0..self.n {
            y[i] = self.diag[i] * x[i];
        }
        for &(i, j, w) in &self.off {
            y[i] -= w * x[j];
            y[j] -= w * x[i];
        }
    }

    /// Matriz densa (fila mayor) para el camino directo.
    pub fn to_dense(&self) -> Vec<f64> {
        let n = self.n;
        let mut a = vec![0.0; n * n];
        for i in 0..n {
            a[i * n + i] = self.diag[i];
        }
        for &(i, j, w) in &self.off {
            a[i * n + j] -= w;
            a[j * n + i] -= w;
        }
        a
    }
}

/// Resuelve `A x = b` por Cholesky denso. `a` se destruye. Devuelve false si no es definida positiva.
pub fn cholesky_solve(a: &mut [f64], n: usize, b: &mut [f64]) -> bool {
    for j in 0..n {
        let mut d = a[j * n + j];
        for k in 0..j {
            d -= a[j * n + k] * a[j * n + k];
        }
        if d.is_nan() || d <= 0.0 || !d.is_finite() {
            return false;
        }
        let d = d.sqrt();
        a[j * n + j] = d;
        for i in (j + 1)..n {
            let mut s = a[i * n + j];
            for k in 0..j {
                s -= a[i * n + k] * a[j * n + k];
            }
            a[i * n + j] = s / d;
        }
    }
    // L y = b
    for i in 0..n {
        let mut s = b[i];
        for k in 0..i {
            s -= a[i * n + k] * b[k];
        }
        b[i] = s / a[i * n + i];
    }
    // L' x = y
    for i in (0..n).rev() {
        let mut s = b[i];
        for k in (i + 1)..n {
            s -= a[k * n + i] * b[k];
        }
        b[i] = s / a[i * n + i];
    }
    true
}

pub struct PcgReport {
    pub converged: bool,
    pub iterations: usize,
    pub rel_residual: f64,
}

/// Gradiente conjugado precondicionado (Jacobi). `x` entra como estimación inicial.
pub fn pcg(a: &SymSparse, b: &[f64], x: &mut [f64], tol: f64, max_iter: usize) -> PcgReport {
    let n = a.n;
    let bnorm = b.iter().map(|v| v * v).sum::<f64>().sqrt();
    if bnorm == 0.0 {
        for v in x.iter_mut() {
            *v = 0.0;
        }
        return PcgReport { converged: true, iterations: 0, rel_residual: 0.0 };
    }
    let mut r = vec![0.0; n];
    let mut ax = vec![0.0; n];
    a.matvec(x, &mut ax);
    for i in 0..n {
        r[i] = b[i] - ax[i];
    }
    let inv_d: Vec<f64> = a.diag.iter().map(|&d| if d > 0.0 { 1.0 / d } else { 1.0 }).collect();
    let mut z: Vec<f64> = (0..n).map(|i| r[i] * inv_d[i]).collect();
    let mut p = z.clone();
    let mut rz: f64 = (0..n).map(|i| r[i] * z[i]).sum();
    let mut ap = vec![0.0; n];
    let mut rel = r.iter().map(|v| v * v).sum::<f64>().sqrt() / bnorm;
    let mut it = 0;
    while rel > tol && it < max_iter {
        a.matvec(&p, &mut ap);
        let pap: f64 = (0..n).map(|i| p[i] * ap[i]).sum();
        if pap.is_nan() || pap <= 0.0 || !pap.is_finite() {
            break;
        }
        let alpha = rz / pap;
        for i in 0..n {
            x[i] += alpha * p[i];
            r[i] -= alpha * ap[i];
        }
        rel = r.iter().map(|v| v * v).sum::<f64>().sqrt() / bnorm;
        if rel <= tol {
            it += 1;
            break;
        }
        for i in 0..n {
            z[i] = r[i] * inv_d[i];
        }
        let rz_new: f64 = (0..n).map(|i| r[i] * z[i]).sum();
        let beta = rz_new / rz;
        rz = rz_new;
        for i in 0..n {
            p[i] = z[i] + beta * p[i];
        }
        it += 1;
    }
    PcgReport { converged: rel <= tol, iterations: it, rel_residual: rel }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lap_chain(n: usize) -> SymSparse {
        // cadena 0-1-2-...-(n-1) con conductancia 1 y apoyo (Dirichlet) en el nodo 0 vía diag extra
        let mut a = SymSparse::new(n);
        for i in 0..n {
            a.diag[i] = 0.0;
        }
        for i in 0..n - 1 {
            a.off.push((i, i + 1, 1.0));
            a.diag[i] += 1.0;
            a.diag[i + 1] += 1.0;
        }
        a.diag[0] += 1.0; // conexión a nodo fijo
        a
    }

    #[test]
    fn cholesky_y_pcg_coinciden() {
        let n = 60;
        let a = lap_chain(n);
        let b: Vec<f64> = (0..n).map(|i| ((i * 7 % 11) as f64) - 3.0).collect();
        let mut x1 = b.clone();
        let mut d = a.to_dense();
        assert!(cholesky_solve(&mut d, n, &mut x1));
        let mut x2 = vec![0.0; n];
        let rep = pcg(&a, &b, &mut x2, 1e-13, 10_000);
        assert!(rep.converged, "pcg no convergió: {:?}", rep.rel_residual);
        for i in 0..n {
            assert!((x1[i] - x2[i]).abs() <= 1e-8 * x1[i].abs().max(1.0), "i={} {} {}", i, x1[i], x2[i]);
        }
        // residuo directo
        let mut ax = vec![0.0; n];
        a.matvec(&x1, &mut ax);
        for i in 0..n {
            assert!((ax[i] - b[i]).abs() < 1e-9);
        }
    }

    #[test]
    fn cholesky_detecta_matriz_singular() {
        // sin apoyo: Laplaciano puro => singular
        let mut a = SymSparse::new(3);
        a.off.push((0, 1, 1.0));
        a.off.push((1, 2, 1.0));
        a.diag = vec![1.0, 2.0, 1.0];
        let mut b = vec![1.0, 0.0, -1.0];
        let mut d = a.to_dense();
        assert!(!cholesky_solve(&mut d, 3, &mut b));
    }

    #[test]
    fn pcg_con_rhs_cero() {
        let a = lap_chain(5);
        let mut x = vec![1.0; 5];
        let rep = pcg(&a, &[0.0; 5], &mut x, 1e-12, 100);
        assert!(rep.converged);
        assert!(x.iter().all(|v| *v == 0.0));
    }
}
