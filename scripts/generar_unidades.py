#!/usr/bin/env python3
"""Genera shared/units.json con factores de conversión EXACTOS (aritmética decimal).

Solo usa definiciones legales/internacionales (no valores normativos de protección
contra incendios):
  1 in = 0.0254 m (exacto)            1 ft = 0.3048 m (exacto)
  1 lb = 0.45359237 kg (exacto)       1 gal US = 3.785411784 L (exacto)
  1 lbf = 4.4482216152605 N (exacto)  1 atm = 101325 Pa (exacto)
  g_n = 9.80665 m/s2 (exacto, CGPM)

Uso: python scripts/generar_unidades.py > shared/units.json
La tabla es la ÚNICA fuente de factores: la leen el motor Rust (include_str!) y la
interfaz JavaScript, de modo que ambos convierten exactamente igual.
"""
import json
from decimal import Decimal as D, getcontext

getcontext().prec = 40

IN = D("0.0254"); FT = D("0.3048"); LB = D("0.45359237")
GAL = D("0.003785411784")           # m3
LBF = D("4.4482216152605")          # N
PSI_PA = LBF / (IN * IN)            # Pa
PSI_BAR = PSI_PA / D(100000)        # bar
FT2 = FT * FT; FT3 = FT * FT * FT; IN2 = IN * IN; IN3 = IN * IN * IN


def f(x):
    """Decimal -> float con redondeo correcto (el `float` de Python es el más cercano)."""
    return float(x)


quantities = {
    "length": {"canonical": "m", "units": {
        "m": 1.0, "mm": 0.001, "cm": 0.01, "km": 1000.0, "in": f(IN), "ft": f(FT), "yd": f(FT * 3)}},
    "area": {"canonical": "m2", "units": {
        "m2": 1.0, "mm2": 1e-6, "cm2": 1e-4, "ft2": f(FT2), "in2": f(IN2)}},
    "volume": {"canonical": "m3", "units": {
        "m3": 1.0, "L": 0.001, "mL": 1e-6, "gal_us": f(GAL), "ft3": f(FT3), "in3": f(IN3)}},
    "flow": {"canonical": "L/min", "units": {
        "L/min": 1.0, "L/s": 60.0, "m3/h": f(D(1000) / D(60)), "m3/s": 60000.0, "gpm": f(GAL * 1000)}},
    "pressure": {"canonical": "bar", "units": {
        "bar": 1.0, "mbar": 0.001, "kPa": 0.01, "MPa": 10.0, "Pa": 1e-5, "psi": f(PSI_BAR), "atm": 1.01325}},
    "mass": {"canonical": "kg", "units": {"kg": 1.0, "g": 0.001, "lb": f(LB)}},
    "velocity": {"canonical": "m/s", "units": {"m/s": 1.0, "ft/s": f(FT), "km/h": f(D(1) / D("3.6"))}},
    "time": {"canonical": "s", "units": {"s": 1.0, "min": 60.0, "h": 3600.0}},
    "density": {"canonical": "kg/m3", "units": {"kg/m3": 1.0, "lb/ft3": f(LB / FT3)}},
    "specific_volume": {"canonical": "m3/kg", "units": {"m3/kg": 1.0, "ft3/lb": f(FT3 / LB)}},
    "mass_flow": {"canonical": "kg/s", "units": {"kg/s": 1.0, "lb/s": f(LB)}},
    # Densidad de aplicación por área: 1 L/min/m2 == 1 mm/min
    "application_rate": {"canonical": "L/min/m2", "units": {
        "L/min/m2": 1.0, "mm/min": 1.0, "gpm/ft2": f(GAL * 1000 / FT2)}},
    # Factor K de descarga: Q = K * sqrt(P)
    "k_factor": {"canonical": "L/min/bar^0.5", "units": {
        "L/min/bar^0.5": 1.0, "gpm/psi^0.5": f(GAL * 1000 / PSI_BAR.sqrt())}},
    "concentration": {"canonical": "%", "units": {"%": 1.0, "fraction": 100.0}},
    "angle": {"canonical": "deg", "units": {"deg": 1.0, "rad": f(D(180) / D("3.14159265358979323846264338327950288"))}},
    "dimensionless": {"canonical": "-", "units": {"-": 1.0}},
    # Temperatura: afín  canónica = a*valor + b
    "temperature": {"canonical": "C", "affine": {
        "C": [1.0, 0.0], "K": [1.0, -273.15], "F": [f(D(5) / D(9)), f(-D(160) / D(9))]}},
}

# Unidad de presentación por sistema de unidades (rol -> [magnitud, unidad]).
display = {
    "SI": {
        "length": ["length", "m"], "length_small": ["length", "mm"], "area": ["area", "m2"],
        "volume": ["volume", "m3"], "volume_small": ["volume", "L"], "flow": ["flow", "L/min"],
        "pressure": ["pressure", "bar"], "mass": ["mass", "kg"], "temperature": ["temperature", "C"],
        "velocity": ["velocity", "m/s"], "time": ["time", "min"], "density": ["density", "kg/m3"],
        "specific_volume": ["specific_volume", "m3/kg"], "mass_flow": ["mass_flow", "kg/s"],
        "application_rate": ["application_rate", "L/min/m2"], "k_factor": ["k_factor", "L/min/bar^0.5"],
        "concentration": ["concentration", "%"], "angle": ["angle", "deg"], "dimensionless": ["dimensionless", "-"],
    },
    "US": {
        "length": ["length", "ft"], "length_small": ["length", "in"], "area": ["area", "ft2"],
        "volume": ["volume", "ft3"], "volume_small": ["volume", "gal_us"], "flow": ["flow", "gpm"],
        "pressure": ["pressure", "psi"], "mass": ["mass", "lb"], "temperature": ["temperature", "F"],
        "velocity": ["velocity", "ft/s"], "time": ["time", "min"], "density": ["density", "lb/ft3"],
        "specific_volume": ["specific_volume", "ft3/lb"], "mass_flow": ["mass_flow", "lb/s"],
        "application_rate": ["application_rate", "gpm/ft2"], "k_factor": ["k_factor", "gpm/psi^0.5"],
        "concentration": ["concentration", "%"], "angle": ["angle", "deg"], "dimensionless": ["dimensionless", "-"],
    },
}

doc = {
    "schema": "vigix.units/1",
    "nota": "Generado por scripts/generar_unidades.py. Factor = cuántas unidades canónicas hay en 1 unidad. "
            "Solo definiciones exactas; ningún valor normativo de protección contra incendios.",
    "gravedad_estandar_m_s2": 9.80665,
    "quantities": quantities,
    "display": display,
}
print(json.dumps(doc, ensure_ascii=False, indent=2, sort_keys=True))
