//! Registro de auditoría local append-only con **cadena de huellas** (contrato 4.3).
//!
//! Cada entrada incluye la huella de la anterior: `hash = SHA256(prev_hash | JSON canónico de la
//! entrada sin `hash`)`. Alterar, borrar o reordenar una entrada rompe la cadena y `verify_chain`
//! señala la primera entrada inválida. El reloj del equipo no es una fuente certificada de
//! tiempo: la fecha es informativa y así se declara.

use crate::hash::sha256_hex;
use crate::json::Json;

pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub seq: u64,
    /// Fecha/hora del equipo (UTC, ISO 8601); no certificada.
    pub ts: String,
    pub user: String,
    pub role: String,
    pub app_version: String,
    /// Perfil activo `codigo@version` (vacío si no hay).
    pub profile: String,
    pub action: String,
    pub project: String,
    pub detail: Json,
    pub prev_hash: String,
    pub hash: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChainError {
    /// La secuencia no es consecutiva en `seq` (entrada borrada o duplicada).
    BadSequence { seq: u64 },
    /// `prev_hash` no coincide con la huella de la entrada anterior.
    BrokenLink { seq: u64 },
    /// La huella no corresponde al contenido (entrada alterada).
    BadHash { seq: u64 },
}

impl Entry {
    fn body_json(&self) -> Json {
        Json::obj()
            .with("seq", Json::num(self.seq as f64))
            .with("ts", Json::str(self.ts.as_str()))
            .with("user", Json::str(self.user.as_str()))
            .with("role", Json::str(self.role.as_str()))
            .with("app_version", Json::str(self.app_version.as_str()))
            .with("profile", Json::str(self.profile.as_str()))
            .with("action", Json::str(self.action.as_str()))
            .with("project", Json::str(self.project.as_str()))
            .with("detail", self.detail.clone())
            .with("prev_hash", Json::str(self.prev_hash.as_str()))
    }

    pub fn compute_hash(&self) -> String {
        sha256_hex(self.body_json().canonical().as_bytes())
    }

    pub fn to_json(&self) -> Json {
        self.body_json().with("hash", Json::str(self.hash.as_str()))
    }

    pub fn from_json(j: &Json) -> Option<Entry> {
        Some(Entry {
            seq: j.field("seq").as_f64().filter(|v| *v >= 0.0 && v.fract() == 0.0)? as u64,
            ts: j.field("ts").as_str()?.to_string(),
            user: j.field("user").as_str()?.to_string(),
            role: j.field("role").as_str()?.to_string(),
            app_version: j.field("app_version").as_str()?.to_string(),
            profile: j.field("profile").as_str().unwrap_or("").to_string(),
            action: j.field("action").as_str()?.to_string(),
            project: j.field("project").as_str().unwrap_or("").to_string(),
            detail: j.field("detail").clone(),
            prev_hash: j.field("prev_hash").as_str()?.to_string(),
            hash: j.field("hash").as_str()?.to_string(),
        })
    }
}

/// Crea la siguiente entrada encadenada a `prev` (o a la génesis si no hay anterior).
#[allow(clippy::too_many_arguments)]
pub fn next_entry(
    prev: Option<&Entry>,
    ts: &str,
    user: &str,
    role: &str,
    app_version: &str,
    profile: &str,
    action: &str,
    project: &str,
    detail: Json,
) -> Entry {
    let (seq, prev_hash) = match prev {
        Some(p) => (p.seq + 1, p.hash.clone()),
        None => (1, GENESIS.to_string()),
    };
    let mut e = Entry {
        seq,
        ts: ts.to_string(),
        user: user.to_string(),
        role: role.to_string(),
        app_version: app_version.to_string(),
        profile: profile.to_string(),
        action: action.to_string(),
        project: project.to_string(),
        detail,
        prev_hash,
        hash: String::new(),
    };
    e.hash = e.compute_hash();
    e
}

/// Verifica una cadena completa (desde la entrada 1) o un tramo contiguo.
pub fn verify_chain(entries: &[Entry]) -> Result<(), ChainError> {
    let mut prev: Option<&Entry> = None;
    for e in entries {
        if let Some(p) = prev {
            if e.seq != p.seq + 1 {
                return Err(ChainError::BadSequence { seq: e.seq });
            }
            if e.prev_hash != p.hash {
                return Err(ChainError::BrokenLink { seq: e.seq });
            }
        } else if e.seq == 1 && e.prev_hash != GENESIS {
            return Err(ChainError::BrokenLink { seq: e.seq });
        }
        if e.compute_hash() != e.hash {
            return Err(ChainError::BadHash { seq: e.seq });
        }
        prev = Some(e);
    }
    Ok(())
}

/// Diferencias a nivel de ruta entre dos documentos (para registrar qué cambió sin copiar todo).
/// Los valores largos se sustituyen por su huella. Devuelve como máximo `limit` cambios.
pub fn diff(old: &Json, new: &Json, limit: usize) -> Vec<Json> {
    let mut out = Vec::new();
    diff_rec(old, new, "", limit, &mut out);
    out
}

fn short(v: &Json) -> Json {
    let c = v.canonical();
    if c.len() > 120 {
        Json::str(format!("sha256:{}", &sha256_hex(c.as_bytes())[..16]))
    } else {
        v.clone()
    }
}

fn diff_rec(a: &Json, b: &Json, path: &str, limit: usize, out: &mut Vec<Json>) {
    if out.len() >= limit || a == b {
        return;
    }
    match (a, b) {
        (Json::Obj(ma), Json::Obj(mb)) => {
            let mut keys: Vec<&String> = ma.keys().chain(mb.keys()).collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                let p = if path.is_empty() { k.clone() } else { format!("{}.{}", path, k) };
                match (ma.get(k), mb.get(k)) {
                    (Some(x), Some(y)) => diff_rec(x, y, &p, limit, out),
                    (Some(x), None) => push(out, limit, &p, short(x), Json::Null),
                    (None, Some(y)) => push(out, limit, &p, Json::Null, short(y)),
                    (None, None) => {}
                }
            }
        }
        (Json::Arr(xa), Json::Arr(xb)) if xa.len() == xb.len() => {
            for (i, (x, y)) in xa.iter().zip(xb.iter()).enumerate() {
                diff_rec(x, y, &format!("{}.{}", path, i), limit, out);
            }
        }
        _ => push(out, limit, path, short(a), short(b)),
    }
}

fn push(out: &mut Vec<Json>, limit: usize, path: &str, old: Json, new: Json) {
    if out.len() < limit {
        out.push(Json::obj().with("path", Json::str(path)).with("old", old).with("new", new));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(n: usize) -> Vec<Entry> {
        let mut v: Vec<Entry> = Vec::new();
        for i in 0..n {
            let e = next_entry(
                v.last(),
                "2026-10-28T00:00:00Z",
                "ana",
                "designer",
                "0.1.0",
                "DEMO@0.1.0",
                "input.changed",
                "P1",
                Json::obj().with("i", Json::num(i as f64)),
            );
            v.push(e);
        }
        v
    }

    #[test]
    fn cadena_valida() {
        let c = chain(5);
        assert_eq!(c[0].prev_hash, GENESIS);
        assert_eq!(c[1].prev_hash, c[0].hash);
        assert_eq!(verify_chain(&c), Ok(()));
        // un tramo contiguo también se verifica
        assert_eq!(verify_chain(&c[2..4]), Ok(()));
        assert_eq!(verify_chain(&[]), Ok(()));
    }

    #[test]
    fn detecta_alteracion_borrado_y_reordenamiento() {
        let mut c = chain(5);
        c[2].detail = Json::obj().with("i", Json::num(99.0));
        assert_eq!(verify_chain(&c), Err(ChainError::BadHash { seq: 3 }));
        // alterar y recalcular la huella de una entrada rompe el enlace de la siguiente
        let mut c = chain(5);
        c[2].detail = Json::obj().with("i", Json::num(99.0));
        c[2].hash = c[2].compute_hash();
        assert_eq!(verify_chain(&c), Err(ChainError::BrokenLink { seq: 4 }));
        // borrado de una entrada intermedia
        let mut c = chain(5);
        c.remove(2);
        assert_eq!(verify_chain(&c), Err(ChainError::BadSequence { seq: 4 }));
        // reordenamiento
        let mut c = chain(5);
        c.swap(1, 2);
        assert!(verify_chain(&c).is_err());
        // cambiar el usuario también se detecta
        let mut c = chain(3);
        c[1].user = "otro".to_string();
        assert!(matches!(verify_chain(&c), Err(ChainError::BadHash { seq: 2 })));
    }

    #[test]
    fn json_ida_y_vuelta_conserva_la_huella() {
        let c = chain(3);
        let back: Vec<Entry> = c.iter().map(|e| Entry::from_json(&Json::parse(&e.to_json().to_compact()).unwrap()).unwrap()).collect();
        assert_eq!(c, back);
        assert_eq!(verify_chain(&back), Ok(()));
    }

    #[test]
    fn diff_por_ruta_con_huella_de_valores_largos() {
        let a = Json::parse(r#"{"a":1,"b":{"c":[1,2,3],"d":"x"},"e":null}"#).unwrap();
        let b = Json::parse(r#"{"a":2,"b":{"c":[1,9,3],"d":"x"},"f":true}"#).unwrap();
        let d = diff(&a, &b, 50);
        let paths: Vec<&str> = d.iter().map(|x| x.field("path").as_str().unwrap()).collect();
        assert_eq!(paths, vec!["a", "b.c.1", "e", "f"]);
        assert_eq!(d[0].field("old").as_f64(), Some(1.0));
        assert_eq!(d[0].field("new").as_f64(), Some(2.0));
        // límite
        assert_eq!(diff(&a, &b, 2).len(), 2);
        // valor largo => huella
        let long = Json::str("x".repeat(500));
        let d = diff(&Json::obj().with("k", long.clone()), &Json::obj().with("k", Json::str("y")), 5);
        assert!(d[0].field("old").as_str().unwrap().starts_with("sha256:"));
        assert_eq!(diff(&a, &a, 5).len(), 0);
    }
}
