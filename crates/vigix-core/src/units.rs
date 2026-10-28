//! Conversión de unidades con almacenamiento canónico (contrato 5.1).
//!
//! La tabla de factores vive en `shared/units.json` y es la **misma** que lee la
//! interfaz JavaScript. Una unidad incompatible es un error explícito, nunca un cero.

use crate::json::Json;
use std::collections::BTreeMap;
use std::sync::OnceLock;

const UNITS_JSON: &str = include_str!("../../../shared/units.json");

#[derive(Clone, Debug, PartialEq)]
pub enum UnitError {
    /// La unidad no existe en la tabla.
    Unknown(String),
    /// Las unidades pertenecen a magnitudes distintas.
    Incompatible { from: String, to: String },
    /// El valor no es finito.
    NotFinite,
}

impl std::fmt::Display for UnitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnitError::Unknown(u) => write!(f, "unidad desconocida '{}'", u),
            UnitError::Incompatible { from, to } => {
                write!(f, "unidades incompatibles '{}' -> '{}'", from, to)
            }
            UnitError::NotFinite => write!(f, "valor no finito"),
        }
    }
}

impl std::error::Error for UnitError {}

#[derive(Clone, Debug)]
struct UnitDef {
    quantity: String,
    /// canónico = a * valor + b
    a: f64,
    b: f64,
}

pub struct UnitTable {
    units: BTreeMap<String, UnitDef>,
    canonical: BTreeMap<String, String>,
    display: BTreeMap<String, BTreeMap<String, (String, String)>>,
    pub standard_gravity: f64,
}

/// Tabla global (se analiza una sola vez).
pub fn table() -> &'static UnitTable {
    static T: OnceLock<UnitTable> = OnceLock::new();
    T.get_or_init(|| {
        let doc = Json::parse(UNITS_JSON).expect("shared/units.json es JSON válido (verificado por pruebas)");
        UnitTable::from_json(&doc).expect("shared/units.json tiene el esquema esperado (verificado por pruebas)")
    })
}

impl UnitTable {
    pub fn from_json(doc: &Json) -> Result<UnitTable, String> {
        let mut units = BTreeMap::new();
        let mut canonical = BTreeMap::new();
        let qs = doc.field("quantities").as_obj().ok_or("falta 'quantities'")?;
        for (qname, q) in qs {
            let canon = q.field("canonical").as_str().ok_or("falta 'canonical'")?.to_string();
            canonical.insert(qname.clone(), canon);
            if let Some(us) = q.field("units").as_obj() {
                for (u, f) in us {
                    let a = f.as_f64().ok_or_else(|| format!("factor inválido de {}", u))?;
                    if a <= 0.0 {
                        return Err(format!("factor no positivo en {}", u));
                    }
                    if units.insert(u.clone(), UnitDef { quantity: qname.clone(), a, b: 0.0 }).is_some() {
                        return Err(format!("unidad duplicada: {}", u));
                    }
                }
            }
            if let Some(us) = q.field("affine").as_obj() {
                for (u, ab) in us {
                    let a = ab.idx(0).and_then(Json::as_f64).ok_or("afín inválido")?;
                    let b = ab.idx(1).and_then(Json::as_f64).ok_or("afín inválido")?;
                    if a <= 0.0 {
                        return Err(format!("pendiente no positiva en {}", u));
                    }
                    if units.insert(u.clone(), UnitDef { quantity: qname.clone(), a, b }).is_some() {
                        return Err(format!("unidad duplicada: {}", u));
                    }
                }
            }
        }
        let mut display = BTreeMap::new();
        if let Some(ds) = doc.field("display").as_obj() {
            for (sys, roles) in ds {
                let mut m = BTreeMap::new();
                if let Some(rs) = roles.as_obj() {
                    for (role, qu) in rs {
                        let q = qu.idx(0).and_then(Json::as_str).ok_or("rol de presentación inválido")?;
                        let u = qu.idx(1).and_then(Json::as_str).ok_or("rol de presentación inválido")?;
                        if !units.contains_key(u) {
                            return Err(format!("unidad de presentación desconocida: {}", u));
                        }
                        m.insert(role.clone(), (q.to_string(), u.to_string()));
                    }
                }
                display.insert(sys.clone(), m);
            }
        }
        let g = doc.field("gravedad_estandar_m_s2").as_f64().ok_or("falta la gravedad estándar")?;
        Ok(UnitTable { units, canonical, display, standard_gravity: g })
    }

    pub fn quantity_of(&self, unit: &str) -> Option<&str> {
        self.units.get(unit).map(|d| d.quantity.as_str())
    }

    pub fn canonical_of(&self, quantity: &str) -> Option<&str> {
        self.canonical.get(quantity).map(String::as_str)
    }

    pub fn is_unit(&self, unit: &str) -> bool {
        self.units.contains_key(unit)
    }

    /// Unidad de presentación `(magnitud, unidad)` para un sistema (`SI`/`US`) y un rol.
    pub fn display_unit(&self, system: &str, role: &str) -> Option<(&str, &str)> {
        self.display.get(system)?.get(role).map(|(q, u)| (q.as_str(), u.as_str()))
    }

    pub fn convert(&self, value: f64, from: &str, to: &str) -> Result<f64, UnitError> {
        if !value.is_finite() {
            return Err(UnitError::NotFinite);
        }
        let f = self.units.get(from).ok_or_else(|| UnitError::Unknown(from.to_string()))?;
        let t = self.units.get(to).ok_or_else(|| UnitError::Unknown(to.to_string()))?;
        if f.quantity != t.quantity {
            return Err(UnitError::Incompatible { from: from.to_string(), to: to.to_string() });
        }
        if from == to {
            return Ok(value);
        }
        let canon = f.a * value + f.b;
        Ok((canon - t.b) / t.a)
    }

    /// Convierte a la unidad canónica de su magnitud. Devuelve `(valor, unidad canónica)`.
    pub fn to_canonical(&self, value: f64, unit: &str) -> Result<(f64, &str), UnitError> {
        let f = self.units.get(unit).ok_or_else(|| UnitError::Unknown(unit.to_string()))?;
        if !value.is_finite() {
            return Err(UnitError::NotFinite);
        }
        let canon_unit = self.canonical.get(&f.quantity).map(String::as_str).unwrap_or("");
        Ok((f.a * value + f.b, canon_unit))
    }
}

/// Atajo: convierte con la tabla global.
pub fn convert(value: f64, from: &str, to: &str) -> Result<f64, UnitError> {
    table().convert(value, from, to)
}

/// Redondeo "mitad lejos de cero" a `decimals` decimales (determinista; contrato 11.2).
///
/// Se aplica sobre la representación decimal más corta del número (la misma que se escribe
/// en JSON), de modo que 2.675 -> 2.68 como espera una persona, y un resultado calculado
/// 2.6749999999999998 -> 2.67 (está realmente por debajo del empate).
pub fn round_half_away(x: f64, decimals: u32) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    let s = format!("{}", x.abs());
    let (int_part, frac_part) = match s.split_once('.') {
        Some((i, f)) => (i, f),
        None => (s.as_str(), ""),
    };
    let d = decimals as usize;
    if frac_part.len() <= d {
        return x;
    }
    let next = frac_part.as_bytes()[d];
    let mut digits: Vec<u8> = int_part.bytes().chain(frac_part.bytes().take(d)).collect();
    if next >= b'5' {
        let mut i = digits.len();
        loop {
            if i == 0 {
                digits.insert(0, b'1');
                break;
            }
            i -= 1;
            if digits[i] == b'9' {
                digits[i] = b'0';
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    let split = digits.len() - d;
    let mut out = String::from_utf8_lossy(&digits[..split]).into_owned();
    if d > 0 {
        out.push('.');
        out.push_str(&String::from_utf8_lossy(&digits[split..]));
    }
    let v: f64 = out.parse().unwrap_or(x.abs());
    v.copysign(x)
}

/// Redondea hacia arriba al múltiplo `step` (cantidades de agente/concentrado; el paso lo fija el perfil).
pub fn ceil_to_step(x: f64, step: f64) -> Option<f64> {
    if !(x.is_finite() && step.is_finite()) || step <= 0.0 {
        return None;
    }
    let n = (x / step - 1e-12).ceil();
    Some(n * step)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, rel: f64) -> bool {
        (a - b).abs() <= rel * a.abs().max(b.abs()).max(1e-300)
    }

    #[test]
    fn tabla_valida_y_sin_duplicados() {
        let t = table();
        assert!(t.is_unit("bar") && t.is_unit("psi") && t.is_unit("gpm") && t.is_unit("F"));
        assert!((t.standard_gravity - 9.80665).abs() < 1e-12);
    }

    #[test]
    fn conversiones_exactas_conocidas() {
        assert!(close(convert(1.0, "in", "mm").unwrap(), 25.4, 1e-15));
        assert!(close(convert(1.0, "ft", "m").unwrap(), 0.3048, 1e-15));
        assert!(close(convert(1.0, "psi", "kPa").unwrap(), 6.894757293168361, 1e-12));
        assert!(close(convert(100.0, "psi", "bar").unwrap(), 6.894757293168361, 1e-12));
        assert!(close(convert(1.0, "gpm", "L/min").unwrap(), 3.785411784, 1e-15));
        assert!(close(convert(1.0, "lb", "kg").unwrap(), 0.45359237, 1e-15));
        assert!(close(convert(1.0, "gal_us", "L").unwrap(), 3.785411784, 1e-14));
        assert!(close(convert(1.0, "atm", "kPa").unwrap(), 101.325, 1e-14));
        assert!(close(convert(1.0, "gpm/ft2", "mm/min").unwrap(), 40.74583333333333, 1e-12));
        assert!(close(convert(1.0, "gpm/psi^0.5", "L/min/bar^0.5").unwrap(), 14.416294257371696, 1e-12));
    }

    #[test]
    fn k_factor_es_consistente_con_q_igual_k_raiz_p() {
        // Q = K sqrt(P) debe dar el mismo caudal en ambos sistemas de unidades.
        let k_us = 5.6; // gpm/psi^0.5
        let p_psi = 50.0;
        let q_us = k_us * f64::sqrt(p_psi); // gpm
        let k_si = convert(k_us, "gpm/psi^0.5", "L/min/bar^0.5").unwrap();
        let p_bar = convert(p_psi, "psi", "bar").unwrap();
        let q_si = k_si * f64::sqrt(p_bar); // L/min
        assert!(close(convert(q_us, "gpm", "L/min").unwrap(), q_si, 1e-12));
    }

    #[test]
    fn temperatura_afin() {
        assert!(close(convert(100.0, "C", "F").unwrap(), 212.0, 1e-12));
        assert!(close(convert(32.0, "F", "C").unwrap(), 0.0, 1e-9) || convert(32.0, "F", "C").unwrap().abs() < 1e-12);
        assert!(close(convert(0.0, "C", "K").unwrap(), 273.15, 1e-12));
        assert!(close(convert(-40.0, "F", "C").unwrap(), -40.0, 1e-12));
        let x = 23.456;
        let back = convert(convert(x, "C", "F").unwrap(), "F", "C").unwrap();
        assert!(close(back, x, 1e-12));
    }

    #[test]
    fn ida_y_vuelta_de_todas_las_unidades() {
        let t = table();
        let names: Vec<String> = t.units.keys().cloned().collect();
        for u in &names {
            let q = t.quantity_of(u).unwrap().to_string();
            let canon = t.canonical_of(&q).unwrap().to_string();
            let v = 12.3456789;
            let c = convert(v, u, &canon).unwrap();
            let back = convert(c, &canon, u).unwrap();
            assert!(close(back, v, 1e-12), "{} -> {} -> {}", u, canon, back);
        }
    }

    #[test]
    fn unidad_incompatible_o_desconocida() {
        assert_eq!(convert(1.0, "psi", "m"), Err(UnitError::Incompatible { from: "psi".into(), to: "m".into() }));
        assert!(matches!(convert(1.0, "furlong", "m"), Err(UnitError::Unknown(_))));
        assert_eq!(convert(f64::NAN, "m", "ft"), Err(UnitError::NotFinite));
        assert_eq!(convert(f64::INFINITY, "m", "ft"), Err(UnitError::NotFinite));
    }

    #[test]
    fn presentacion_por_sistema() {
        let t = table();
        assert_eq!(t.display_unit("SI", "pressure"), Some(("pressure", "bar")));
        assert_eq!(t.display_unit("US", "pressure"), Some(("pressure", "psi")));
        assert_eq!(t.display_unit("SI", "length_small"), Some(("length", "mm")));
        assert_eq!(t.display_unit("US", "length_small"), Some(("length", "in")));
        assert_eq!(t.display_unit("XX", "pressure"), None);
    }

    #[test]
    fn redondeo_mitad_lejos_de_cero() {
        assert_eq!(round_half_away(2.675, 2), 2.68);
        assert_eq!(round_half_away(-2.675, 2), -2.68);
        assert_eq!(round_half_away(2.5, 0), 3.0);
        assert_eq!(round_half_away(-2.5, 0), -3.0);
        assert_eq!(round_half_away(1.005, 2), 1.01);
        assert_eq!(round_half_away(0.0, 3), 0.0);
        assert_eq!(round_half_away(1234.5678, 2), 1234.57);
        assert_eq!(round_half_away(1234.5649, 2), 1234.56);
    }

    #[test]
    fn redondeo_hacia_arriba_a_paso() {
        assert_eq!(ceil_to_step(12.1, 0.5), Some(12.5));
        assert_eq!(ceil_to_step(12.5, 0.5), Some(12.5));
        assert_eq!(ceil_to_step(0.0, 0.5), Some(0.0));
        assert_eq!(ceil_to_step(3.0, 0.0), None);
        assert_eq!(ceil_to_step(f64::NAN, 1.0), None);
    }
}
