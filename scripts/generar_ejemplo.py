#!/usr/bin/env python3
"""Genera examples/ejemplo_edificio_demo.json: proyecto de EJEMPLO (datos ficticios y anonimizados)
que ejercita todos los módulos con el perfil de DEMOSTRACIÓN. No contiene resultados: se calculan
al abrirlo ("Calcular todo"). Uso: python scripts/generar_ejemplo.py > examples/ejemplo_edificio_demo.json
"""
import json, sys

Z = 2.8
wall = lambda i, pts: {"id": i, "type": "wall", "pts": pts, "thickness_m": 0.2}
room = lambda i, name, x0, y0, x1, y1, haz, cls: {"id": i, "type": "room", "name": name, "hazard_class": haz, "fire_classes": cls,
                                                  "pts": [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]}
entities = [
    room("R1", "Oficina", 0, 0, 10, 8, "demo_low", ["A", "C"]),
    room("R2", "Bodega", 10, 0, 20, 8, "demo_medium", ["A", "B"]),
    room("R3", "Sala de servidores", 20, 0, 26, 8, "demo_high", ["A", "C"]),
    wall("W1", [[0, 0], [26, 0], [26, 8], [0, 8], [0, 0]]),
    wall("W2", [[10, 0], [10, 8]]),
    wall("W3", [[20, 0], [20, 8]]),
    {"id": "D1", "type": "door", "pts": [[10, 3], [10, 4]]},
    {"id": "D2", "type": "door", "pts": [[20, 3], [20, 4]]},
]
ext = lambda i, x, y, agent, rat: {"id": i, "level": "L1", "x": x, "y": y, "agent": agent, "ratings": rat, "mount_height_m": 1.2}
units = [ext("E1", 9.4, 4, "powder_abc", {"A": 3, "B": 3, "C": 3}),
         ext("E2", 10.6, 4, "powder_abc", {"A": 3, "B": 3, "C": 3}),
         ext("E3", 20.6, 4, "co2", {"B": 3, "C": 3}),
         ext("E4", 19.4, 5, "water", {"A": 3})]
spr = lambda i, x, y: {"id": i, "kind": "sprinkler", "level": "L1", "x": x, "y": y, "elevation_m": Z, "k_lpm_bar05": 80,
                       "coverage_area_m2": 12, "min_pressure_bar": 0.5}
nodes = [
    {"id": "SUP", "kind": "supply", "level": "L1", "x": -2, "y": 4, "elevation_m": 0, "static_pressure_bar": 6.0,
     "residual_pressure_bar": 4.0, "test_flow_lpm": 1000},
    {"id": "J0", "kind": "junction", "level": "L1", "x": 0.5, "y": 4, "elevation_m": Z},
    {"id": "J1", "kind": "junction", "level": "L1", "x": 5, "y": 4, "elevation_m": Z},
    {"id": "J2", "kind": "junction", "level": "L1", "x": 15, "y": 4, "elevation_m": Z},
    spr("S1", 5, 2), spr("S2", 5, 6), spr("S3", 15, 2), spr("S4", 15, 6),
]
pipe = lambda i, a, b, L, d, fit: {"id": i, "kind": "pipe", "from": a, "to": b, "length_m": L, "diameter_mm": d, "c_factor": 120, "fittings": fit}
links = [
    pipe("P0", "SUP", "J0", 12.0, 65, [{"type": "elbow_90", "count": 2}, {"type": "gate_valve", "count": 1}]),
    pipe("P1", "J0", "J1", 4.5, 65, []), pipe("P2", "J1", "J2", 10.0, 52, [{"type": "tee_run", "count": 1}]),
    pipe("P3", "J1", "S1", 2.0, 40, [{"type": "tee_branch", "count": 1}]), pipe("P4", "J1", "S2", 2.0, 40, [{"type": "elbow_90", "count": 1}]),
    pipe("P5", "J2", "S3", 2.0, 40, []), pipe("P6", "J2", "S4", 2.0, 40, [{"type": "elbow_90", "count": 1}]),
]
scenarios = [
    {"id": "SC1", "name": "Oficina: 2 rociadores (punto de operación)", "active_devices": ["S1", "S2"], "hazard_class": "demo_low", "mode": "operating_point"},
    {"id": "SC2", "name": "Bodega: presión requerida en la fuente", "active_devices": ["S3", "S4"], "hazard_class": "demo_low", "required_source": "SUP", "mode": "required_pressure"},
]
proj = {
    "schema": "vigix.project/1",
    "app": {"name": "VIGIX EXTINCIÓN FIJA", "version": "0.1.0"},
    "meta": {"id": "EJEMPLO-DEMO-001", "name": "Edificio de ejemplo (DEMOSTRACIÓN — datos ficticios)", "client": "Cliente de ejemplo S.A.",
             "address": "Calle Ficticia 123", "author": "", "reviewer": "", "status": "borrador", "mode": "new_design",
             "design_date": "2026-10-28", "created_at": "2026-10-28T00:00:00Z", "updated_at": "2026-10-28T00:00:00Z", "revision": 1, "revisions": []},
    "jurisdiction": {"country": "ZZ", "region": "N/A", "authority": "N/A", "profile_code": "DEMO-NO-NORMATIVO", "profile_version": "0.1.0", "profile_sha256": None},
    "display": {"unit_system": "SI", "language": "es", "decimals": 2},
    "site": {"occupancy": "oficinas y almacén (ficticio)", "building_use": "mixto", "height_m": 3.0,
             "levels": [{"id": "L1", "name": "Planta baja", "elevation_m": 0, "height_m": 3.0, "entities": entities}],
             "hazards": [], "restrictions": ""},
    "water_supply": {"type": "red pública (ficticia)", "static_pressure_bar": 6.0, "residual_pressure_bar": 4.0, "test_flow_lpm": 1000,
                     "elevation_m": 0, "limitations": "Datos de ensayo ficticios"},
    "assumptions": [{"id": "S1", "text": "Los datos de suministro de agua provienen de un ensayo ficticio.", "confirmed": False, "module": "water"}],
    "systems": {
        "extinguishers": {"analysis": {"grid_m": 0.25}, "units": units},
        "water": {"networks": [{"id": "NET1", "name": "Rociadores planta baja", "system_type": "wet_pipe_sprinkler", "nodes": nodes, "links": links, "scenarios": scenarios}]},
        "foam": [{"id": "FM1", "name": "Cubeto de líquido (ejemplo)", "method": "surface_application", "hazard_class": "demo_high", "application": "bund",
                  "concentrate": {"type": "demo_afff", "proportion_pct": 3}, "area_m2": 50,
                  "hose_streams": [{"flow_lpm": 100, "duration_min": 20}],
                  "proportioner": {"min_flow_lpm": 100, "max_flow_lpm": 600, "min_inlet_pressure_bar": 7.0}, "tank": {"capacity_l": 300}}],
        "gas": [{"id": "G1", "name": "Sala de servidores", "family": "clean_agent", "agent": "demo_halocarbon", "hazard_class": "demo_high", "occupied": False,
                 "enclosure": {"volume_m3": 144, "min_temperature_c": 18, "altitude_m": 100}, "design_concentration_pct": 9.5, "discharge_time_s": 10, "hold_time_min": 12,
                 "cylinder": {"max_fill_kg": 60, "min_fill_kg": 20}}],
        "dry_chemical": [{"id": "DC1", "name": "Cabina de pintura (ejemplo)", "mode": "total_flooding", "agent": "demo_powder", "hazard_class": "demo_medium", "occupied": False,
                          "discharge_time_s": 25, "enclosure": {"volume_m3": 120, "open_area_m2": 2}, "nozzles": {"count": 4, "flow_kg_s": 1.5}, "cylinder": {"capacity_kg": 45}}],
        "wet_chemical": [{"id": "K1", "name": "Cocina (ejemplo)", "listing_ref": "Listado de demostración del fabricante (ficticio)", "tank": {"max_flow_numbers": 12},
                          "appliances": [{"id": "A1", "type": "freidora", "area_m2": 0.9, "nozzle": {"coverage_area_m2": 0.5, "flow_number": 1}, "nozzle_count": 2},
                                         {"id": "A2", "type": "plancha", "area_m2": 0.6, "nozzle": {"coverage_area_m2": 0.6, "flow_number": 2}, "nozzle_count": 1}],
                          "plenum": {"required_nozzles": 1, "nozzle_count": 1, "flow_number_each": 2}, "duct": {"required_nozzles": 1, "nozzle_count": 1, "flow_number_each": 2},
                          "fuel_shutoff": {"provided": True}}],
        "aerosol": [{"id": "AE1", "name": "Archivo (ejemplo)", "hazard_class": "demo_high", "occupied": False, "actual_clearance_m": 1.5, "enclosure": {"volume_m3": 90},
                     "product": {"design_density_g_m3": 100, "unit_mass_g": 2500, "max_volume_per_unit_m3": 40, "min_clearance_m": 1.0, "listing_ref": "Listado de demostración (ficticio)"}}],
    },
    "cause_effect": {
        "inputs": [{"id": "DET1", "name": "Detector zona A", "kind": "detector"}, {"id": "DET2", "name": "Detector zona B", "kind": "detector"},
                   {"id": "MAN1", "name": "Pulsador manual", "kind": "manual_station"}],
        "outputs": [{"id": "HORN", "name": "Sirena de predescarga", "kind": "alarm_horn", "delay_s": 0},
                    {"id": "REL1", "name": "Descarga G1", "kind": "agent_release", "delay_s": 30, "target_system": "G1"},
                    {"id": "HVAC", "name": "Paro de climatización", "kind": "hvac_stop", "delay_s": 0}],
        "rules": [{"id": "R1", "logic": "cross_zone", "inputs": ["DET1", "DET2"], "outputs": ["HORN", "REL1", "HVAC"], "delay_s": 0},
                  {"id": "R2", "logic": "any", "inputs": ["MAN1"], "outputs": ["HORN", "REL1"], "delay_s": 0}],
    },
    "catalog": {"items": [
        {"id": "DEMO-SPR-80", "category": "dispositivo", "description": "Rociador de demostración K=80", "manufacturer": "", "model": "", "unit": "u", "price": "12.50", "currency": "USD", "source": "Ejemplo ficticio", "status": "demo", "notes": ""},
        {"id": "DEMO-TUB-65", "category": "tuberia", "description": "Tubería de demostración 65 mm", "manufacturer": "", "model": "", "unit": "m", "price": "8.20", "currency": "USD", "source": "Ejemplo ficticio", "status": "demo", "notes": ""},
        {"id": "DEMO-EXT-ABC", "category": "extintor", "description": "Extintor ABC de demostración", "manufacturer": "", "model": "", "unit": "u", "price": None, "currency": None, "source": "", "status": "demo", "notes": "sin precio"}]},
    "budget": {"currency": "USD", "decimals": 2, "lines": [
        {"id": "B1", "bom_key": "device|sprinkler|80", "catalog_ref": "DEMO-SPR-80", "description": "Rociadores K=80", "qty": 4, "unit": "u", "category": "dispositivo"},
        {"id": "B2", "bom_key": "pipe|65", "catalog_ref": "DEMO-TUB-65", "description": "Tubería 65 mm", "qty": 16.5, "unit": "m", "category": "tuberia"},
        {"id": "B3", "bom_key": "extinguisher|powder_abc|{\"A\":3,\"B\":3,\"C\":3}", "catalog_ref": "DEMO-EXT-ABC", "description": "Extintores ABC", "qty": 2, "unit": "u", "category": "extintor"}],
        "taxes": [{"name": "Impuesto de ejemplo", "rate_pct": 10}], "notes": "Precios de ejemplo ficticios"},
    "results": {}, "exceptions": [], "reviews": [], "inspections": [],
}
sys.stdout.write(json.dumps(proj, ensure_ascii=False, indent=2) + "\n")
