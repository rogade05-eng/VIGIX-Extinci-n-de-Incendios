//! Rendimiento y escala (contrato 11.3): proyecto de referencia de 5 000 nodos, 10 000 tramos y
//! 2 000 dispositivos; la solución hidráulica debe tardar menos de 60 s.

use std::time::Instant;
use vigix_core::hydraulics::solver::{solve, Link, LinkKind, Network, Node, Options};

/// Malla `w x h` con una fuente en la esquina. Cada nodo se une con el vecino derecho y el inferior.
fn grid(w: usize, h: usize, devices: usize) -> Network {
    let gamma = 0.0980665;
    let mut nodes = vec![Node { id: "SRC".into(), elevation_m: 0.0, fixed_pressure_bar: Some(12.0), demand_lpm: 0.0, device_k: None }];
    for i in 0..h {
        for j in 0..w {
            nodes.push(Node { id: format!("N{}_{}", i, j), elevation_m: 3.0, fixed_pressure_bar: None, demand_lpm: 0.0, device_k: None });
        }
    }
    let total = w * h;
    // dispositivos repartidos de forma determinista y uniforme
    let step = (total / devices).max(1);
    let mut placed = 0;
    let mut idx = 0;
    while idx < total && placed < devices {
        nodes[1 + idx].device_k = Some(80.0);
        idx += step;
        placed += 1;
    }
    let r_of = |d: f64, len: f64| 6.05e5 * len / (120f64.powf(1.85) * d.powf(4.87));
    let mut links = vec![Link {
        id: "FEED".into(),
        from: 0,
        to: 1,
        kind: LinkKind::Power { r: r_of(200.0, 10.0), n: 1.85 },
        check_valve: false,
        q0_lpm: 1000.0,
    }];
    let id = |i: usize, j: usize| 1 + i * w + j;
    let mut k = 0;
    for i in 0..h {
        for j in 0..w {
            // diámetros decrecientes con la distancia a la fuente
            let d = if i == 0 || j == 0 { 150.0 } else { 80.0 };
            if j + 1 < w {
                links.push(Link {
                    id: format!("H{}", k),
                    from: id(i, j),
                    to: id(i, j + 1),
                    kind: LinkKind::Power { r: r_of(d, 3.0), n: 1.85 },
                    check_valve: false,
                    q0_lpm: 100.0,
                });
                k += 1;
            }
            if i + 1 < h {
                links.push(Link {
                    id: format!("V{}", k),
                    from: id(i, j),
                    to: id(i + 1, j),
                    kind: LinkKind::Power { r: r_of(d, 3.0), n: 1.85 },
                    check_valve: false,
                    q0_lpm: 100.0,
                });
                k += 1;
            }
        }
    }
    Network { nodes, links, gamma }
}

#[test]
fn red_de_referencia_5000_nodos_10000_tramos_2000_dispositivos() {
    let net = grid(71, 72, 2000); // 5 112 nodos libres, 10 082 tramos
    assert!(net.nodes.len() >= 5000, "{}", net.nodes.len());
    assert!(net.links.len() >= 10000, "{}", net.links.len());
    assert_eq!(net.nodes.iter().filter(|n| n.device_k.is_some()).count(), 2000);
    let t0 = Instant::now();
    let sol = solve(&net, None, &Options::default(), None).expect("debe resolver");
    let dt = t0.elapsed().as_secs_f64();
    eprintln!(
        "malla {} nodos / {} tramos / 2000 dispositivos: {:.2} s, {} iteraciones Newton, {} iteraciones lineales, solver {}, residuo relativo {:.2e}",
        net.nodes.len(),
        net.links.len(),
        dt,
        sol.iterations,
        sol.linear_iterations,
        sol.linear_solver,
        sol.rel_imbalance
    );
    assert!(sol.converged, "no convergió: rel {} iter {}", sol.rel_change, sol.iterations);
    assert!(sol.rel_imbalance <= 1e-3);
    assert!(dt < 60.0, "tardó {:.1} s (límite 60 s)", dt);
    // Todos los dispositivos descargan algo y la masa se conserva
    let dev_flow: f64 = net.nodes.iter().enumerate().filter(|(_, n)| n.device_k.is_some()).map(|(i, _)| sol.outflow_lpm[i]).sum();
    assert!((dev_flow - sol.source_flow_lpm).abs() < 1e-3 * dev_flow, "{} vs {}", dev_flow, sol.source_flow_lpm);
}

fn r_of(d: f64, len: f64) -> f64 {
    6.05e5 * len / (120f64.powf(1.85) * d.powf(4.87))
}

fn timed(name: &str, net: &Network) {
    let t0 = Instant::now();
    let sol = solve(net, None, &Options::default(), None).expect("debe resolver");
    let dt = t0.elapsed().as_secs_f64();
    eprintln!(
        "{}: {} nodos / {} tramos: {:.2} s, {} iter. Newton, {} iter. lineales ({}), residuo {:.2e}",
        name,
        net.nodes.len(),
        net.links.len(),
        dt,
        sol.iterations,
        sol.linear_iterations,
        sol.linear_solver,
        sol.rel_imbalance
    );
    assert!(sol.converged, "{}: no convergió (rel {})", name, sol.rel_change);
    assert!(sol.rel_imbalance <= 1e-3);
    assert!(dt < 60.0, "{}: {:.1} s", name, dt);
}

#[test]
fn cadena_de_5000_nodos_en_serie_peor_caso_para_gradiente_conjugado() {
    let n = 5000;
    let mut nodes = vec![Node { id: "SRC".into(), elevation_m: 0.0, fixed_pressure_bar: Some(30.0), demand_lpm: 0.0, device_k: None }];
    for i in 0..n {
        nodes.push(Node {
            id: format!("C{}", i),
            elevation_m: 0.0,
            fixed_pressure_bar: None,
            demand_lpm: 0.0,
            device_k: if i % 10 == 9 { Some(50.0) } else { None },
        });
    }
    let mut links = Vec::new();
    for i in 0..n {
        links.push(Link {
            id: format!("L{}", i),
            from: i,
            to: i + 1,
            kind: LinkKind::Power { r: r_of(100.0, 3.0), n: 1.85 },
            check_valve: false,
            q0_lpm: 100.0,
        });
    }
    timed("cadena", &Network { nodes, links, gamma: 0.0980665 });
}

#[test]
fn peine_ramificado_5100_nodos() {
    // colector de 100 nodos con un ramal de 50 nodos en cada uno; rociador al final de cada ramal
    let (m, b) = (100usize, 50usize);
    let mut nodes = vec![Node { id: "SRC".into(), elevation_m: 0.0, fixed_pressure_bar: Some(15.0), demand_lpm: 0.0, device_k: None }];
    let mut links = Vec::new();
    let mut prev = 0usize;
    for i in 0..m {
        nodes.push(Node { id: format!("M{}", i), elevation_m: 3.0, fixed_pressure_bar: None, demand_lpm: 0.0, device_k: None });
        let mi = nodes.len() - 1;
        links.push(Link {
            id: format!("PM{}", i),
            from: prev,
            to: mi,
            kind: LinkKind::Power { r: r_of(150.0, 3.0), n: 1.85 },
            check_valve: false,
            q0_lpm: 800.0,
        });
        prev = mi;
        let mut bprev = mi;
        for j in 0..b {
            nodes.push(Node {
                id: format!("B{}_{}", i, j),
                elevation_m: 3.0,
                fixed_pressure_bar: None,
                demand_lpm: 0.0,
                device_k: if j == b - 1 || j % 7 == 6 { Some(80.0) } else { None },
            });
            let bi = nodes.len() - 1;
            links.push(Link {
                id: format!("PB{}_{}", i, j),
                from: bprev,
                to: bi,
                kind: LinkKind::Power { r: r_of(50.0, 3.0), n: 1.85 },
                check_valve: false,
                q0_lpm: 50.0,
            });
            bprev = bi;
        }
    }
    timed("peine", &Network { nodes, links, gamma: 0.0980665 });
}
