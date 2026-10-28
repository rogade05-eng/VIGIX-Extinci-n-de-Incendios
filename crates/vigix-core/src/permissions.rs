//! Roles y permisos (contrato 4.3).
//!
//! Roles: administrador, proyectista, revisor especialista, instalador/inspector y lectura.
//! La autorización de guardado se evalúa sobre **las diferencias reales** entre el documento
//! anterior y el nuevo: un revisor no puede modificar entradas del proyectista en silencio, un
//! proyectista no puede firmar revisiones ni aprobar excepciones, y un inspector solo escribe en
//! `inspections`. El programa no falsifica identidad ni firma profesional: el usuario local es un
//! nombre + rol declarados, y todo queda en la auditoría.

use crate::issues::Issue;
use crate::json::Json;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Admin,
    Designer,
    Reviewer,
    Inspector,
    Viewer,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Designer => "designer",
            Role::Reviewer => "reviewer",
            Role::Inspector => "inspector",
            Role::Viewer => "viewer",
        }
    }
    pub fn parse(s: &str) -> Option<Role> {
        Some(match s {
            "admin" => Role::Admin,
            "designer" => Role::Designer,
            "reviewer" => Role::Reviewer,
            "inspector" => Role::Inspector,
            "viewer" => Role::Viewer,
            _ => return None,
        })
    }
    pub const ALL: [Role; 5] = [Role::Admin, Role::Designer, Role::Reviewer, Role::Inspector, Role::Viewer];
}

/// Acciones que el motor reconoce (cada comando de la API declara la suya).
pub const ACTIONS: &[&str] = &[
    "project.read",
    "project.create",
    "project.write",
    "project.delete",
    "calc.run",
    "export.generate",
    "catalog.write",
    "profile.import",
    "review.add",
    "exception.approve",
    "inspection.write",
    "admin.users",
    "license.activate",
    "backup.create",
    "backup.restore",
    "audit.read",
    "audit.export",
    "settings.write",
];

pub fn can(role: Role, action: &str) -> bool {
    if !ACTIONS.contains(&action) {
        return false; // acción desconocida: denegada
    }
    match role {
        Role::Admin => true,
        Role::Designer => matches!(
            action,
            "project.read"
                | "project.create"
                | "project.write"
                | "calc.run"
                | "export.generate"
                | "catalog.write"
                | "backup.create"
                | "audit.read"
                | "audit.export"
                | "settings.write"
        ),
        Role::Reviewer => matches!(
            action,
            "project.read"
                | "calc.run"
                | "export.generate"
                | "profile.import"
                | "review.add"
                | "exception.approve"
                | "backup.create"
                | "audit.read"
                | "audit.export"
        ),
        Role::Inspector => matches!(action, "project.read" | "inspection.write" | "export.generate" | "audit.read"),
        Role::Viewer => matches!(action, "project.read" | "export.generate"),
    }
}

fn top_keys_changed(old: &Json, new: &Json) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    let (Some(a), Some(b)) = (old.as_obj(), new.as_obj()) else { return vec!["*".to_string()] };
    let mut all: Vec<&String> = a.keys().chain(b.keys()).collect();
    all.sort();
    all.dedup();
    for k in all {
        match (a.get(k), b.get(k)) {
            (Some(x), Some(y)) if x == y => {}
            (Some(Json::Obj(x)), Some(Json::Obj(y))) if k == "meta" => {
                let mut sub: Vec<&String> = x.keys().chain(y.keys()).collect();
                sub.sort();
                sub.dedup();
                for s in sub {
                    if x.get(s) != y.get(s) {
                        keys.push(format!("meta.{}", s));
                    }
                }
            }
            _ => keys.push(k.clone()),
        }
    }
    keys
}

fn approved_exceptions(p: &Json) -> Vec<String> {
    p.field("exceptions")
        .items()
        .iter()
        .filter(|e| e.field("approved_by").as_str().map(|s| !s.trim().is_empty()).unwrap_or(false))
        .map(|e| e.canonical())
        .collect()
}

/// ¿Puede `role` guardar `new` sobre `old`? Devuelve la incidencia con la primera ruta prohibida.
pub fn authorize_save(role: Role, old: &Json, new: &Json) -> Result<(), Issue> {
    let changed = top_keys_changed(old, new);
    if changed.is_empty() {
        return Ok(());
    }
    let deny =
        |path: &str| Issue::block("auth.forbidden_change").module("auth").path(path).param("role", role.as_str()).param("path", path);
    let volatile = |k: &str| matches!(k, "meta.updated_at" | "meta.revision" | "meta.revisions" | "meta.status");
    match role {
        Role::Admin => Ok(()),
        Role::Viewer => Err(deny(&changed[0])),
        Role::Inspector => {
            for k in &changed {
                if k != "inspections" && !volatile(k) {
                    return Err(deny(k));
                }
            }
            Ok(())
        }
        Role::Reviewer => {
            for k in &changed {
                if !(k == "reviews" || k == "exceptions" || k == "results" || volatile(k)) {
                    return Err(deny(k));
                }
            }
            Ok(())
        }
        Role::Designer => {
            for k in &changed {
                if k == "reviews" {
                    return Err(deny(k));
                }
            }
            // las excepciones aprobadas por un revisor no pueden alterarse ni crearse aquí
            if approved_exceptions(old) != approved_exceptions(new) {
                return Err(deny("exceptions"));
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::new_project;

    fn p() -> Json {
        new_project("P1", "Prueba", "es", "SI", "2026-10-28T00:00:00Z")
    }

    #[test]
    fn matriz_de_permisos() {
        assert!(can(Role::Admin, "admin.users") && can(Role::Admin, "license.activate"));
        assert!(
            can(Role::Designer, "project.write")
                && !can(Role::Designer, "review.add")
                && !can(Role::Designer, "profile.import")
                && !can(Role::Designer, "admin.users")
        );
        assert!(can(Role::Reviewer, "review.add") && can(Role::Reviewer, "profile.import") && !can(Role::Reviewer, "project.write"));
        assert!(can(Role::Inspector, "inspection.write") && !can(Role::Inspector, "calc.run"));
        assert!(can(Role::Viewer, "project.read") && !can(Role::Viewer, "project.write") && !can(Role::Viewer, "calc.run"));
        // acción desconocida: denegada incluso para el administrador
        assert!(!can(Role::Admin, "formatear.disco"));
        // todo rol puede leer
        for r in Role::ALL {
            assert!(can(r, "project.read"), "{:?}", r);
            assert_eq!(Role::parse(r.as_str()), Some(r));
        }
        assert_eq!(Role::parse("root"), None);
    }

    #[test]
    fn el_revisor_no_modifica_entradas_en_silencio() {
        let old = p();
        let mut new = old.clone();
        new.set_at("site.height_m", Json::num(20.0));
        let e = authorize_save(Role::Reviewer, &old, &new).unwrap_err();
        assert_eq!(e.code, "auth.forbidden_change");
        assert_eq!(e.params.get("path").map(String::as_str), Some("site"));
        // pero sí puede añadir revisiones, excepciones y resultados
        let mut ok = old.clone();
        ok.set_at("reviews", Json::Arr(vec![Json::obj().with("id", Json::str("RV1"))]));
        ok.set_at("exceptions", Json::Arr(vec![Json::obj().with("id", Json::str("EX1"))]));
        ok.set_at("results.extinguishers", Json::obj());
        ok.set_at("meta.updated_at", Json::str("2027-01-01T00:00:00Z"));
        assert!(authorize_save(Role::Reviewer, &old, &ok).is_ok());
    }

    #[test]
    fn el_proyectista_no_firma_revisiones_ni_aprueba_excepciones() {
        let old = p();
        let mut new = old.clone();
        new.set_at("reviews", Json::Arr(vec![Json::obj().with("id", Json::str("RV1"))]));
        assert!(authorize_save(Role::Designer, &old, &new).is_err());
        // puede proponer una excepción (sin aprobación)…
        let mut prop = old.clone();
        prop.set_at("exceptions", Json::Arr(vec![Json::obj().with("id", Json::str("EX1")).with("approved_by", Json::Null)]));
        assert!(authorize_save(Role::Designer, &old, &prop).is_ok());
        // …pero no puede autoaprobarla
        let mut self_ok = old.clone();
        self_ok.set_at("exceptions", Json::Arr(vec![Json::obj().with("id", Json::str("EX1")).with("approved_by", Json::str("yo mismo"))]));
        assert_eq!(authorize_save(Role::Designer, &old, &self_ok).unwrap_err().params.get("path").map(String::as_str), Some("exceptions"));
        // ni borrar una aprobada por el revisor
        let approved = self_ok.clone();
        let mut removed = approved.clone();
        removed.set_at("exceptions", Json::arr());
        assert!(authorize_save(Role::Designer, &approved, &removed).is_err());
        // sí puede cambiar entradas
        let mut edit = old.clone();
        edit.set_at("site.height_m", Json::num(12.0));
        assert!(authorize_save(Role::Designer, &old, &edit).is_ok());
    }

    #[test]
    fn inspector_y_lectura() {
        let old = p();
        let mut insp = old.clone();
        insp.set_at("inspections", Json::Arr(vec![Json::obj().with("id", Json::str("I1"))]));
        assert!(authorize_save(Role::Inspector, &old, &insp).is_ok());
        let mut bad = insp.clone();
        bad.set_at("site.height_m", Json::num(1.0));
        assert!(authorize_save(Role::Inspector, &old, &bad).is_err());
        assert!(authorize_save(Role::Viewer, &old, &insp).is_err());
        // sin cambios: siempre permitido
        assert!(authorize_save(Role::Viewer, &old, &old).is_ok());
        assert!(authorize_save(Role::Admin, &old, &bad).is_ok());
    }
}
