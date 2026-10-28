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
