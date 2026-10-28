//! Fechas y reloj sin dependencias.
//!
//! El reloj del equipo **solo** se usa para sellos de auditoría y de cálculo (informativos).
//! Ninguna decisión de seguridad ni de licencia depende de él (contrato 8.3), y se informa
//! que no es una fuente certificada de tiempo (contrato 4.2). La vigencia de un perfil se
//! evalúa contra la *fecha de diseño del proyecto* (un dato de entrada), no contra el reloj.

use std::time::{SystemTime, UNIX_EPOCH};

/// Fuente de tiempo inyectable (para pruebas deterministas).
pub trait Clock: Send + Sync {
    /// Segundos Unix (UTC).
    fn now_unix(&self) -> i64;
}

pub struct SystemClock;
impl Clock for SystemClock {
    fn now_unix(&self) -> i64 {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_secs() as i64,
            Err(e) => -(e.duration().as_secs() as i64),
        }
    }
}

/// Reloj fijo para pruebas.
pub struct FixedClock(pub i64);
impl Clock for FixedClock {
    fn now_unix(&self) -> i64 {
        self.0
    }
}

/// Días desde 1970-01-01 (algoritmo de H. Hinnant, calendario gregoriano proléptico).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe as i64 - 719468
}

pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

pub fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Analiza `YYYY-MM-DD` estricto. Devuelve los días desde 1970-01-01.
pub fn parse_date(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        let t = &s[r];
        if t.bytes().all(|c| c.is_ascii_digit()) {
            t.parse().ok()
        } else {
            None
        }
    };
    let y = num(0..4)? as i64;
    let m = num(5..7)?;
    let d = num(8..10)?;
    if !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

pub fn format_date(days: i64) -> String {
    let (y, m, d) = civil_from_days(days);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

/// `YYYY-MM-DDThh:mm:ssZ`.
pub fn iso8601_utc(unix: i64) -> String {
    let days = unix.div_euclid(86400);
    let rem = unix.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m,
        d,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

pub fn parse_iso8601_utc(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 20 || b[10] != b'T' || b[13] != b':' || b[16] != b':' || b[19] != b'Z' {
        return None;
    }
    let days = parse_date(&s[0..10])?;
    let n = |r: std::ops::Range<usize>| -> Option<i64> { s[r].parse::<i64>().ok() };
    let (h, mi, se) = (n(11..13)?, n(14..16)?, n(17..19)?);
    if !(0..24).contains(&h) || !(0..60).contains(&mi) || !(0..60).contains(&se) {
        return None;
    }
    Some(days * 86400 + h * 3600 + mi * 60 + se)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_y_fechas_conocidas() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
        // 2026-10-28T00:00:00Z (fecha de la especificación) = 1793145600
        assert_eq!(iso8601_utc(1_793_145_600), "2026-10-28T00:00:00Z");
        assert_eq!(
            parse_iso8601_utc("2026-10-28T00:00:00Z"),
            Some(1_793_145_600)
        );
        assert_eq!(iso8601_utc(951_782_400), "2000-02-29T00:00:00Z"); // bisiesto
    }

    #[test]
    fn ida_y_vuelta_de_muchos_dias() {
        // Aritmética civil: rango amplio (incluye años negativos).
        for d in (-800_000..800_000).step_by(997) {
            let (y, m, dd) = civil_from_days(d);
            assert_eq!(days_from_civil(y, m, dd), d);
        }
        // Texto AAAA-MM-DD: solo años 0001..9999.
        for d in (-719_000..2_900_000).step_by(991) {
            assert_eq!(parse_date(&format_date(d)), Some(d));
        }
    }

    #[test]
    fn fechas_invalidas() {
        for bad in [
            "2026-13-01",
            "2026-02-29",
            "2026-00-10",
            "2026-1-10",
            "26-10-10",
            "2026/10/10",
            "2026-10-1x",
            "",
            "2026-04-31",
        ] {
            assert!(parse_date(bad).is_none(), "{}", bad);
        }
        assert!(parse_date("2028-02-29").is_some());
        assert!(parse_iso8601_utc("2026-10-28T24:00:00Z").is_none());
        assert!(parse_iso8601_utc("2026-10-28 00:00:00Z").is_none());
    }

    #[test]
    fn reloj_fijo() {
        let c = FixedClock(1_793_145_600);
        assert_eq!(iso8601_utc(c.now_unix()), "2026-10-28T00:00:00Z");
        assert!(SystemClock.now_unix() > 1_700_000_000);
    }
}
