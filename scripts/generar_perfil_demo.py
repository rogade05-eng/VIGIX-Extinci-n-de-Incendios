#!/usr/bin/env python3
"""Genera profiles/demo/DEMO-NO-NORMATIVO_<versión>.json.

PERFIL DE DEMOSTRACIÓN: todos los valores son FICTICIOS y están marcados "DEMO". Sirve para
probar el software y la interfaz; NO representa ninguna norma ni código y nunca puede quedar
como "verificado" (el motor lo impide por `demo: true`).

Uso:  python scripts/generar_perfil_demo.py > profiles/demo/DEMO-NO-NORMATIVO_0.1.0.json
      vigix-cli seal-profile profiles/demo/DEMO-NO-NORMATIVO_0.1.0.json     # escribe la huella SHA-256
"""
import json, sys

REF = "DEMO: valor ficticio, sin fuente normativa"
HAZ = ["demo_low", "demo_medium", "demo_high"]
CLS = ["A", "B", "C"]


def num(v, unit, ref=REF):
    return {"value": v, "unit": unit, "ref": ref}


def rows_table(keys, unit, rows):
    return {"keys": keys, "value_unit": unit, "rows": rows}


profile = {
    "schema": "vigix.profile/1",
    "profile_code": "DEMO-NO-NORMATIVO",
    "profile_version": "0.1.0",
    "demo": True,
    "title": {
        "es": "PERFIL DE DEMOSTRACIÓN — VALORES FICTICIOS, NO NORMATIVO, SOLO PARA PROBAR EL SOFTWARE",
        "en": "DEMONSTRATION PROFILE — FICTITIOUS VALUES, NOT NORMATIVE, FOR SOFTWARE TESTING ONLY",
    },
    "family": "OTRA",
    "country": "ZZ",
    "region": "N/A",
    "authority": "N/A (perfil de demostración, sin autoridad)",
    "language": "es",
    "unit_system": "SI",
    "sources": [{
        "id": "DEMO", "body": "VIGIX (contratista)", "code": "DEMO-0",
        "title": "Valores ficticios de demostración; no proceden de ninguna norma",
        "edition": "0", "effective_date": "2026-10-28", "amendments": [],
        "license_note": "Sin contenido normativo ni texto protegido.",
    }],
    "scope": {
        "systems": ["portable_extinguishers", "wet_pipe_sprinkler", "foam", "gas_clean_agent", "gas_inert", "gas_co2",
                    "dry_chemical", "wet_chemical_kitchen", "condensed_aerosol"],
        "exclusions": [
            "Todo uso real: este perfil NO representa ninguna norma ni código.",
            "Cualquier resultado obtenido con él es solo una demostración del funcionamiento del software.",
        ],
    },
    "methods": {
        "hydraulics.friction": "hazen_williams.power_law_si.v1",
        "hydraulics.fittings": "equivalent_length_table.v1",
        "extinguishers.coverage": "travel_distance_and_area_per_unit.v1",
        "foam.surface": "foam.surface_application.v1",
        "foam.high_expansion": "foam.high_expansion_flooding.v1",
        "gas.clean_agent": "gas.halocarbon_total_flooding.v1",
        "gas.inert": "gas.inert_total_flooding.v1",
        "gas.co2": "gas.co2_total_flooding.v1",
        "dry_chemical.total_flooding": "dry_chemical.total_flooding.v1",
        "dry_chemical.local_application": "dry_chemical.local_application.v1",
        "wet_chemical.kitchen": "wet_chemical.listing_based.v1",
        "aerosol.total_flooding": "aerosol.total_flooding.v1",
    },
    "parameters": {
        "hydraulics": {
            "fluid": {"density": num(1000, "kg/m3", "DEMO: valor de prueba (agua), sin fuente normativa")},
            "friction": {
                "coefficient": num(605000, "-", "DEMO: constante de prueba de la forma H-W en base SI (bar, L/min, mm, m)"),
                "exponent_flow": num(1.85, "-"),
                "exponent_diameter": num(4.87, "-"),
            },
            "supply": {"curve_exponent": num(1.85, "-")},
            "limits": {"max_velocity": num(12, "m/s", "DEMO: límite ficticio, NO es un criterio de aceptación real")},
        },
        "extinguishers": {"max_mount_height": num(1.5, "m")},
        "foam": {
            "reserve_factor": num(1.2, "-"),
            "he_shrinkage_factor": num(1.15, "-"),
            "he_leakage_factor": num(1.2, "-"),
        },
        "gas": {
            "altitude_correction_applies": {"flag": True, "ref": REF},
            "occupied_limit": {"text": "noael", "ref": REF},
            "min_predischarge_delay": num(20, "s"),
            "clean_agent": {"max_discharge_time": num(10, "s"), "hold_time": num(10, "min")},
            "inert": {"max_discharge_time": num(60, "s"), "hold_time": num(10, "min")},
            "co2": {"max_discharge_time": num(60, "s"), "hold_time": num(20, "min")},
        },
        "dry_chemical": {"max_discharge_time": num(30, "s"), "local_min_time": num(30, "s")},
        "kitchen": {"fuel_shutoff_required": {"flag": True, "ref": REF}},
    },
    "tables": {},
    "validity": {"effective_date": "2026-10-28", "expires": None},
    "authorship": {
        "author": "VIGIX (contratista)", "reviewer": "", "date": "2026-10-28",
        "approval_status": "borrador", "approval_ref": "", "sha256": "",
    },
    "test_cases": [],
    "changelog": [{"version": "0.1.0", "date": "2026-10-28",
                   "change": "Perfil de demostración (hidráulica y extintores). Sin valor normativo."}],
    "compatibility": {"supersedes": None, "min_engine": "0.1.0",
                      "notes": "Perfil de demostración: no usar para proyectos reales."},
}

T = profile["tables"]
# --- hidráulica
fit = []
for name, small, big in [("elbow_90", 1.0, 2.0), ("tee_run", 0.5, 1.0), ("tee_branch", 2.0, 4.0)]:
    fit.append({"fitting": name, "from": 0, "to": 60, "value": small, "ref": REF})
    fit.append({"fitting": name, "from": 60, "to": 1000, "to_inclusive": True, "value": big, "ref": REF})
fit.append({"fitting": "gate_valve", "from": 0, "to": 1000, "to_inclusive": True, "value": 0.3, "ref": REF})
T["hydraulics.fittings_equiv_length"] = rows_table(["fitting"], "m", fit)
T["hydraulics.design_density"] = rows_table(["hazard_class"], "L/min/m2",
    [{"hazard_class": h, "value": v, "ref": REF} for h, v in zip(HAZ, [2.0, 4.0, 8.0])])
T["hydraulics.design_area"] = rows_table(["hazard_class"], "m2",
    [{"hazard_class": h, "value": v, "ref": REF} for h, v in zip(HAZ, [24.0, 36.0, 48.0])])
T["hydraulics.supply_duration"] = rows_table(["hazard_class"], "min",
    [{"hazard_class": h, "value": v, "ref": REF} for h, v in zip(HAZ, [10, 20, 30])])
# --- extintores
compat = {"water": [1, 0, 0], "foam": [1, 1, 0], "powder_abc": [1, 1, 1], "co2": [0, 1, 1]}
T["extinguishers.compat"] = rows_table(["agent", "fire_class"], "-",
    [{"agent": a, "fire_class": c, "value": v, "ref": REF} for a, vs in compat.items() for c, v in zip(CLS, vs)])
travel = {"demo_low": [20, 15, 15], "demo_medium": [15, 12, 12], "demo_high": [10, 9, 9]}
T["extinguishers.max_travel_distance"] = rows_table(["hazard_class", "fire_class"], "m",
    [{"hazard_class": h, "fire_class": c, "value": v, "ref": REF} for h, vs in travel.items() for c, v in zip(CLS, vs)])
minr = {"demo_low": [1, 1, 1], "demo_medium": [2, 2, 2], "demo_high": [3, 3, 3]}
T["extinguishers.min_rating"] = rows_table(["hazard_class", "fire_class"], "-",
    [{"hazard_class": h, "fire_class": c, "value": v, "ref": REF} for h, vs in minr.items() for c, v in zip(CLS, vs)])
area = {"demo_low": 100, "demo_medium": 80, "demo_high": 50}
T["extinguishers.max_area_per_unit"] = rows_table(["hazard_class", "fire_class"], "m2",
    [{"hazard_class": h, "fire_class": c, "value": v, "ref": REF} for h, v in area.items() for c in CLS])

# --- espuma
T["foam.application_rate"] = rows_table(["application", "concentrate_type"], "L/min/m2", [
    {"application": a, "concentrate_type": c, "value": v, "ref": REF}
    for a, c, v in [("bund", "demo_afff", 4.0), ("bund", "demo_protein", 6.0), ("tank", "demo_afff", 5.0)]])
T["foam.discharge_duration"] = rows_table(["application", "hazard_class"], "min", [
    {"application": a, "hazard_class": h, "value": v, "ref": REF}
    for a, h, v in [("bund", "demo_medium", 10), ("bund", "demo_high", 15), ("tank", "demo_medium", 20), ("tank", "demo_high", 30)]])
T["foam.he_submergence_time"] = rows_table(["hazard_class"], "min", [
    {"hazard_class": h, "value": v, "ref": REF} for h, v in [("demo_medium", 4), ("demo_high", 3)]])
T["foam.he_operating_time"] = rows_table(["hazard_class"], "min", [
    {"hazard_class": h, "value": v, "ref": REF} for h, v in [("demo_medium", 10), ("demo_high", 12)]])
# --- gases
def agent_rows(vals, unit_note=None):
    return [{"agent": a, "value": v, "ref": REF} for a, v in vals.items()]
T["gas.k1"] = rows_table(["agent"], "m3/kg", agent_rows({"demo_halocarbon": 0.15, "demo_inert": 0.65}))
T["gas.k2"] = rows_table(["agent"], "m3/kg/K", agent_rows({"demo_halocarbon": 0.0006, "demo_inert": 0.0024}))
T["gas.vs_reference"] = rows_table(["agent"], "m3/kg", agent_rows({"demo_inert": 0.70}))
T["gas.noael"] = rows_table(["agent"], "%", agent_rows({"demo_halocarbon": 9.0, "demo_inert": 43.0}))
T["gas.loael"] = rows_table(["agent"], "%", agent_rows({"demo_halocarbon": 10.5, "demo_inert": 52.0}))
T["gas.min_design_concentration"] = rows_table(["agent", "hazard_class"], "%", [
    {"agent": a, "hazard_class": h, "value": v, "ref": REF}
    for a, vs in {"demo_halocarbon": [7.0, 8.0, 9.0], "demo_inert": [34.0, 38.0, 42.0]}.items()
    for h, v in zip(HAZ, vs)])
T["gas.altitude_correction"] = rows_table(["family"], "-", [
    {"family": f, "from": lo, "to": hi, "to_inclusive": inc, "value": v, "ref": REF}
    for f in ["clean_agent", "inert"]
    for lo, hi, inc, v in [(-100, 500, False, 1.0), (500, 1500, False, 0.95), (1500, 3000, True, 0.9)]])
T["gas.co2.flooding_factor"] = rows_table(["hazard_class"], "kg/m3", [
    {"hazard_class": h, "value": v, "ref": REF} for h, v in zip(HAZ, [0.6, 0.8, 1.0])])
T["gas.co2.material_conversion_factor"] = rows_table(["material"], "-", [
    {"material": m, "value": v, "ref": REF} for m, v in [("standard", 1.0), ("demo_material_x", 1.3)]])
# --- polvo químico
T["dry_chemical.flooding_rate"] = rows_table(["agent", "hazard_class"], "kg/m3", [
    {"agent": "demo_powder", "hazard_class": h, "value": v, "ref": REF} for h, v in zip(HAZ, [0.5, 0.65, 0.8])])
T["dry_chemical.opening_factor"] = rows_table(["agent"], "kg/m2", [{"agent": "demo_powder", "value": 5.0, "ref": REF}])
T["dry_chemical.local_rate"] = rows_table(["agent", "hazard_class"], "kg/min/m2", [
    {"agent": "demo_powder", "hazard_class": h, "value": v, "ref": REF} for h, v in zip(HAZ, [2.0, 2.5, 3.0])])
# --- aerosol
T["aerosol.design_factor"] = rows_table(["hazard_class"], "-", [
    {"hazard_class": h, "value": v, "ref": REF} for h, v in zip(HAZ, [1.0, 1.1, 1.2])])


# ---------------------------------------------------------------------------------------------
# Casos de prueba INDEPENDIENTES: los resultados esperados se calculan aquí, en Python, con las
# fórmulas escritas de nuevo (sin usar el motor Rust). Si el motor y este script divergen, el
# autotest del perfil falla y el perfil deja de considerarse verificado.
import math


def hw_r(length, d_mm, c):
    return 605000.0 * length / (c ** 1.85 * d_mm ** 4.87)


def hyd_case():
    gamma = 1000.0 * 9.80665 / 1e5
    z = 3.0
    r1, r2 = hw_r(30, 65, 120), hw_r(10, 40, 120)

    def f(q):
        p_sup = 6.0 - (6.0 - 4.0) * (q / 1000.0) ** 1.85
        p_dev = p_sup - r1 * q ** 1.85 - r2 * (q / 2.0) ** 1.85 - gamma * z
        qd = 80.0 * math.sqrt(p_dev) if p_dev > 0 else 0.0
        return 2.0 * qd - q
    lo, hi = 0.0, 5000.0
    for _ in range(300):
        mid = 0.5 * (lo + hi)
        lo, hi = (mid, hi) if f(mid) > 0 else (lo, mid)
    q = 0.5 * (lo + hi)
    net = {
        "id": "NET1", "system_type": "wet_pipe_sprinkler",
        "nodes": [
            {"id": "SUP", "kind": "supply", "elevation_m": 0, "static_pressure_bar": 6.0, "residual_pressure_bar": 4.0, "test_flow_lpm": 1000},
            {"id": "J", "kind": "junction", "elevation_m": 0},
            {"id": "S1", "kind": "sprinkler", "elevation_m": z, "k_lpm_bar05": 80, "coverage_area_m2": 12, "min_pressure_bar": 0.5},
            {"id": "S2", "kind": "sprinkler", "elevation_m": z, "k_lpm_bar05": 80, "coverage_area_m2": 12, "min_pressure_bar": 0.5}],
        "links": [
            {"id": "P1", "kind": "pipe", "from": "SUP", "to": "J", "length_m": 30, "diameter_mm": 65, "c_factor": 120, "fittings": []},
            {"id": "P2", "kind": "pipe", "from": "J", "to": "S1", "length_m": 10, "diameter_mm": 40, "c_factor": 120, "fittings": []},
            {"id": "P3", "kind": "pipe", "from": "J", "to": "S2", "length_m": 10, "diameter_mm": 40, "c_factor": 120, "fittings": []}],
        "scenarios": [{"id": "SC1", "name": "Ambos", "active_devices": ["S1", "S2"]}],
    }
    return {
        "id": "HYD-001", "system": "wet_pipe_sprinkler", "module": "hydraulic",
        "input": {"network": net, "scenario": "SC1", "mode": "operating_point"},
        "expected": [
            {"path": "values.summary.total_demand_lpm", "value": q, "tolerance_rel": 1e-6},
            {"path": "values.convergence.converged", "equals": True},
        ],
        "source": "Independiente: ecuación escalar resuelta por bisección en Python (scripts/generar_perfil_demo.py)",
    }


def foam_cases():
    sysj = {"id": "FM1", "method": "surface_application", "hazard_class": "demo_high", "application": "bund",
            "concentrate": {"type": "demo_afff", "proportion_pct": 3}, "area_m2": 50}
    q = 4.0 * 50
    c1 = {"id": "FOAM-001", "system": "foam", "module": "foam", "input": {"system": sysj}, "expected": [
        {"path": "values.flows.solution_lpm", "value": q, "tolerance_rel": 1e-9},
        {"path": "values.flows.concentrate_lpm", "value": q * 0.03, "tolerance_rel": 1e-9},
        {"path": "values.volumes.concentrate_stored_l", "value": q * 0.03 * 15 * 1.2, "tolerance_rel": 1e-9}],
        "source": "Independiente: aritmética directa"}
    he = {"id": "FM2", "method": "high_expansion_flooding", "hazard_class": "demo_high", "enclosure_volume_m3": 600,
          "sprinklers_present": True, "sprinkler_breakdown_m3_min": 10,
          "generator": {"foam_output_m3_min": 120, "expansion_ratio": 500}, "concentrate": {"type": "demo_afff", "proportion_pct": 2}}
    r = (600 / 3 + 10) * 1.15 * 1.2
    c2 = {"id": "FOAM-002", "system": "foam", "module": "foam", "input": {"system": he}, "expected": [
        {"path": "values.foam_discharge_m3_min", "value": r, "tolerance_rel": 1e-9},
        {"path": "values.generators_required", "value": math.ceil(r / 120), "tolerance_abs": 0},
        {"path": "values.volumes.concentrate_stored_l", "value": r * 1000 / 500 * 0.02 * 12 * 1.2, "tolerance_rel": 1e-9}],
        "source": "Independiente: aritmética directa"}
    return [c1, c2]


def gas_cases():
    base = {"hazard_class": "demo_medium", "occupied": False, "discharge_time_s": 10,
            "enclosure": {"volume_m3": 200, "min_temperature_c": 20, "altitude_m": 100}}
    s1 = dict(base, id="G1", family="clean_agent", agent="demo_halocarbon", design_concentration_pct=8.5,
              cylinder={"max_fill_kg": 60, "min_fill_kg": 20}, hold_time_min=12)
    s = 0.15 + 0.0006 * 20
    w = (200 / s) * (8.5 / 91.5)
    s2 = dict(base, id="G2", family="inert", agent="demo_inert", design_concentration_pct=40, discharge_time_s=50,
              enclosure={"volume_m3": 300, "min_temperature_c": 15, "altitude_m": 600}, cylinder={"gas_volume_m3": 12})
    si = 0.65 + 0.0024 * 15
    vgas = 300 * (0.70 / si) * math.log(100 / 60) * 0.95
    s3 = dict(base, id="G3", family="co2", hazard_class="demo_high", material="demo_material_x", discharge_time_s=45,
              enclosure={"volume_m3": 150, "min_temperature_c": 20, "altitude_m": 0}, cylinder={"max_fill_kg": 45, "min_fill_kg": 30})
    return [
        {"id": "GAS-001", "system": "gas_clean_agent", "module": "gas", "input": {"system": s1}, "expected": [
            {"path": "values.agent_mass_kg", "value": w, "tolerance_rel": 1e-9}, {"path": "values.cylinders.count", "value": math.ceil(w / 60), "tolerance_abs": 0}],
         "source": "Independiente: W = (V/S)*C/(100-C), S = k1 + k2*T"},
        {"id": "GAS-002", "system": "gas_inert", "module": "gas", "input": {"system": s2}, "expected": [
            {"path": "values.gas_volume_reference_m3", "value": vgas, "tolerance_rel": 1e-9}],
         "source": "Independiente: V_gas = V*(Vs/S)*ln(100/(100-C))*Ca"},
        {"id": "GAS-003", "system": "gas_co2", "module": "gas", "input": {"system": s3}, "expected": [
            {"path": "values.agent_mass_kg", "value": 150 * 1.0 * 1.3, "tolerance_rel": 1e-9}],
         "source": "Independiente: W = V*FF*MCF"},
    ]


def other_cases():
    dc = {"id": "DC1", "mode": "total_flooding", "agent": "demo_powder", "hazard_class": "demo_medium", "occupied": False,
          "discharge_time_s": 25, "enclosure": {"volume_m3": 120, "open_area_m2": 2}, "nozzles": {"count": 4, "flow_kg_s": 1.5},
          "cylinder": {"capacity_kg": 45}}
    kit = {"id": "K1", "listing_ref": "Listado de demostración", "tank": {"max_flow_numbers": 12},
           "appliances": [{"id": "A1", "type": "fryer", "area_m2": 0.9, "nozzle": {"coverage_area_m2": 0.5, "flow_number": 1}, "nozzle_count": 2},
                          {"id": "A2", "type": "griddle", "area_m2": 0.6, "nozzle": {"coverage_area_m2": 0.6, "flow_number": 2}, "nozzle_count": 1}],
           "plenum": {"required_nozzles": 1, "nozzle_count": 1, "flow_number_each": 2},
           "duct": {"required_nozzles": 1, "nozzle_count": 1, "flow_number_each": 2}, "fuel_shutoff": {"provided": True}}
    ae = {"id": "AE1", "hazard_class": "demo_high", "occupied": False, "actual_clearance_m": 1.5, "enclosure": {"volume_m3": 90},
          "product": {"design_density_g_m3": 100, "unit_mass_g": 2500, "max_volume_per_unit_m3": 40, "min_clearance_m": 1.0, "listing_ref": "Listado de demostración"}}
    room = {"id": "R1", "type": "room", "pts": [[0, 0], [6, 0], [6, 6], [0, 6]], "hazard_class": "demo_low", "fire_classes": ["A"]}
    proj = {"site": {"levels": [{"id": "L1", "entities": [room]}]},
            "systems": {"extinguishers": {"analysis": {"grid_m": 0.25}, "units": [
                {"id": "E1", "level": "L1", "x": 3, "y": 3, "agent": "water", "ratings": {"A": 5}}]}}}
    return [
        {"id": "DRY-001", "system": "dry_chemical", "module": "dry_chemical", "input": {"system": dc}, "expected": [
            {"path": "values.agent_mass_kg", "value": 120 * 0.65 + 2 * 5.0, "tolerance_rel": 1e-9}, {"path": "values.cylinders_required", "value": 2, "tolerance_abs": 0}],
         "source": "Independiente: W = V*tasa + A*factor"},
        {"id": "KIT-001", "system": "wet_chemical_kitchen", "module": "wet_chemical", "input": {"system": kit}, "expected": [
            {"path": "values.total_flow_numbers", "value": 2 * 1 + 1 * 2 + 2 + 2, "tolerance_abs": 0}],
         "source": "Independiente: suma de números de flujo"},
        {"id": "AER-001", "system": "condensed_aerosol", "module": "aerosol", "input": {"system": ae}, "expected": [
            {"path": "values.agent_mass_g", "value": 90 * 100 * 1.2, "tolerance_rel": 1e-9}, {"path": "values.units_required", "value": 5, "tolerance_abs": 0}],
         "source": "Independiente: m = V*densidad*factor"},
        {"id": "EXT-001", "system": "portable_extinguishers", "module": "extinguishers", "input": {"project": proj}, "expected": [
            {"path": "values.summary.uncovered_area_total_m2", "value": 0, "tolerance_abs": 1e-9}, {"path": "values.summary.zones_ok", "value": 1, "tolerance_abs": 0}],
         "source": "Independiente: sala 6x6 con la unidad en el centro; el punto más lejano (esquina) está a 4,24 m < 20 m"},
    ]


profile["test_cases"] = [hyd_case()] + foam_cases() + gas_cases() + other_cases()


def dump(o, ind=0):
    """JSON legible: los objetos 'fila' (sin anidación) en una sola línea."""
    pad = "  " * ind
    if isinstance(o, dict):
        if all(not isinstance(v, (dict, list)) for v in o.values()):
            return json.dumps(o, ensure_ascii=False)
        items = [f'{pad}  {json.dumps(k, ensure_ascii=False)}: {dump(v, ind + 1)}' for k, v in o.items()]
        return "{\n" + ",\n".join(items) + f"\n{pad}}}"
    if isinstance(o, list):
        if not o:
            return "[]"
        if all(not isinstance(v, (dict, list)) for v in o):
            return json.dumps(o, ensure_ascii=False)
        items = [f"{pad}  {dump(v, ind + 1)}" for v in o]
        return "[\n" + ",\n".join(items) + f"\n{pad}]"
    return json.dumps(o, ensure_ascii=False)


sys.stdout.write(dump(profile) + "\n")
