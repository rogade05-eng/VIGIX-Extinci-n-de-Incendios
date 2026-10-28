//! Capa de aplicación: un único punto de entrada `App::handle(json)` para la interfaz, el servidor
//! local de desarrollo y el shell Tauri. Aplica **roles**, **auditoría** y límites de entrada, y
//! convierte cualquier pánico en un error explícito (nunca un resultado parcial como válido).
//!
//! Sobre de petición: `{"cmd": "project.save", "args": {...}}`.
//! Respuesta: `{"ok": true, "result": ...}` o `{"ok": false, "error": {"code", "params", "path"}}`.

use crate::agents;
use crate::audit::{self, Entry};
use crate::budget;
use crate::calc::CalcOutput;
use crate::cause_effect;
use crate::export::{self, ExportContext};
use crate::extinguishers;
use crate::gates::{self, DocStatus};
use crate::hash::{base64_decode, base64_encode, sha256_hex};
use crate::hydraulics::model as hyd;
use crate::issues::{Issue, IssueList, Severity};
use crate::json::{Json, Limits};
use crate::license::{self, SignatureVerifier};
use crate::permissions::{self, Role};
use crate::profile::{self, Profile, Registry};
use crate::project;
use crate::selftest;
use crate::store::{self, Store, StoreError};
use crate::timeutil::{iso8601_utc, Clock};
use crate::{ENGINE_VERSION, PRODUCT_NAME};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

/// Límite de tamaño de una petición (contrato 8.4: límites de memoria).
pub const MAX_REQUEST_BYTES: usize = 128 * 1024 * 1024;

const UNITS_JSON: &str = include_str!("../../../shared/units.json");
const ES_JSON: &str = include_str!("../../../i18n/es.json");
const EN_JSON: &str = include_str!("../../../i18n/en.json");
const PROFILE_TEMPLATE: &str = include_str!("../../../profiles/plantilla/PLANTILLA-PERFIL-VACIA.json");
const CATALOG_DEMO: &str = include_str!("../../../catalog/demo_catalogo.csv");

#[derive(Debug, Clone)]
pub struct ApiError {
    pub code: String,
    pub params: BTreeMap<String, String>,
    pub path: Option<String>,
}

impl ApiError {
    pub fn new(code: &str) -> ApiError {
        ApiError { code: code.to_string(), params: BTreeMap::new(), path: None }
    }
    pub fn param<V: ToString>(mut self, k: &str, v: V) -> ApiError {
        self.params.insert(k.to_string(), v.to_string());
        self
    }
    pub fn path(mut self, p: &str) -> ApiError {
        self.path = Some(p.to_string());
        self
    }
    pub fn to_json(&self) -> Json {
        let mut p = Json::obj();
        for (k, v) in &self.params {
            p.set(k.as_str(), Json::str(v.as_str()));
        }
        Json::obj().with("code", Json::str(self.code.as_str())).with("params", p).with("path", Json::opt_str(self.path.as_deref()))
    }
}

impl From<Issue> for ApiError {
    fn from(i: Issue) -> ApiError {
        let mut e = ApiError::new(&i.code);
        e.params = i.params.clone();
        if !i.path.is_empty() {
            e.path = Some(i.path.clone());
        }
        e
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> ApiError {
        let msg = e.to_string();
        ApiError::new(e.code()).param("detail", msg)
    }
}

type R = Result<Json, ApiError>;

#[derive(Clone, Debug)]
pub struct Session {
    pub user: String,
    pub role: Role,
}

pub struct App {
    store: Mutex<Box<dyn Store>>,
    registry: RwLock<Registry>,
    bundled: Mutex<Vec<(String, String)>>,
    clock: Box<dyn Clock>,
    verifier: Box<dyn SignatureVerifier>,
    license: Mutex<license::Status>,
    session: Mutex<Session>,
    machine_id: Option<String>,
    jobs: Mutex<BTreeMap<String, Arc<AtomicBool>>>,
    unclean_exit: bool,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn a_str<'a>(args: &'a Json, key: &str) -> Result<&'a str, ApiError> {
    args.field(key).as_str().ok_or_else(|| ApiError::new("data.missing").param("field", key).path(key))
}
fn a_obj<'a>(args: &'a Json, key: &str) -> Result<&'a Json, ApiError> {
    match args.field(key) {
        j @ Json::Obj(_) => Ok(j),
        _ => Err(ApiError::new("data.missing").param("field", key).path(key)),
    }
}

impl App {
    pub fn new(store: Box<dyn Store>, clock: Box<dyn Clock>, verifier: Box<dyn SignatureVerifier>, machine_id: Option<String>) -> App {
        let mut store = store;
        // ¿se cerró bien la sesión anterior?
        let unclean = store.get_setting("session.clean_exit").ok().flatten().and_then(|j| j.as_bool()) == Some(false);
        let _ = store.set_setting("session.clean_exit", &Json::Bool(false));
        // usuarios: el primero es administrador local
        let users = store.get_setting("users").ok().flatten();
        let first_user = match users.as_ref().and_then(|u| u.idx(0)).and_then(|u| u.field("name").as_str().map(|s| s.to_string())) {
            Some(n) => (
                n,
                users.as_ref().and_then(|u| u.idx(0)).and_then(|u| u.field("role").as_str().and_then(Role::parse)).unwrap_or(Role::Viewer),
            ),
            None => {
                let name = std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "usuario".to_string());
                let name: String = name.chars().filter(|c| !c.is_control()).take(64).collect();
                let name = if name.trim().is_empty() { "usuario".to_string() } else { name };
                let _ = store.set_setting(
                    "users",
                    &Json::Arr(vec![Json::obj().with("name", Json::str(name.as_str())).with("role", Json::str("admin"))]),
                );
                (name, Role::Admin)
            }
        };
        let app = App {
            store: Mutex::new(store),
            registry: RwLock::new(Registry::new()),
            bundled: Mutex::new(Vec::new()),
            clock,
            verifier,
            license: Mutex::new(license::Status::freemium(None)),
            session: Mutex::new(Session { user: first_user.0, role: first_user.1 }),
            machine_id,
            jobs: Mutex::new(BTreeMap::new()),
            unclean_exit: unclean,
        };
        // perfiles importados por el usuario
        let imported = lock(&app.store).list_profiles().unwrap_or_default();
        for pkg in imported {
            let _ = app.register_profile(pkg);
        }
        // licencia
        if let Some(text) =
            lock(&app.store).get_setting("license").ok().flatten().and_then(|j| j.field("text").as_str().map(|s| s.to_string()))
        {
            *lock(&app.license) = license::check(&text, app.verifier.as_ref(), app.machine_id.as_deref());
        }
        app.audit(
            "app.start",
            "",
            Json::obj().with("version", Json::str(ENGINE_VERSION)).with("unclean_previous_exit", Json::Bool(unclean)),
        );
        app
    }

    /// Marca la salida limpia (para detectar cierres inesperados al reabrir).
    pub fn mark_clean_exit(&self) {
        let _ = lock(&self.store).set_setting("session.clean_exit", &Json::Bool(true));
        self.audit("app.exit", "", Json::Null);
    }

    /// Carga perfiles distribuidos con la aplicación (solo lectura; no se copian al almacén).
    pub fn load_bundled_profiles(&self, dir: &Path) -> Vec<String> {
        let mut loaded = Vec::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            let mut files: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
            files.sort();
            for p in files {
                if p.extension().map(|e| e == "json").unwrap_or(false) {
                    if let Ok(t) = std::fs::read_to_string(&p) {
                        if let Ok(j) = Json::parse(&t) {
                            if j.field("profile_code").as_str().is_some() {
                                if let Ok((c, v)) = self.register_profile(j) {
                                    lock(&self.bundled).push((c.clone(), v.clone()));
                                    loaded.push(format!("{}@{}", c, v));
                                }
                            }
                        }
                    }
                }
            }
        }
        // también subcarpetas directas (profiles/demo/...)
        if let Ok(rd) = std::fs::read_dir(dir) {
            let mut subs: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
            subs.sort();
            for d in subs {
                if d.file_name().and_then(|n| n.to_str()) == Some("plantilla") {
                    continue;
                }
                loaded.extend(self.load_bundled_profiles(&d));
            }
        }
        loaded
    }

    /// Registra un paquete y ejecuta su autotest; si algún caso falla, el perfil se degrada.
    fn register_profile(&self, pkg: Json) -> Result<(String, String), Vec<Issue>> {
        let mut reg = self.registry.write().unwrap_or_else(|e| e.into_inner());
        let (code, ver) = {
            let p = reg.add(pkg)?;
            (p.code().to_string(), p.version().to_string())
        };
        let failed: Vec<selftest::CaseResult> = match reg.get(&code, &ver) {
            Some(p) => selftest::run(p).into_iter().filter(|r| !r.passed).collect(),
            None => vec![],
        };
        if !failed.is_empty() {
            reg.downgrade(
                &code,
                &ver,
                Issue::error("profile.selftest_failed")
                    .module("profile")
                    .param("cases", failed.iter().map(|f| f.id.clone()).collect::<Vec<_>>().join(", ")),
            );
        }
        Ok((code, ver))
    }

    fn now(&self) -> String {
        iso8601_utc(self.clock.now_unix())
    }

    fn session(&self) -> Session {
        lock(&self.session).clone()
    }

    fn require(&self, action: &str) -> Result<Session, ApiError> {
        let s = self.session();
        if permissions::can(s.role, action) {
            Ok(s)
        } else {
            Err(ApiError::new("auth.denied").param("action", action).param("role", s.role.as_str()))
        }
    }

    fn audit(&self, action: &str, project_id: &str, detail: Json) {
        let s = self.session();
        let profile_label = String::new();
        self.audit_with(action, project_id, detail, &s, &profile_label);
    }

    fn audit_with(&self, action: &str, project_id: &str, detail: Json, s: &Session, profile_label: &str) {
        let mut st = lock(&self.store);
        let prev = st.read_audit().ok().and_then(|v| v.last().cloned());
        let e = audit::next_entry(
            prev.as_ref(),
            &self.now(),
            &s.user,
            s.role.as_str(),
            ENGINE_VERSION,
            profile_label,
            action,
            project_id,
            detail,
        );
        let _ = st.append_audit(&e);
    }

    fn profile_label(&self, project: &Json) -> String {
        match (
            project.at("jurisdiction.profile_code").and_then(Json::as_str),
            project.at("jurisdiction.profile_version").and_then(Json::as_str),
        ) {
            (Some(c), Some(v)) => format!("{}@{}", c, v),
            _ => String::new(),
        }
    }

    fn profile_of(&self, project: &Json) -> Result<Profile, Issue> {
        match (
            project.at("jurisdiction.profile_code").and_then(Json::as_str),
            project.at("jurisdiction.profile_version").and_then(Json::as_str),
        ) {
            (Some(c), Some(v)) => self.registry.read().unwrap_or_else(|e| e.into_inner()).get(c, v).cloned().ok_or_else(|| {
                Issue::block("gate.profile_missing").module("project").path("jurisdiction").param("code", c).param("version", v)
            }),
            _ => Err(Issue::block("project.no_profile").module("project").path("jurisdiction.profile_code")),
        }
    }

    // ================================================================= entrada
    /// Punto de entrada del sobre JSON. Nunca entra en pánico hacia el exterior.
    pub fn handle(&self, request: &str) -> String {
        let resp = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let req = match Json::parse_with_limits(request, Limits { max_bytes: MAX_REQUEST_BYTES, max_depth: 128 }) {
                Ok(r) => r,
                Err(e) => return Err(ApiError::new("api.bad_request").param("detail", e.to_string())),
            };
            let cmd = req.field("cmd").as_str().unwrap_or("").to_string();
            if cmd.is_empty() {
                return Err(ApiError::new("api.bad_request").param("detail", "cmd"));
            }
            self.dispatch(&cmd, req.field("args"))
        }));
        match resp {
            Ok(Ok(r)) => Json::obj().with("ok", Json::Bool(true)).with("result", r).to_compact(),
            Ok(Err(e)) => Json::obj().with("ok", Json::Bool(false)).with("error", e.to_json()).to_compact(),
            Err(_) => Json::obj().with("ok", Json::Bool(false)).with("error", ApiError::new("internal.panic").to_json()).to_compact(),
        }
    }

    pub fn dispatch(&self, cmd: &str, args: &Json) -> R {
        match cmd {
            "app.info" => self.app_info(),
            "app.resource" => self.app_resource(args),
            "session.get" => Ok(self.session_json()),
            "session.set" => self.session_set(args),
            "users.list" => Ok(lock(&self.store).get_setting("users")?.unwrap_or(Json::arr())),
            "users.save" => self.users_save(args),
            "users.delete" => self.users_delete(args),
            "settings.get" => self.settings_get(args),
            "settings.set" => self.settings_set(args),
            "profile.list" => self.profile_list(),
            "profile.get" => self.profile_get(args),
            "profile.verify" => self.profile_verify(args),
            "profile.import" => self.profile_import(args),
            "profile.seal" => self.profile_seal(args),
            "profile.selftest" => self.profile_selftest(args),
            "project.list" => Ok(Json::Arr(lock(&self.store).list_projects()?.iter().map(|m| m.to_json()).collect())),
            "project.new" => self.project_new(args),
            "project.open" => self.project_open(args),
            "project.import" => self.project_import(args),
            "project.save" => self.project_save(args),
            "project.delete" => self.project_delete(args),
            "project.validate" => Ok(project::validate(a_obj(args, "project")?).to_json()),
            "project.status" => self.project_status(a_obj(args, "project")?),
            "project.new_revision" => self.project_new_revision(args),
            "autosave.write" => self.autosave_write(args),
            "autosave.read" => Ok(lock(&self.store).get_setting("autosave")?.unwrap_or(Json::Null)),
            "autosave.clear" => {
                lock(&self.store).set_setting("autosave", &Json::Null)?;
                Ok(Json::Null)
            }
            "calc.extinguishers" | "calc.hydraulic" | "calc.foam" | "calc.gas" | "calc.dry_chemical" | "calc.wet_chemical"
            | "calc.aerosol" | "calc.cause_effect" | "calc.budget" => self.calc_one(cmd, args),
            "calc.all" => self.calc_all(args),
            "calc.cancel" => self.calc_cancel(args),
            "calc.estimate" => self.calc_estimate(args),
            "review.add" => self.review_add(args),
            "exception.add" => self.exception_add(args),
            "exception.approve" => self.exception_approve(args),
            "catalog.import_csv" => self.catalog_import(args),
            "catalog.export_csv" => self.catalog_export(args),
            "catalog.demo" => self.catalog_demo(),
            "export.run" => self.export_run(args),
            "audit.list" => self.audit_list(args),
            "audit.verify" => self.audit_verify(),
            "audit.export" => self.audit_export(args),
            "backup.create" => self.backup_create(args),
            "backup.list" => Ok(Json::Arr(lock(&self.store).list_backups()?.into_iter().map(Json::Str).collect())),
            "backup.restore" => self.backup_restore(args),
            "license.status" => Ok(lock(&self.license).to_json()),
            "license.request_code" => Ok(Json::obj()
                .with("code", Json::opt_str(self.machine_id.as_deref().map(license::request_code).as_deref()))
                .with("machine_id_available", Json::Bool(self.machine_id.is_some()))),
            "license.activate" => self.license_activate(args),
            _ => Err(ApiError::new("api.unknown_command").param("cmd", cmd)),
        }
    }

    // ================================================================= app / sesión
    fn session_json(&self) -> Json {
        let s = self.session();
        Json::obj().with("user", Json::str(s.user.as_str())).with("role", Json::str(s.role.as_str())).with(
            "permissions",
            Json::Arr(permissions::ACTIONS.iter().filter(|a| permissions::can(s.role, a)).map(|a| Json::str(*a)).collect()),
        )
    }

    fn app_info(&self) -> R {
        let reg = self.registry.read().unwrap_or_else(|e| e.into_inner());
        Ok(Json::obj()
            .with("name", Json::str(PRODUCT_NAME))
            .with("version", Json::str(ENGINE_VERSION))
            .with("engine_version", Json::str(ENGINE_VERSION))
            .with("store", Json::str(lock(&self.store).kind()))
            .with("languages", Json::Arr(vec![Json::str("es"), Json::str("en")]))
            .with("unit_systems", Json::Arr(vec![Json::str("SI"), Json::str("US")]))
            .with("session", self.session_json())
            .with("license", lock(&self.license).to_json())
            .with("profiles", Json::num(reg.list().len() as f64))
            .with("unclean_previous_exit", Json::Bool(self.unclean_exit))
            .with("roles", Json::Arr(Role::ALL.iter().map(|r| Json::str(r.as_str())).collect()))
            .with("max_request_bytes", Json::num(MAX_REQUEST_BYTES as f64))
            .with("network", Json::str("none: 100 % offline, sin telemetría")))
    }

    fn app_resource(&self, args: &Json) -> R {
        let name = a_str(args, "name")?;
        let text = match name {
            "units" => UNITS_JSON,
            "i18n.es" => ES_JSON,
            "i18n.en" => EN_JSON,
            "profile.template" => PROFILE_TEMPLATE,
            "catalog.demo" => CATALOG_DEMO,
            _ => return Err(ApiError::new("api.unknown_resource").param("name", name)),
        };
        if name == "catalog.demo" {
            return Ok(Json::str(text));
        }
        Json::parse(text).map_err(|e| ApiError::new("api.bad_resource").param("detail", e.to_string()))
    }

    fn session_set(&self, args: &Json) -> R {
        let name = a_str(args, "user")?;
        let users = lock(&self.store).get_setting("users")?.unwrap_or(Json::arr());
        let found = users.items().iter().find(|u| u.field("name").as_str() == Some(name)).cloned();
        let Some(u) = found else { return Err(ApiError::new("users.unknown").param("user", name)) };
        let role = u.field("role").as_str().and_then(Role::parse).unwrap_or(Role::Viewer);
        let before = self.session();
        *lock(&self.session) = Session { user: name.to_string(), role };
        self.audit_with(
            "session.change",
            "",
            Json::obj().with("from", Json::str(before.user.as_str())).with("to", Json::str(name)).with("role", Json::str(role.as_str())),
            &Session { user: name.to_string(), role },
            "",
        );
        Ok(self.session_json())
    }

    fn users_save(&self, args: &Json) -> R {
        self.require("admin.users")?;
        let name = a_str(args, "name")?.trim();
        let role = a_str(args, "role")?;
        if name.is_empty() || name.chars().count() > 64 || name.chars().any(|c| c.is_control()) {
            return Err(ApiError::new("data.bad_enum").param("field", "name").param("value", name).param("allowed", "1..64"));
        }
        if Role::parse(role).is_none() {
            return Err(ApiError::new("data.bad_enum")
                .param("field", "role")
                .param("value", role)
                .param("allowed", "admin|designer|reviewer|inspector|viewer"));
        }
        let mut users = lock(&self.store).get_setting("users")?.unwrap_or(Json::arr());
        let mut list: Vec<Json> = users.items().to_vec();
        let new_u = Json::obj().with("name", Json::str(name)).with("role", Json::str(role));
        match list.iter().position(|u| u.field("name").as_str() == Some(name)) {
            Some(i) => {
                // no se puede degradar al último administrador
                let admins = list.iter().filter(|u| u.field("role").as_str() == Some("admin")).count();
                if list[i].field("role").as_str() == Some("admin") && role != "admin" && admins <= 1 {
                    return Err(ApiError::new("users.last_admin"));
                }
                list[i] = new_u;
            }
            None => list.push(new_u),
        }
        users = Json::Arr(list);
        lock(&self.store).set_setting("users", &users)?;
        self.audit("user.save", "", Json::obj().with("name", Json::str(name)).with("role", Json::str(role)));
        Ok(users)
    }

    fn users_delete(&self, args: &Json) -> R {
        self.require("admin.users")?;
        let name = a_str(args, "name")?;
        let users = lock(&self.store).get_setting("users")?.unwrap_or(Json::arr());
        let list: Vec<Json> = users.items().to_vec();
        let Some(i) = list.iter().position(|u| u.field("name").as_str() == Some(name)) else {
            return Err(ApiError::new("users.unknown").param("user", name));
        };
        let admins = list.iter().filter(|u| u.field("role").as_str() == Some("admin")).count();
        if list[i].field("role").as_str() == Some("admin") && admins <= 1 {
            return Err(ApiError::new("users.last_admin"));
        }
        if name == self.session().user {
            return Err(ApiError::new("users.cannot_delete_self"));
        }
        let rest: Vec<Json> = list.into_iter().filter(|u| u.field("name").as_str() != Some(name)).collect();
        lock(&self.store).set_setting("users", &Json::Arr(rest.clone()))?;
        self.audit("user.delete", "", Json::obj().with("name", Json::str(name)));
        Ok(Json::Arr(rest))
    }

    const SETTING_KEYS: &'static [&'static str] = &[
        "language",
        "unit_system",
        "autosave_seconds",
        "backup_minutes",
        "theme",
        "symbols",
        "recent_projects",
        "catalog_global",
        "high_contrast",
        "font_scale",
    ];

    fn settings_get(&self, args: &Json) -> R {
        let st = lock(&self.store);
        match args.field("key").as_str() {
            Some(k) if Self::SETTING_KEYS.contains(&k) => Ok(st.get_setting(k)?.unwrap_or(Json::Null)),
            Some(k) => Err(ApiError::new("settings.unknown_key").param("key", k)),
            None => {
                let mut o = Json::obj();
                for k in Self::SETTING_KEYS {
                    if let Some(v) = st.get_setting(k)? {
                        o.set(*k, v);
                    }
                }
                Ok(o)
            }
        }
    }

    fn settings_set(&self, args: &Json) -> R {
        self.require("settings.write")?;
        let k = a_str(args, "key")?;
        let v = args.field("value");
        if !Self::SETTING_KEYS.contains(&k) {
            return Err(ApiError::new("settings.unknown_key").param("key", k));
        }
        let bad = |allowed: &str| ApiError::new("data.bad_enum").param("field", k).param("value", v.to_compact()).param("allowed", allowed);
        match k {
            "language" if !matches!(v.as_str(), Some("es") | Some("en")) => return Err(bad("es|en")),
            "unit_system" if !matches!(v.as_str(), Some("SI") | Some("US")) => return Err(bad("SI|US")),
            "autosave_seconds" if !matches!(v.as_f64(), Some(x) if (0.0..=3600.0).contains(&x) && x.fract() == 0.0) => {
                return Err(bad("0..3600"))
            }
            "backup_minutes" if !matches!(v.as_f64(), Some(x) if (0.0..=1440.0).contains(&x) && x.fract() == 0.0) => {
                return Err(bad("0..1440"))
            }
            "high_contrast" if v.as_bool().is_none() => return Err(bad("true|false")),
            "font_scale" if !matches!(v.as_f64(), Some(x) if (0.75..=2.0).contains(&x)) => return Err(bad("0.75..2")),
            _ => {}
        }
        if v.canonical().len() > 8 * 1024 * 1024 {
            return Err(ApiError::new("settings.too_large").param("key", k));
        }
        lock(&self.store).set_setting(k, v)?;
        self.audit("settings.set", "", Json::obj().with("key", Json::str(k)));
        Ok(Json::Null)
    }

    // ================================================================= perfiles
    fn profile_list(&self) -> R {
        let reg = self.registry.read().unwrap_or_else(|e| e.into_inner());
        let bundled = lock(&self.bundled).clone();
        Ok(Json::Arr(
            reg.list()
                .iter()
                .map(|p| {
                    let mut s = p.summary_json();
                    s.set("bundled", Json::Bool(bundled.iter().any(|(c, v)| c == p.code() && v == p.version())));
                    s.set("selftest_cases", Json::num(p.raw.field("test_cases").items().len() as f64));
                    s
                })
                .collect(),
        ))
    }

    fn profile_get(&self, args: &Json) -> R {
        let (c, v) = (a_str(args, "code")?, a_str(args, "version")?);
        let reg = self.registry.read().unwrap_or_else(|e| e.into_inner());
        match reg.get(c, v) {
            Some(p) => Ok(Json::obj().with("summary", p.summary_json()).with("package", p.raw.clone())),
            None => Err(ApiError::new("gate.profile_missing").param("code", c).param("version", v)),
        }
    }

    fn profile_verify(&self, args: &Json) -> R {
        let pkg = a_obj(args, "package")?;
        let st = profile::assess(pkg);
        let mut o = Json::obj()
            .with("level", Json::str(st.level.as_str()))
            .with("computed_sha256", Json::str(st.computed_sha256.as_str()))
            .with("declared_sha256", Json::opt_str(st.declared_sha256.as_deref()))
            .with("issues", Json::Arr(st.issues.iter().map(Issue::to_json).collect()));
        if st.level != profile::Level::Invalid {
            if let Ok(p) = Profile::from_json(pkg.clone()) {
                o.set("selftest", selftest::summary_json(&selftest::run(&p)));
            }
        }
        Ok(o)
    }

    fn profile_import(&self, args: &Json) -> R {
        let s = self.require("profile.import")?;
        let pkg = a_obj(args, "package")?.clone();
        let (code, ver) = self
            .register_profile(pkg.clone())
            .map_err(|v| ApiError::from(v.into_iter().next().unwrap_or_else(|| Issue::block("profile.internal"))))?;
        lock(&self.store).save_profile(&code, &ver, &pkg)?;
        let (level, sha) = {
            let reg = self.registry.read().unwrap_or_else(|e| e.into_inner());
            reg.get(&code, &ver).map(|p| (p.level.as_str().to_string(), p.sha256.clone())).unwrap_or_default()
        };
        self.audit_with(
            "profile.import",
            "",
            Json::obj()
                .with("code", Json::str(code.as_str()))
                .with("version", Json::str(ver.as_str()))
                .with("sha256", Json::str(sha.as_str()))
                .with("level", Json::str(level.as_str())),
            &s,
            "",
        );
        self.profile_get(&Json::obj().with("code", Json::str(code)).with("version", Json::str(ver)))
    }

    fn profile_seal(&self, args: &Json) -> R {
        self.require("profile.import")?;
        let mut pkg = a_obj(args, "package")?.clone();
        let h = profile::seal(&mut pkg);
        Ok(Json::obj().with("sha256", Json::str(h)).with("package", pkg))
    }

    fn profile_selftest(&self, args: &Json) -> R {
        let (c, v) = (a_str(args, "code")?, a_str(args, "version")?);
        let reg = self.registry.read().unwrap_or_else(|e| e.into_inner());
        match reg.get(c, v) {
            Some(p) => Ok(selftest::summary_json(&selftest::run(p))),
            None => Err(ApiError::new("gate.profile_missing").param("code", c).param("version", v)),
        }
    }

    // ================================================================= proyectos
    fn project_new(&self, args: &Json) -> R {
        let s = self.require("project.create")?;
        let name = a_str(args, "name")?.trim();
        if name.is_empty() || name.chars().count() > 200 {
            return Err(ApiError::new("data.empty").param("field", "name").path("name"));
        }
        let (lang, units) = {
            let st = lock(&self.store);
            (
                args.field("language")
                    .as_str()
                    .map(|s| s.to_string())
                    .or_else(|| st.get_setting("language").ok().flatten().and_then(|j| j.as_str().map(|s| s.to_string())))
                    .unwrap_or_else(|| "es".to_string()),
                args.field("unit_system")
                    .as_str()
                    .map(|s| s.to_string())
                    .or_else(|| st.get_setting("unit_system").ok().flatten().and_then(|j| j.as_str().map(|s| s.to_string())))
                    .unwrap_or_else(|| "SI".to_string()),
            )
        };
        let now = self.now();
        let existing: Vec<String> = lock(&self.store).list_projects()?.into_iter().map(|m| m.id).collect();
        let day: String = now.chars().take(10).filter(|c| c.is_ascii_digit()).collect();
        let mut n = 0u32;
        let id = loop {
            let h = sha256_hex(format!("{}|{}|{}", name, now, n).as_bytes());
            let id = format!("PRJ-{}-{}", day, h[..4].to_uppercase());
            if !existing.contains(&id) {
                break id;
            }
            n += 1;
        };
        let mut p = project::new_project(&id, name, &lang, &units, &now);
        p.set_at("meta.author", Json::str(s.user.as_str()));
        lock(&self.store).save_project(&id, &p)?;
        self.audit_with("project.create", &id, Json::obj().with("name", Json::str(name)), &s, "");
        Ok(p)
    }

    fn project_open(&self, args: &Json) -> R {
        self.require("project.read")?;
        let id = a_str(args, "id")?;
        let raw = lock(&self.store).load_project(id)?;
        let (p, notes) = project::migrate(raw).map_err(ApiError::from)?;
        let mut out = Json::obj().with("project", p.clone()).with("migration_notes", Json::Arr(notes.into_iter().map(Json::Str).collect()));
        out.set("status", self.project_status(&p)?);
        Ok(out)
    }

    /// Abre un documento externo (texto JSON) sin guardarlo todavía.
    fn project_import(&self, args: &Json) -> R {
        self.require("project.create")?;
        let text = a_str(args, "text")?;
        let j = Json::parse_with_limits(text, Limits { max_bytes: MAX_REQUEST_BYTES, max_depth: 128 })
            .map_err(|e| ApiError::new("project.file_corrupt").param("detail", e.to_string()))?;
        // un archivo exportado envuelve el proyecto en {"project": ...}
        let inner = if j.field("format").as_str() == Some("vigix.export/1") { j.field("project").clone() } else { j };
        let (p, notes) = project::migrate(inner).map_err(ApiError::from)?;
        Ok(Json::obj()
            .with("project", p.clone())
            .with("migration_notes", Json::Arr(notes.into_iter().map(Json::Str).collect()))
            .with("status", self.project_status(&p)?))
    }

    fn project_save(&self, args: &Json) -> R {
        let s = self.session();
        let mut new = a_obj(args, "project")?.clone();
        let id = args
            .field("id")
            .as_str()
            .map(|s| s.to_string())
            .or_else(|| new.at("meta.id").and_then(Json::as_str).map(|s| s.to_string()))
            .ok_or_else(|| ApiError::new("data.missing").param("field", "id"))?;
        if !store::valid_id(&id) {
            return Err(ApiError::new("store.invalid").param("detail", id));
        }
        if new.at("meta.id").and_then(Json::as_str) != Some(id.as_str()) {
            return Err(ApiError::new("project.id_mismatch").param("id", id.as_str()));
        }
        let vi = project::validate(&new);
        if vi.0.iter().any(|i| i.code == "project.unsupported_schema" || i.code == "project.missing_section") {
            return Err(ApiError::from(
                vi.0.into_iter().find(|i| i.severity == Severity::Block).unwrap_or_else(|| Issue::block("project.unsupported_schema")),
            ));
        }
        let old = lock(&self.store).load_project(&id);
        let (old_doc, is_new) = match old {
            Ok(j) => (j, false),
            Err(StoreError::NotFound(_)) => (project::new_project(&id, "", "es", "SI", &self.now()), true),
            Err(e) => return Err(e.into()),
        };
        if is_new {
            if !permissions::can(s.role, "project.create") {
                return Err(ApiError::new("auth.denied").param("action", "project.create").param("role", s.role.as_str()));
            }
        } else if !permissions::can(s.role, "project.write")
            && !permissions::can(s.role, "review.add")
            && !permissions::can(s.role, "inspection.write")
        {
            return Err(ApiError::new("auth.denied").param("action", "project.write").param("role", s.role.as_str()));
        }
        if !is_new {
            permissions::authorize_save(s.role, &old_doc, &new).map_err(ApiError::from)?;
        }
        let diffs = audit::diff(&old_doc, &new, 60);
        let changed_inputs = project::inputs_fingerprint(&old_doc) != project::inputs_fingerprint(&new);
        new.set_at("meta.updated_at", Json::str(self.now()));
        lock(&self.store).save_project(&id, &new)?;
        let fp = project::inputs_fingerprint(&new);
        self.audit_with(
            "project.save",
            &id,
            Json::obj()
                .with("inputs_changed", Json::Bool(changed_inputs))
                .with("changes", Json::Arr(diffs))
                .with("inputs_fingerprint", Json::str(fp.as_str())),
            &s,
            &self.profile_label(&new),
        );
        Ok(Json::obj().with("project", new).with("inputs_fingerprint", Json::str(fp)).with("inputs_changed", Json::Bool(changed_inputs)))
    }

    fn project_delete(&self, args: &Json) -> R {
        let s = self.require("project.delete")?;
        let id = a_str(args, "id")?;
        lock(&self.store).delete_project(id)?;
        self.audit_with("project.delete", id, Json::Null, &s, "");
        Ok(Json::Null)
    }

    fn project_new_revision(&self, args: &Json) -> R {
        let s = self.require("project.write")?;
        let id = a_str(args, "id")?;
        let desc = a_str(args, "description")?.trim().to_string();
        if desc.is_empty() {
            return Err(ApiError::new("data.empty").param("field", "description"));
        }
        let mut p = lock(&self.store).load_project(id)?;
        let rev = p.at("meta.revision").and_then(Json::as_f64).unwrap_or(0.0) + 1.0;
        let mut revs = p.at("meta.revisions").map(|r| r.items().to_vec()).unwrap_or_default();
        revs.push(
            Json::obj()
                .with("rev", Json::num(rev))
                .with("date", Json::str(self.now()))
                .with("author", Json::str(s.user.as_str()))
                .with("description", Json::str(desc.as_str())),
        );
        p.set_at("meta.revision", Json::num(rev));
        p.set_at("meta.revisions", Json::Arr(revs));
        p.set_at("meta.updated_at", Json::str(self.now()));
        lock(&self.store).save_project(id, &p)?;
        self.audit_with(
            "project.revision",
            id,
            Json::obj().with("rev", Json::num(rev)).with("description", Json::str(desc)),
            &s,
            &self.profile_label(&p),
        );
        Ok(p)
    }

    fn autosave_write(&self, args: &Json) -> R {
        self.require("project.write")?;
        let p = a_obj(args, "project")?;
        lock(&self.store).set_setting(
            "autosave",
            &Json::obj()
                .with("saved_at", Json::str(self.now()))
                .with("id", p.at("meta.id").cloned().unwrap_or(Json::Null))
                .with("project", p.clone()),
        )?;
        Ok(Json::Null)
    }

    /// Validación + compuerta + estado de los resultados, sobre el documento tal como está en pantalla.
    fn project_status(&self, p: &Json) -> R {
        let reg = self.registry.read().unwrap_or_else(|e| e.into_inner());
        let gate = gates::evaluate(p, &reg);
        let validation = project::validate(p);
        let required: Vec<Json> = gates::required_results(p)
            .into_iter()
            .map(|r| {
                Json::obj().with("kind", Json::str(r.kind)).with("key", Json::opt_str(r.key.as_deref())).with("system", Json::str(r.system))
            })
            .collect();
        let profile = self.profile_of(p).ok().map(|pr| pr.summary_json()).unwrap_or(Json::Null);
        Ok(Json::obj()
            .with("validation", validation.to_json())
            .with("validation_summary", validation.summary_json())
            .with("gate", gate.to_json())
            .with("required_results", Json::Arr(required))
            .with("inputs_fingerprint", Json::str(project::inputs_fingerprint(p)))
            .with("profile", profile))
    }

    // ================================================================= cálculos
    fn envelope(&self, out: &CalcOutput, profile: Option<&Profile>, args_note: Json) -> Json {
        let mut e = out.to_json(profile, &self.now());
        e.set("args", args_note);
        e
    }

    fn blocked_env(&self, issue: Issue, kind: &str) -> Json {
        let out = CalcOutput::blocked(kind, vec![issue], String::new());
        out.to_json(None, &self.now())
    }

    fn find<'a>(list: &'a Json, key: &str, id: &str) -> Option<&'a Json> {
        list.items().iter().find(|x| x.field(key).as_str() == Some(id))
    }

    fn run_system(&self, kind: &str, project: &Json, profile: &Profile, args: &Json) -> Result<(Vec<String>, CalcOutput, Json), ApiError> {
        let sys_id = |a: &Json| -> Result<String, ApiError> { Ok(a_str(a, "system_id")?.to_string()) };
        match kind {
            "extinguishers" => {
                let suggest = args.get("suggest").filter(|s| !s.is_null());
                let out = extinguishers::run(project, profile, suggest);
                Ok((
                    vec!["results".into(), "extinguishers".into()],
                    out,
                    Json::obj().with("suggest", suggest.cloned().unwrap_or(Json::Null)),
                ))
            }
            "hydraulic" => {
                let (nid, sid) = (a_str(args, "network_id")?.to_string(), a_str(args, "scenario_id")?.to_string());
                let nets = project.at("systems.water.networks").cloned().unwrap_or(Json::arr());
                let net =
                    Self::find(&nets, "id", &nid).ok_or_else(|| ApiError::new("hyd.network_not_found").param("network", nid.as_str()))?;
                let mode_s = args.field("mode").as_str().unwrap_or("operating_point");
                let mode = if mode_s == "required_pressure" { hyd::Mode::RequiredPressure } else { hyd::Mode::OperatingPoint };
                let cancel = args.field("job_id").as_str().map(|j| {
                    let flag = Arc::new(AtomicBool::new(false));
                    lock(&self.jobs).insert(j.to_string(), flag.clone());
                    (j.to_string(), flag)
                });
                let out = hyd::run(net, &sid, mode, profile, cancel.as_ref().map(|c| c.1.as_ref()));
                if let Some((j, _)) = cancel {
                    lock(&self.jobs).remove(&j);
                }
                Ok((
                    vec!["results".into(), "hydraulic".into(), format!("{}:{}", nid, sid)],
                    out,
                    Json::obj().with("mode", Json::str(mode_s)).with("network_id", Json::str(nid)).with("scenario_id", Json::str(sid)),
                ))
            }
            "foam" | "gas" | "dry_chemical" | "wet_chemical" | "aerosol" => {
                let id = sys_id(args)?;
                let list = project.at(&format!("systems.{}", kind)).cloned().unwrap_or(Json::arr());
                let idx = list
                    .items()
                    .iter()
                    .position(|x| x.field("id").as_str() == Some(id.as_str()))
                    .ok_or_else(|| ApiError::new("api.system_not_found").param("kind", kind).param("id", id.as_str()))?;
                let sys = &list.items()[idx];
                let base = format!("systems.{}.{}", kind, idx);
                let out = match kind {
                    "foam" => agents::foam::run(sys, profile, &base),
                    "gas" => agents::gas::run(sys, profile, &base),
                    "dry_chemical" => agents::dry_chem::run(sys, profile, &base),
                    "wet_chemical" => agents::wet_chem::run(sys, profile, &base),
                    _ => agents::aerosol::run(sys, profile, &base),
                };
                Ok((vec!["results".into(), kind.into(), id], out, Json::Null))
            }
            "cause_effect" => Ok((vec!["results".into(), "cause_effect".into()], cause_effect::run(project, Some(profile)), Json::Null)),
            "budget" => {
                let items: Vec<budget::CatalogItem> =
                    project.at("catalog.items").map(Json::items).unwrap_or(&[]).iter().filter_map(budget::CatalogItem::from_json).collect();
                Ok((vec!["results".into(), "budget".into()], budget::compute(project, &items), Json::Null))
            }
            _ => Err(ApiError::new("api.unknown_command").param("cmd", kind)),
        }
    }

    fn calc_one(&self, cmd: &str, args: &Json) -> R {
        let s = self.require("calc.run")?;
        let kind = cmd.trim_start_matches("calc.");
        let project = a_obj(args, "project")?;
        let profile = match self.profile_of(project) {
            Ok(p) => p,
            Err(i) => {
                return Ok(Json::obj()
                    .with("path", Json::Arr(vec![Json::str("results"), Json::str(kind)]))
                    .with("envelope", self.blocked_env(i, kind)));
            }
        };
        let (path, out, note) = self.run_system(kind, project, &profile, args)?;
        let env = self.envelope(&out, Some(&profile), note);
        self.audit_with(
            "calc.run",
            project.at("meta.id").and_then(Json::as_str).unwrap_or(""),
            Json::obj()
                .with("kind", Json::str(kind))
                .with("path", Json::Arr(path.iter().map(|p| Json::str(p.as_str())).collect()))
                .with("status", Json::str(out.status.as_str()))
                .with("fingerprint", Json::str(out.fingerprint.as_str())),
            &s,
            &self.profile_label(project),
        );
        Ok(Json::obj().with("path", Json::Arr(path.into_iter().map(Json::Str).collect())).with("envelope", env))
    }

    /// Ejecuta todos los cálculos que correspondan y devuelve el árbol `results` completo.
    fn calc_all(&self, args: &Json) -> R {
        let s = self.require("calc.run")?;
        let project = a_obj(args, "project")?;
        let profile = match self.profile_of(project) {
            Ok(p) => p,
            Err(i) => {
                return Ok(Json::obj().with("results", project.field("results").clone()).with(
                    "summary",
                    Json::Arr(vec![Json::obj()
                        .with("kind", Json::str("all"))
                        .with("status", Json::str("blocked"))
                        .with("issue", i.to_json())]),
                ))
            }
        };
        // Solo lo calculado ahora es vigente: los resultados anteriores se reemplazan por completo.
        let mut results = Json::obj();
        let mut jobs: Vec<(&str, Json)> = Vec::new();
        for r in gates::required_results(project) {
            let a = match r.kind {
                "hydraulic" => {
                    let (nid, sid) = r.key.as_deref().and_then(|k| k.split_once(':')).unwrap_or(("", ""));
                    let mode = Self::find(&project.at("systems.water.networks").cloned().unwrap_or(Json::arr()), "id", nid)
                        .and_then(|n| {
                            Self::find(&n.field("scenarios").clone(), "id", sid)
                                .map(|sc| sc.field("mode").as_str().unwrap_or("operating_point").to_string())
                        })
                        .unwrap_or_else(|| "operating_point".to_string());
                    Json::obj().with("network_id", Json::str(nid)).with("scenario_id", Json::str(sid)).with("mode", Json::str(mode))
                }
                "extinguishers" | "cause_effect" => Json::obj(),
                _ => Json::obj().with("system_id", Json::str(r.key.as_deref().unwrap_or(""))),
            };
            jobs.push((r.kind, a));
        }
        if project.at("budget.lines").map(|l| !l.items().is_empty()).unwrap_or(false) {
            jobs.push(("budget", Json::obj()));
        }
        let mut summary = Vec::new();
        for (kind, a) in jobs {
            let (path, out, note) = self.run_system(kind, project, &profile, &a)?;
            let env = self.envelope(&out, Some(&profile), note);
            // asignación por claves literales (las claves de hidráulica llevan ':' pero no '.')
            let mut cur = &mut results;
            for (i, seg) in path[1..].iter().enumerate() {
                let last = i + 1 == path.len() - 1;
                if let Json::Obj(m) = cur {
                    if last {
                        m.insert(seg.clone(), env.clone());
                        break;
                    }
                    cur = m.entry(seg.clone()).or_insert_with(Json::obj);
                } else {
                    break;
                }
            }
            summary.push(
                Json::obj()
                    .with("kind", Json::str(kind))
                    .with("key", Json::str(path[1..].join("/")))
                    .with("status", Json::str(out.status.as_str()))
                    .with("worst", Json::opt_str(out.worst().map(|w| w.as_str())))
                    .with("fingerprint", Json::str(out.fingerprint.as_str())),
            );
        }
        self.audit_with(
            "calc.run",
            project.at("meta.id").and_then(Json::as_str).unwrap_or(""),
            Json::obj().with("kind", Json::str("all")).with("results", Json::Arr(summary.clone())),
            &s,
            &self.profile_label(project),
        );
        Ok(Json::obj().with("results", results).with("summary", Json::Arr(summary)))
    }

    fn calc_cancel(&self, args: &Json) -> R {
        let j = a_str(args, "job_id")?;
        match lock(&self.jobs).get(j) {
            Some(f) => {
                f.store(true, Ordering::Relaxed);
                Ok(Json::obj().with("cancelled", Json::Bool(true)))
            }
            None => Ok(Json::obj().with("cancelled", Json::Bool(false))),
        }
    }

    /// Estimación de tamaño y memoria antes de calcular (contrato 8.2).
    fn calc_estimate(&self, args: &Json) -> R {
        let project = a_obj(args, "project")?;
        let nid = a_str(args, "network_id")?;
        let nets = project.at("systems.water.networks").cloned().unwrap_or(Json::arr());
        let net = Self::find(&nets, "id", nid).ok_or_else(|| ApiError::new("hyd.network_not_found").param("network", nid))?;
        let (n, m) = (net.field("nodes").items().len(), net.field("links").items().len());
        let devices = net
            .field("nodes")
            .items()
            .iter()
            .filter(|x| matches!(x.field("kind").as_str(), Some("sprinkler") | Some("nozzle") | Some("hydrant") | Some("hose_valve")))
            .count();
        let dense = n <= 300;
        let bytes = if dense { 8 * n * n + 200 * (n + m) } else { 200 * (n + m) + 8 * 12 * n };
        Ok(Json::obj()
            .with("nodes", Json::num(n as f64))
            .with("links", Json::num(m as f64))
            .with("devices", Json::num(devices as f64))
            .with("estimated_memory_bytes", Json::num(bytes as f64))
            .with("linear_solver", Json::str(if dense { "cholesky" } else { "pcg-jacobi" })))
    }

    // ================================================================= revisión y excepciones
    fn load_for_edit(&self, id: &str) -> Result<Json, ApiError> {
        Ok(lock(&self.store).load_project(id)?)
    }

    fn review_add(&self, args: &Json) -> R {
        let s = self.require("review.add")?;
        let id = a_str(args, "id")?;
        let decision = a_str(args, "decision")?;
        if !matches!(decision, "approve" | "reject" | "comment") {
            return Err(ApiError::new("data.bad_enum")
                .param("field", "decision")
                .param("value", decision)
                .param("allowed", "approve|reject|comment"));
        }
        let notes = args.field("notes").as_str().unwrap_or("").trim().to_string();
        if decision != "approve" && notes.is_empty() {
            return Err(ApiError::new("data.empty").param("field", "notes"));
        }
        let mut p = self.load_for_edit(id)?;
        let (gate, psha) = {
            let reg = self.registry.read().unwrap_or_else(|e| e.into_inner());
            (gates::evaluate(&p, &reg), self.profile_of(&p).map(|x| x.sha256).unwrap_or_default())
        };
        if decision == "approve" && gate.status == DocStatus::Draft {
            return Err(ApiError::new("review.cannot_approve_with_blocks").param("blocks", gate.issues.count(Severity::Block)));
        }
        let mut reviews = p.field("reviews").items().to_vec();
        let n = reviews.len() + 1;
        reviews.push(
            Json::obj()
                .with("id", Json::str(format!("RV-{}", n)))
                .with("date", Json::str(self.now()))
                .with("user", Json::str(s.user.as_str()))
                .with("role", Json::str(s.role.as_str()))
                .with("decision", Json::str(decision))
                .with("notes", Json::str(notes.as_str()))
                .with("revision", p.at("meta.revision").cloned().unwrap_or(Json::Null))
                .with("inputs_fingerprint", Json::str(project::inputs_fingerprint(&p)))
                .with("profile_sha256", Json::str(psha))
                .with("profile", Json::str(self.profile_label(&p))),
        );
        let old = p.clone();
        p.set_at("reviews", Json::Arr(reviews));
        permissions::authorize_save(s.role, &old, &p).map_err(ApiError::from)?;
        p.set_at("meta.updated_at", Json::str(self.now()));
        lock(&self.store).save_project(id, &p)?;
        self.audit_with(
            "review.add",
            id,
            Json::obj().with("decision", Json::str(decision)).with("notes", Json::str(notes)),
            &s,
            &self.profile_label(&p),
        );
        Ok(p)
    }

    fn exception_add(&self, args: &Json) -> R {
        let s = self.require("project.write")?;
        let id = a_str(args, "id")?;
        let code = a_str(args, "code")?;
        let reason = a_str(args, "reason")?.trim().to_string();
        if reason.is_empty() {
            return Err(ApiError::new("data.empty").param("field", "reason"));
        }
        let mut p = self.load_for_edit(id)?;
        let mut ex = p.field("exceptions").items().to_vec();
        let eid = format!("EX-{}", ex.len() + 1);
        ex.push(
            Json::obj()
                .with("id", Json::str(eid.as_str()))
                .with("code", Json::str(code))
                .with("object", Json::opt_str(args.field("object").as_str()))
                .with("reason", Json::str(reason.as_str()))
                .with("proposed_by", Json::str(s.user.as_str()))
                .with("proposed_at", Json::str(self.now()))
                .with("approved_by", Json::Null)
                .with("approved_role", Json::Null)
                .with("approved_at", Json::Null),
        );
        let old = p.clone();
        p.set_at("exceptions", Json::Arr(ex));
        permissions::authorize_save(s.role, &old, &p).map_err(ApiError::from)?;
        lock(&self.store).save_project(id, &p)?;
        self.audit_with(
            "exception.add",
            id,
            Json::obj().with("exception", Json::str(eid)).with("code", Json::str(code)),
            &s,
            &self.profile_label(&p),
        );
        Ok(p)
    }

    fn exception_approve(&self, args: &Json) -> R {
        let s = self.require("exception.approve")?;
        let id = a_str(args, "id")?;
        let eid = a_str(args, "exception_id")?;
        let mut p = self.load_for_edit(id)?;
        let mut ex = p.field("exceptions").items().to_vec();
        let Some(i) = ex.iter().position(|e| e.field("id").as_str() == Some(eid)) else {
            return Err(ApiError::new("api.exception_not_found").param("id", eid));
        };
        ex[i].set("approved_by", Json::str(s.user.as_str()));
        ex[i].set("approved_role", Json::str(s.role.as_str()));
        ex[i].set("approved_at", Json::str(self.now()));
        let old = p.clone();
        p.set_at("exceptions", Json::Arr(ex));
        permissions::authorize_save(s.role, &old, &p).map_err(ApiError::from)?;
        lock(&self.store).save_project(id, &p)?;
        self.audit_with("exception.approve", id, Json::obj().with("exception", Json::str(eid)), &s, &self.profile_label(&p));
        Ok(p)
    }

    // ================================================================= catálogo
    fn catalog_import(&self, args: &Json) -> R {
        self.require("catalog.write")?;
        let (items, issues) = budget::catalog_from_csv(a_str(args, "text")?);
        Ok(Json::obj().with("items", Json::Arr(items.iter().map(budget::CatalogItem::to_json).collect())).with("issues", issues.to_json()))
    }

    fn catalog_export(&self, args: &Json) -> R {
        let items: Vec<budget::CatalogItem> = args.field("items").items().iter().filter_map(budget::CatalogItem::from_json).collect();
        Ok(Json::str(budget::catalog_to_csv(&items)))
    }

    fn catalog_demo(&self) -> R {
        let (items, issues) = budget::catalog_from_csv(CATALOG_DEMO);
        Ok(Json::obj().with("items", Json::Arr(items.iter().map(budget::CatalogItem::to_json).collect())).with("issues", issues.to_json()))
    }

    // ================================================================= exportaciones
    fn export_run(&self, args: &Json) -> R {
        let s = self.require("export.generate")?;
        let project = a_obj(args, "project")?;
        let kind = a_str(args, "kind")?;
        let lang = args
            .field("lang")
            .as_str()
            .filter(|l| matches!(*l, "es" | "en"))
            .or_else(|| project.at("display.language").and_then(Json::as_str))
            .unwrap_or("es")
            .to_string();
        let profile = self.profile_of(project).ok();
        let gate = {
            let reg = self.registry.read().unwrap_or_else(|e| e.into_inner());
            gates::evaluate(project, &reg)
        };
        let lic = lock(&self.license).clone();
        let now = self.now();
        let digest = lock(&self.store).read_audit().ok().and_then(|a| a.last().map(|e| e.hash.clone()));
        let ctx = ExportContext {
            project,
            profile: profile.as_ref(),
            gate: &gate,
            lang: &lang,
            user: &s.user,
            role: s.role.as_str(),
            license: &lic,
            generated_at: &now,
            audit_digest: digest.as_deref(),
        };
        let file = match kind {
            "pdf" => export::export_pdf(&ctx),
            "xlsx" => export::export_xlsx(&ctx),
            "csv" => export::export_csv_zip(&ctx),
            "json" => export::export_json(&ctx),
            "ifc" => export::export_ifc(&ctx),
            "dxf" => export::export_dxf(&ctx, args.field("level").as_str()),
            _ => {
                return Err(ApiError::new("data.bad_enum")
                    .param("field", "kind")
                    .param("value", kind)
                    .param("allowed", "pdf|xlsx|csv|dxf|json|ifc"))
            }
        };
        let pid = project.at("meta.id").and_then(Json::as_str).unwrap_or("");
        self.audit_with(
            "export.generate",
            pid,
            Json::obj()
                .with("kind", Json::str(kind))
                .with("file", Json::str(file.filename.as_str()))
                .with("sha256", Json::str(file.sha256.as_str()))
                .with("bytes", Json::num(file.bytes.len() as f64))
                .with("document_status", Json::str(gate.status.as_str()))
                .with("watermark", Json::Bool(lic.watermark)),
            &s,
            &self.profile_label(project),
        );
        let manifest = export::manifest(&ctx, &[&file]);
        Ok(Json::obj()
            .with("filename", Json::str(file.filename.as_str()))
            .with("mime", Json::str(file.mime))
            .with("size", Json::num(file.bytes.len() as f64))
            .with("sha256", Json::str(file.sha256.as_str()))
            .with("document_status", Json::str(gate.status.as_str()))
            .with("notes", Json::Arr(file.notes.clone()))
            .with("manifest", manifest)
            .with("base64", Json::str(base64_encode(&file.bytes))))
    }

    // ================================================================= auditoría
    fn audit_list(&self, args: &Json) -> R {
        self.require("audit.read")?;
        let entries = lock(&self.store).read_audit()?;
        let limit = args.field("limit").as_f64().unwrap_or(500.0).clamp(1.0, 10000.0) as usize;
        let project = args.field("project").as_str();
        let filtered: Vec<&Entry> = entries.iter().filter(|e| project.map(|p| e.project == p).unwrap_or(true)).collect();
        let start = filtered.len().saturating_sub(limit);
        Ok(Json::obj()
            .with("total", Json::num(filtered.len() as f64))
            .with("entries", Json::Arr(filtered[start..].iter().map(|e| e.to_json()).collect())))
    }

    fn audit_verify(&self) -> R {
        self.require("audit.read")?;
        let entries = lock(&self.store).read_audit();
        match entries {
            Err(e) => Ok(Json::obj().with("ok", Json::Bool(false)).with("error", ApiError::from(e).to_json())),
            Ok(es) => Ok(match audit::verify_chain(&es) {
                Ok(()) => Json::obj()
                    .with("ok", Json::Bool(true))
                    .with("entries", Json::num(es.len() as f64))
                    .with("head", Json::opt_str(es.last().map(|e| e.hash.as_str()))),
                Err(e) => {
                    let (kind, seq) = match e {
                        audit::ChainError::BadSequence { seq } => ("sequence", seq),
                        audit::ChainError::BrokenLink { seq } => ("link", seq),
                        audit::ChainError::BadHash { seq } => ("hash", seq),
                    };
                    Json::obj().with("ok", Json::Bool(false)).with("kind", Json::str(kind)).with("seq", Json::num(seq as f64))
                }
            }),
        }
    }

    fn audit_export(&self, args: &Json) -> R {
        let s = self.require("audit.export")?;
        let entries = lock(&self.store).read_audit()?;
        let fmt = args.field("format").as_str().unwrap_or("json");
        let (bytes, mime, name): (Vec<u8>, &str, &str) = match fmt {
            "csv" => {
                let mut rows: Vec<Vec<String>> = vec![[
                    "seq",
                    "ts_utc_no_certificado",
                    "user",
                    "role",
                    "action",
                    "project",
                    "profile",
                    "app_version",
                    "detail",
                    "prev_hash",
                    "hash",
                ]
                .iter()
                .map(|s| s.to_string())
                .collect()];
                for e in &entries {
                    rows.push(vec![
                        e.seq.to_string(),
                        e.ts.clone(),
                        e.user.clone(),
                        e.role.clone(),
                        e.action.clone(),
                        e.project.clone(),
                        e.profile.clone(),
                        e.app_version.clone(),
                        e.detail.to_compact(),
                        e.prev_hash.clone(),
                        e.hash.clone(),
                    ]);
                }
                (format!("\u{feff}{}", crate::csv::write(&rows, ',', true)).into_bytes(), "text/csv", "auditoria.csv")
            }
            _ => {
                let ok = audit::verify_chain(&entries).is_ok();
                (
                    Json::obj()
                        .with("format", Json::str("vigix.audit/1"))
                        .with("chain_valid", Json::Bool(ok))
                        .with("entries", Json::Arr(entries.iter().map(|e| e.to_json()).collect()))
                        .to_pretty()
                        .into_bytes(),
                    "application/json",
                    "auditoria.json",
                )
            }
        };
        self.audit_with(
            "audit.export",
            "",
            Json::obj().with("format", Json::str(fmt)).with("sha256", Json::str(sha256_hex(&bytes))),
            &s,
            "",
        );
        Ok(Json::obj()
            .with("filename", Json::str(name))
            .with("mime", Json::str(mime))
            .with("base64", Json::str(base64_encode(&bytes)))
            .with("sha256", Json::str(sha256_hex(&bytes))))
    }

    // ================================================================= copias y licencia
    fn backup_create(&self, args: &Json) -> R {
        let s = self.require("backup.create")?;
        let label = match args.field("label").as_str().filter(|l| !l.trim().is_empty()) {
            Some(l) => l.to_string(),
            None => format!("auto-{}", self.now().replace([':', '-'], "").replace('T', "-").replace('Z', "")),
        };
        let name = lock(&self.store).create_backup(&label)?;
        self.audit_with("backup.create", "", Json::obj().with("name", Json::str(name.as_str())), &s, "");
        Ok(Json::str(name))
    }

    fn backup_restore(&self, args: &Json) -> R {
        let s = self.require("backup.restore")?;
        let name = a_str(args, "name")?;
        if args.field("confirm").as_bool() != Some(true) {
            return Err(ApiError::new("api.confirmation_required").param("action", "backup.restore"));
        }
        let n = lock(&self.store).restore_backup(name)?;
        self.audit_with("backup.restore", "", Json::obj().with("name", Json::str(name)).with("projects", Json::num(n as f64)), &s, "");
        Ok(Json::obj().with("projects_restored", Json::num(n as f64)))
    }

    fn license_activate(&self, args: &Json) -> R {
        let s = self.require("license.activate")?;
        let text = a_str(args, "text")?;
        let st = license::check(text, self.verifier.as_ref(), self.machine_id.as_deref());
        if st.edition == license::Edition::Pro {
            lock(&self.store).set_setting("license", &Json::obj().with("text", Json::str(text)))?;
        }
        *lock(&self.license) = st.clone();
        self.audit_with(
            "license.activate",
            "",
            Json::obj().with("edition", Json::str(st.edition.as_str())).with("reason", Json::opt_str(st.reason.as_deref())),
            &s,
            "",
        );
        Ok(st.to_json())
    }
}

/// Utilidad para clientes: decodifica el campo `base64` de una respuesta de exportación.
pub fn decode_export(result: &Json) -> Option<Vec<u8>> {
    base64_decode(result.field("base64").as_str()?)
}

/// Incidencias de un resultado como lista tipada (útil en pruebas y en el CLI).
pub fn issues_of(env: &Json) -> IssueList {
    IssueList(env.field("issues").items().iter().filter_map(Issue::from_json).collect())
}
