#!/usr/bin/env python3
"""Genera fixtures de referencia hidráulica con EPANET 2.2 (vía WNTR).

REFERENCIA INDEPENDIENTE, NO NORMATIVA: EPANET es un solver reconocido de redes a presión,
pero estos casos NO sustituyen los benchmarks firmados por el especialista (contrato 11.2).
Solo demuestran que el solver de VIGIX resuelve las mismas ecuaciones que un motor de
referencia externo con tolerancia muy inferior al 1 % exigido.

Requisitos (solo para regenerar; los fixtures ya están versionados):
    pip install wntr==1.5.0
Uso:
    python tests/referencia/generar_epanet.py crates/vigix-core/tests/fixtures/epanet

Mapeo de unidades (todo SI en EPANET; VIGIX trabaja en bar, L/min, mm):
    carga fija:   head_m = P_bar/gamma + z          (gamma = 0.0980665 bar/m)
    dispositivo:  coef. de emisor c = K/60000*sqrt(gamma)   [m3/s por raiz(m)]
    Hazen–Williams EPANET SI: h[m] = 10.667 L Q^1.852 / (C^1.852 d^4.871)
      => K_vigix = 10.667*gamma*(1/60000)^1.852*(1000)^4.871  con n=1.852, m=4.871
"""
import json, math, os, random, sys
import wntr

GAMMA = 0.0980665
HW_K = 10.667 * GAMMA * (1 / 60000.0) ** 1.852 * 1000.0 ** 4.871
HW_N, HW_M = 1.852, 4.871


def run(spec):
    wn = wntr.network.WaterNetworkModel()
    for n in spec["nodes"]:
        if "fixed_pressure_bar" in n:
            wn.add_reservoir(n["id"], base_head=n["fixed_pressure_bar"] / GAMMA + n["elevation_m"])
        else:
            wn.add_junction(n["id"], base_demand=n.get("demand_lpm", 0.0) / 60000.0, elevation=n["elevation_m"])
            if n.get("device_k"):
                wn.get_node(n["id"]).emitter_coefficient = n["device_k"] / 60000.0 * math.sqrt(GAMMA)
    for i, l in enumerate(spec["links"]):
        if l["kind"] == "pipe":
            wn.add_pipe(l["id"], l["from"], l["to"], length=l["length_m"], diameter=l["diameter_mm"] / 1000.0,
                        roughness=l["c"], minor_loss=0.0, check_valve=l.get("check_valve", False))
        elif l["kind"] == "pump":
            pts = [(q / 60000.0, h / GAMMA) for q, h in l["curve"]]
            assert len(pts) >= 4 or len(pts) == 2, "EPANET ajusta potencia con 1 o 3 puntos; usar 2 o >=4"
            wn.add_curve("c_" + l["id"], "HEAD", pts)
            wn.add_pump(l["id"], l["from"], l["to"], pump_type="HEAD", pump_parameter="c_" + l["id"])
    wn.options.hydraulic.headloss = "H-W"
    wn.options.hydraulic.accuracy = 1e-9
    wn.options.hydraulic.trials = 2000
    wn.options.hydraulic.demand_model = "DDA"
    wn.options.time.duration = 0
    res = wntr.sim.EpanetSimulator(wn).run_sim()
    head_m = res.node["head"].iloc[0].to_dict()
    flow = res.link["flowrate"].iloc[0].to_dict()
    out = dict(spec)
    out["epanet"] = {
        "head_bar": {k: v * GAMMA for k, v in head_m.items()},
        "pressure_bar": {n["id"]: (head_m[n["id"]] - n["elevation_m"]) * GAMMA for n in spec["nodes"]},
        "flow_lpm": {k: v * 60000.0 for k, v in flow.items()},
    }
    return out


def base(name, desc):
    return {"name": name, "description": desc, "gamma_bar_per_m": GAMMA,
            "hw": {"K": HW_K, "n": HW_N, "m": HW_M}, "nodes": [], "links": []}


def case_tree():
    s = base("arbol_rociadores", "Depósito -> principal -> 2 ramales con 4 rociadores cada uno (K 80 y 115), cotas distintas")
    s["nodes"].append({"id": "R", "elevation_m": 0.0, "fixed_pressure_bar": 6.0})
    s["nodes"].append({"id": "M1", "elevation_m": 4.0})
    s["links"].append({"id": "PM", "kind": "pipe", "from": "R", "to": "M1", "length_m": 60.0, "diameter_mm": 100.0, "c": 120})
    for b, (z, k) in enumerate([(4.0, 80.0), (5.0, 115.0)]):
        prev = "M1"
        for j in range(4):
            nid = f"B{b}S{j}"
            s["nodes"].append({"id": nid, "elevation_m": z, "device_k": k})
            s["links"].append({"id": f"PB{b}{j}", "kind": "pipe", "from": prev, "to": nid, "length_m": 3.5,
                               "diameter_mm": [65.0, 52.0, 40.0, 32.0][j], "c": 120})
            prev = nid
    return s


def case_grid():
    s = base("malla_3x3", "Malla 3x3 con una fuente en una esquina, 4 dispositivos y 2 demandas fijas")
    s["nodes"].append({"id": "R", "elevation_m": 0.0, "fixed_pressure_bar": 7.5})
    for i in range(3):
        for j in range(3):
            n = {"id": f"G{i}{j}", "elevation_m": 3.0 + 0.5 * i}
            if (i, j) in [(2, 2), (0, 2), (2, 0), (1, 1)]:
                n["device_k"] = 80.0 if (i, j) != (1, 1) else 115.0
            if (i, j) in [(0, 1), (1, 2)]:
                n["demand_lpm"] = 90.0
            s["nodes"].append(n)
    s["links"].append({"id": "PR", "kind": "pipe", "from": "R", "to": "G00", "length_m": 25.0, "diameter_mm": 100.0, "c": 120})
    k = 0
    for i in range(3):
        for j in range(3):
            if j < 2:
                s["links"].append({"id": f"H{k}", "kind": "pipe", "from": f"G{i}{j}", "to": f"G{i}{j+1}", "length_m": 8.0 + i,
                                   "diameter_mm": 65.0 if i == 0 else 52.0, "c": 120 if (i + j) % 2 == 0 else 100})
                k += 1
            if i < 2:
                s["links"].append({"id": f"V{k}", "kind": "pipe", "from": f"G{i}{j}", "to": f"G{i+1}{j}", "length_m": 7.0 + j,
                                   "diameter_mm": 52.0 if j == 0 else 40.0, "c": 120})
                k += 1
    return s


def case_pump():
    s = base("bomba_deposito_malla", "Depósito a presión 0 -> bomba (curva de 4 puntos) -> lazo -> 3 dispositivos")
    s["nodes"] += [
        {"id": "T", "elevation_m": 0.0, "fixed_pressure_bar": 0.0},
        {"id": "D", "elevation_m": 0.0},
        {"id": "A", "elevation_m": 6.0}, {"id": "B", "elevation_m": 6.0},
        {"id": "C", "elevation_m": 8.0, "device_k": 115.0},
        {"id": "E", "elevation_m": 8.0, "device_k": 115.0},
        {"id": "F", "elevation_m": 8.0, "device_k": 80.0, "demand_lpm": 0.0},
    ]
    s["links"] += [
        {"id": "PU", "kind": "pump", "from": "T", "to": "D", "curve": [[0, 12.0], [600, 11.2], [1200, 9.5], [1800, 6.0]]},
        {"id": "PD", "kind": "pipe", "from": "D", "to": "A", "length_m": 40.0, "diameter_mm": 100.0, "c": 120},
        {"id": "PA", "kind": "pipe", "from": "A", "to": "B", "length_m": 20.0, "diameter_mm": 80.0, "c": 120},
        {"id": "PA2", "kind": "pipe", "from": "A", "to": "B", "length_m": 26.0, "diameter_mm": 65.0, "c": 110},
        {"id": "PBC", "kind": "pipe", "from": "B", "to": "C", "length_m": 12.0, "diameter_mm": 65.0, "c": 120},
        {"id": "PBE", "kind": "pipe", "from": "B", "to": "E", "length_m": 15.0, "diameter_mm": 65.0, "c": 120},
        {"id": "PCF", "kind": "pipe", "from": "C", "to": "F", "length_m": 9.0, "diameter_mm": 52.0, "c": 120},
    ]
    return s


def case_two_sources():
    s = base("dos_fuentes_retencion", "Dos fuentes con válvula de retención hacia un nodo común; demanda fija y dispositivo")
    s["nodes"] += [
        {"id": "S1", "elevation_m": 0.0, "fixed_pressure_bar": 5.5},
        {"id": "S2", "elevation_m": 0.0, "fixed_pressure_bar": 4.2},
        {"id": "M", "elevation_m": 2.0, "demand_lpm": 400.0},
        {"id": "N", "elevation_m": 2.0, "device_k": 115.0},
    ]
    s["links"] += [
        {"id": "P1", "kind": "pipe", "from": "S1", "to": "M", "length_m": 80.0, "diameter_mm": 100.0, "c": 120, "check_valve": True},
        {"id": "P2", "kind": "pipe", "from": "S2", "to": "M", "length_m": 30.0, "diameter_mm": 100.0, "c": 120, "check_valve": True},
        {"id": "PN", "kind": "pipe", "from": "M", "to": "N", "length_m": 20.0, "diameter_mm": 65.0, "c": 120},
    ]
    return s


def case_random(seed, n_nodes):
    rnd = random.Random(seed)
    s = base(f"aleatoria_{seed}", f"Red aleatoria reproducible (semilla {seed}, {n_nodes} nodos libres) con lazos")
    s["nodes"].append({"id": "R", "elevation_m": 0.0, "fixed_pressure_bar": round(rnd.uniform(7.0, 10.0), 3)})
    ids = []
    for i in range(n_nodes):
        n = {"id": f"N{i}", "elevation_m": round(rnd.uniform(0.0, 6.0), 2)}
        r = rnd.random()
        if r < 0.3:
            n["device_k"] = rnd.choice([57.0, 80.0, 115.0, 160.0])
        elif r < 0.45:
            n["demand_lpm"] = round(rnd.uniform(20.0, 120.0), 1)
        s["nodes"].append(n)
        ids.append(n["id"])
    dia = [40.0, 52.0, 65.0, 80.0, 100.0]
    # árbol de expansión aleatorio + aristas extra (lazos)
    connected = ["R"]
    k = 0
    for nid in ids:
        a = rnd.choice(connected)
        s["links"].append({"id": f"T{k}", "kind": "pipe", "from": a, "to": nid, "length_m": round(rnd.uniform(3.0, 25.0), 2),
                           "diameter_mm": rnd.choice(dia[1:]) if a == "R" else rnd.choice(dia), "c": rnd.choice([100, 120, 140])})
        connected.append(nid)
        k += 1
    for _ in range(max(2, n_nodes // 4)):
        a, b = rnd.sample(ids, 2)
        s["links"].append({"id": f"L{k}", "kind": "pipe", "from": a, "to": b, "length_m": round(rnd.uniform(3.0, 25.0), 2),
                           "diameter_mm": rnd.choice(dia), "c": rnd.choice([100, 120, 140])})
        k += 1
    return s


def main():
    out_dir = sys.argv[1]
    os.makedirs(out_dir, exist_ok=True)
    cases = [case_tree(), case_grid(), case_pump(), case_two_sources()]
    for seed, n in [(11, 18), (23, 30), (37, 45), (41, 60), (53, 24)]:
        cases.append(case_random(seed, n))
    for c in cases:
        r = run(c)
        pr = [v for k, v in r["epanet"]["pressure_bar"].items() if any(n["id"] == k and n.get("device_k") for n in c["nodes"])]
        assert all(p > 0.2 for p in pr), (c["name"], "presión de dispositivo no positiva en EPANET", min(pr))
        path = os.path.join(out_dir, c["name"] + ".json")
        with open(path, "w", encoding="utf-8") as f:
            json.dump(r, f, ensure_ascii=False, indent=1, sort_keys=True)
        print("ok", c["name"], "nodos", len(c["nodes"]), "tramos", len(c["links"]), "min P dispositivos %.3f bar" % min(pr))


if __name__ == "__main__":
    main()
