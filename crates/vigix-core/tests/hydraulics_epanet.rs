//! Validación cruzada del solver hidráulico contra EPANET 2.2 (vía WNTR).
//!
//! Los fixtures los genera `tests/referencia/generar_epanet.py` (semillas fijas). Es una
//! referencia INDEPENDIENTE pero NO normativa: no sustituye benchmarks firmados por el
//! especialista (contrato 11.2). Tolerancia aquí: 2e-5 relativa (EPANET opera en simple
//! precisión), muy por debajo del 1 % exigido por el contrato.

use std::fs;
use std::path::PathBuf;
use vigix_core::hydraulics::solver::{solve, Link, LinkKind, Network, Node, Options};
use vigix_core::json::Json;

fn fixtures() -> Vec<(String, Json)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/epanet");
    let mut v: Vec<_> = fs::read_dir(&dir)
        .expect("existe el directorio de fixtures")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
        .collect();
    v.sort();
    v.into_iter()
        .map(|p| {
            let name = p.file_stem().unwrap().to_string_lossy().to_string();
            let txt = fs::read_to_string(&p).unwrap();
            (name, Json::parse(&txt).unwrap())
        })
        .collect()
}

fn build(spec: &Json) -> Network {
    let gamma = spec.field("gamma_bar_per_m").as_f64().unwrap();
    let (kk, n, m) = (
        spec.at("hw.K").and_then(Json::as_f64).unwrap(),
        spec.at("hw.n").and_then(Json::as_f64).unwrap(),
        spec.at("hw.m").and_then(Json::as_f64).unwrap(),
    );
    let mut nodes = Vec::new();
    for nj in spec.field("nodes").items() {
        nodes.push(Node {
            id: nj.field("id").as_str().unwrap().to_string(),
            elevation_m: nj.field("elevation_m").as_f64().unwrap(),
            fixed_pressure_bar: nj.field("fixed_pressure_bar").as_f64(),
            demand_lpm: nj.field("demand_lpm").as_f64().unwrap_or(0.0),
            device_k: nj.field("device_k").as_f64(),
        });
    }
    let idx = |id: &str| nodes.iter().position(|x| x.id == id).unwrap();
    let mut links = Vec::new();
    for lj in spec.field("links").items() {
        let id = lj.field("id").as_str().unwrap().to_string();
        let (a, b) = (idx(lj.field("from").as_str().unwrap()), idx(lj.field("to").as_str().unwrap()));
        match lj.field("kind").as_str().unwrap() {
            "pipe" => {
                let (len, d, c) =
                    (lj.field("length_m").as_f64().unwrap(), lj.field("diameter_mm").as_f64().unwrap(), lj.field("c").as_f64().unwrap());
                let r = kk * len / (c.powf(n) * d.powf(m));
                let q0 = std::f64::consts::FRAC_PI_4 * (d / 1000.0).powi(2) * 60000.0;
                links.push(Link {
                    id,
                    from: a,
                    to: b,
                    kind: LinkKind::Power { r, n },
                    check_valve: lj.field("check_valve").as_bool() == Some(true),
                    q0_lpm: q0,
                });
            }
            "pump" => {
                let curve: Vec<(f64, f64)> = lj
                    .field("curve")
                    .items()
                    .iter()
                    .map(|p| (p.idx(0).and_then(Json::as_f64).unwrap(), p.idx(1).and_then(Json::as_f64).unwrap()))
                    .collect();
                links.push(Link { id, from: a, to: b, kind: LinkKind::Pump { curve }, check_valve: true, q0_lpm: 300.0 });
            }
            other => panic!("tipo de tramo no soportado en fixture: {}", other),
        }
    }
    Network { nodes, links, gamma }
}

#[test]
fn todos_los_casos_coinciden_con_epanet() {
    let cases = fixtures();
    assert!(cases.len() >= 9, "se esperaban al menos 9 fixtures, hay {}", cases.len());
    let mut worst_flow = 0.0f64;
    let mut worst_p = 0.0f64;
    for (name, spec) in &cases {
        let net = build(spec);
        let sol = solve(&net, None, &Options::default(), None).unwrap_or_else(|e| panic!("{}: {:?}", name, e));
        assert!(sol.converged, "{}: no convergió (iter {}, rel {})", name, sol.iterations, sol.rel_change);
        assert!(sol.rel_imbalance <= 1e-3, "{}: residuo de continuidad {} supera 0,1 %", name, sol.rel_imbalance);
        let ep = spec.field("epanet");
        for (k, l) in net.links.iter().enumerate() {
            let qe = ep.at(&format!("flow_lpm.{}", l.id)).and_then(Json::as_f64).unwrap();
            let qm = sol.flow_lpm[k];
            // Piso absoluto de 2e-3 L/min. En ramales ciegos el caudal físico es 0 y EPANET
            // (precisión simple) deja ~1e-3 L/min: medido sobre la propia solución de EPANET, el
            // desequilibrio nodal llega a 9,7e-4 L/min (aleatoria_23, nodo N4), mientras que el de
            // VIGIX es <= 1e-6 relativo. No es una diferencia de modelo sino el ruido de la referencia.
            let err = (qm - qe).abs() / qe.abs().max(1.0);
            if qe.abs() >= 1.0 {
                worst_flow = worst_flow.max(err);
            }
            assert!(
                (qm - qe).abs() <= 2e-5 * qe.abs().max(1.0) + 2e-3,
                "{}: caudal del tramo {}: vigix={} epanet={} err_rel={:.3e}",
                name,
                l.id,
                qm,
                qe,
                err
            );
        }
        for (i, n) in net.nodes.iter().enumerate() {
            let pe = ep.at(&format!("pressure_bar.{}", n.id)).and_then(Json::as_f64).unwrap();
            if n.fixed_pressure_bar.is_some() {
                continue; // en depósitos EPANET devuelve ~0 de presión relativa
            }
            let pm = sol.pressure_bar[i];
            let err = (pm - pe).abs() / pe.abs().max(1.0);
            worst_p = worst_p.max(err);
            assert!(err <= 2e-5, "{}: presión del nodo {}: vigix={} epanet={} err_rel={:.3e}", name, n.id, pm, pe, err);
        }
    }
    eprintln!("peor error relativo de caudal (tramos >= 1 L/min): {:.3e}; de presión: {:.3e} ({} casos)", worst_flow, worst_p, cases.len());
}

#[test]
fn el_camino_pcg_da_lo_mismo_que_cholesky() {
    // Forzamos PCG con dense_limit = 0 y comparamos con el camino directo.
    for (name, spec) in fixtures() {
        let net = build(&spec);
        let direct = solve(&net, None, &Options::default(), None).unwrap();
        let iter = solve(&net, None, &Options { dense_limit: 0, ..Options::default() }, None).unwrap();
        assert_eq!(iter.linear_solver, "pcg-jacobi", "{}", name);
        assert!(iter.converged, "{}: PCG no convergió", name);
        for k in 0..net.links.len() {
            let (a, b) = (direct.flow_lpm[k], iter.flow_lpm[k]);
            assert!((a - b).abs() <= 1e-6 * a.abs().max(1.0) + 1e-4, "{}: tramo {} {} vs {}", name, net.links[k].id, a, b);
        }
    }
}
