//! Puerto de persistencia (contrato 4.4 y 8.1) con dos implementaciones sin dependencias:
//! `MemStore` (pruebas) y `FsStore` (archivos JSON + auditoría en JSON Lines).
//!
//! La aplicación de escritorio usa además un adaptador **SQLite** (en `src-tauri`) que implementa
//! este mismo rasgo; el contrato de comportamiento (guardado atómico, auditoría append-only,
//! identificadores seguros, copias con manifiesto) se verifica aquí con pruebas.
//!
//! Seguridad: los identificadores de proyecto solo admiten `[A-Za-z0-9._-]` (máx. 64) para impedir
//! rutas maliciosas (`..`, separadores, nombres reservados de Windows).

use crate::audit::Entry;
use crate::hash::sha256_hex;
use crate::json::Json;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum StoreError {
    NotFound(String),
    Invalid(String),
    Io(String),
    Corrupt(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::NotFound(s) => write!(f, "no encontrado: {}", s),
            StoreError::Invalid(s) => write!(f, "inválido: {}", s),
            StoreError::Io(s) => write!(f, "error de E/S: {}", s),
            StoreError::Corrupt(s) => write!(f, "datos corruptos: {}", s),
        }
    }
}
impl std::error::Error for StoreError {}

impl StoreError {
    pub fn code(&self) -> &'static str {
        match self {
            StoreError::NotFound(_) => "store.not_found",
            StoreError::Invalid(_) => "store.invalid",
            StoreError::Io(_) => "store.io",
            StoreError::Corrupt(_) => "store.corrupt",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectMeta {
    pub id: String,
    pub name: String,
    pub updated_at: String,
    pub revision: u64,
}

impl ProjectMeta {
    pub fn to_json(&self) -> Json {
        Json::obj()
            .with("id", Json::str(self.id.as_str()))
            .with("name", Json::str(self.name.as_str()))
            .with("updated_at", Json::str(self.updated_at.as_str()))
            .with("revision", Json::num(self.revision as f64))
    }
    fn of(p: &Json) -> ProjectMeta {
        ProjectMeta {
            id: p.at("meta.id").and_then(Json::as_str).unwrap_or("").to_string(),
            name: p.at("meta.name").and_then(Json::as_str).unwrap_or("").to_string(),
            updated_at: p.at("meta.updated_at").and_then(Json::as_str).unwrap_or("").to_string(),
            revision: p.at("meta.revision").and_then(Json::as_f64).unwrap_or(0.0) as u64,
        }
    }
}

/// Identificador seguro (sin rutas): `[A-Za-z0-9][A-Za-z0-9._-]{0,63}`, sin `..` y sin nombres
/// reservados de Windows.
pub fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    if b.is_empty() || b.len() > 64 || !b[0].is_ascii_alphanumeric() {
        return false;
    }
    if !b.iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-')) || id.contains("..") || id.ends_with('.') {
        return false;
    }
    let stem = id.split('.').next().unwrap_or("").to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !(stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit())
}

pub trait Store: Send {
    fn kind(&self) -> &'static str;
    fn list_projects(&self) -> Result<Vec<ProjectMeta>, StoreError>;
    fn load_project(&self, id: &str) -> Result<Json, StoreError>;
    /// Guardado atómico: o queda la versión anterior o la nueva, nunca un archivo a medias.
    fn save_project(&mut self, id: &str, project: &Json) -> Result<(), StoreError>;
    /// Borrado lógico (papelera): el contenido se conserva.
    fn delete_project(&mut self, id: &str) -> Result<(), StoreError>;
    fn append_audit(&mut self, e: &Entry) -> Result<(), StoreError>;
    fn read_audit(&self) -> Result<Vec<Entry>, StoreError>;
    fn get_setting(&self, key: &str) -> Result<Option<Json>, StoreError>;
    fn set_setting(&mut self, key: &str, value: &Json) -> Result<(), StoreError>;
    fn list_profiles(&self) -> Result<Vec<Json>, StoreError>;
    fn save_profile(&mut self, code: &str, version: &str, pkg: &Json) -> Result<(), StoreError>;
    fn create_backup(&mut self, label: &str) -> Result<String, StoreError>;
    fn list_backups(&self) -> Result<Vec<String>, StoreError>;
    /// Restaura (superpone) proyectos, ajustes y perfiles; **nunca** toca la auditoría.
    fn restore_backup(&mut self, name: &str) -> Result<usize, StoreError>;
    fn last_audit(&self) -> Result<Option<Entry>, StoreError> {
        Ok(self.read_audit()?.pop())
    }
}

// ============================================================================ memoria
/// Copia en memoria: (proyectos, ajustes, perfiles).
type Snapshot = (BTreeMap<String, String>, BTreeMap<String, String>, BTreeMap<String, String>);

#[derive(Default)]
pub struct MemStore {
    projects: BTreeMap<String, String>,
    trash: Vec<(String, String)>,
    audit: Vec<Entry>,
    settings: BTreeMap<String, String>,
    profiles: BTreeMap<String, String>,
    backups: BTreeMap<String, Snapshot>,
}

impl MemStore {
    pub fn new() -> MemStore {
        MemStore::default()
    }
}

impl Store for MemStore {
    fn kind(&self) -> &'static str {
        "memory"
    }
    fn list_projects(&self) -> Result<Vec<ProjectMeta>, StoreError> {
        let mut v = Vec::new();
        for t in self.projects.values() {
            let j = Json::parse(t).map_err(|e| StoreError::Corrupt(e.to_string()))?;
            v.push(ProjectMeta::of(&j));
        }
        Ok(v)
    }
    fn load_project(&self, id: &str) -> Result<Json, StoreError> {
        if !valid_id(id) {
            return Err(StoreError::Invalid(id.to_string()));
        }
        let t = self.projects.get(id).ok_or_else(|| StoreError::NotFound(id.to_string()))?;
        Json::parse(t).map_err(|e| StoreError::Corrupt(e.to_string()))
    }
    fn save_project(&mut self, id: &str, project: &Json) -> Result<(), StoreError> {
        if !valid_id(id) {
            return Err(StoreError::Invalid(id.to_string()));
        }
        self.projects.insert(id.to_string(), project.to_pretty());
        Ok(())
    }
    fn delete_project(&mut self, id: &str) -> Result<(), StoreError> {
        if !valid_id(id) {
            return Err(StoreError::Invalid(id.to_string()));
        }
        let t = self.projects.remove(id).ok_or_else(|| StoreError::NotFound(id.to_string()))?;
        self.trash.push((id.to_string(), t));
        Ok(())
    }
    fn append_audit(&mut self, e: &Entry) -> Result<(), StoreError> {
        self.audit.push(e.clone());
        Ok(())
    }
    fn read_audit(&self) -> Result<Vec<Entry>, StoreError> {
        Ok(self.audit.clone())
    }
    fn get_setting(&self, key: &str) -> Result<Option<Json>, StoreError> {
        match self.settings.get(key) {
            Some(t) => Ok(Some(Json::parse(t).map_err(|e| StoreError::Corrupt(e.to_string()))?)),
            None => Ok(None),
        }
    }
    fn set_setting(&mut self, key: &str, value: &Json) -> Result<(), StoreError> {
        self.settings.insert(key.to_string(), value.to_compact());
        Ok(())
    }
    fn list_profiles(&self) -> Result<Vec<Json>, StoreError> {
        self.profiles.values().map(|t| Json::parse(t).map_err(|e| StoreError::Corrupt(e.to_string()))).collect()
    }
    fn save_profile(&mut self, code: &str, version: &str, pkg: &Json) -> Result<(), StoreError> {
        if !valid_id(code) || !valid_id(version) {
            return Err(StoreError::Invalid(format!("{}@{}", code, version)));
        }
        self.profiles.insert(format!("{}@{}", code, version), pkg.to_pretty());
        Ok(())
    }
    fn create_backup(&mut self, label: &str) -> Result<String, StoreError> {
        if !valid_id(label) {
            return Err(StoreError::Invalid(label.to_string()));
        }
        self.backups.insert(label.to_string(), (self.projects.clone(), self.settings.clone(), self.profiles.clone()));
        Ok(label.to_string())
    }
    fn list_backups(&self) -> Result<Vec<String>, StoreError> {
        Ok(self.backups.keys().cloned().collect())
    }
    fn restore_backup(&mut self, name: &str) -> Result<usize, StoreError> {
        let (p, s, pr) = self.backups.get(name).cloned().ok_or_else(|| StoreError::NotFound(name.to_string()))?;
        let n = p.len();
        self.projects.extend(p);
        self.settings.extend(s);
        self.profiles.extend(pr);
        Ok(n)
    }
}

// ============================================================================ archivos
pub struct FsStore {
    root: PathBuf,
}

fn io<E: std::fmt::Display>(e: E) -> StoreError {
    StoreError::Io(e.to_string())
}

/// Escritura atómica: archivo temporal en el mismo directorio + `sync_all` + renombrado.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let dir = path.parent().ok_or_else(|| StoreError::Invalid("ruta sin directorio".into()))?;
    fs::create_dir_all(dir).map_err(io)?;
    let tmp = dir.join(format!(".{}.tmp-{}", path.file_name().and_then(|n| n.to_str()).unwrap_or("f"), std::process::id()));
    {
        let mut f = fs::File::create(&tmp).map_err(io)?;
        f.write_all(bytes).map_err(io)?;
        f.sync_all().map_err(io)?;
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        io(e)
    })
}

impl FsStore {
    pub fn open(root: &Path) -> Result<FsStore, StoreError> {
        for d in ["projects", "trash", "profiles", "backups"] {
            fs::create_dir_all(root.join(d)).map_err(io)?;
        }
        Ok(FsStore { root: root.to_path_buf() })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn project_path(&self, id: &str) -> Result<PathBuf, StoreError> {
        if !valid_id(id) {
            return Err(StoreError::Invalid(id.to_string()));
        }
        Ok(self.root.join("projects").join(format!("{}.vigix.json", id)))
    }

    fn read_json(path: &Path) -> Result<Json, StoreError> {
        let t = fs::read_to_string(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                StoreError::NotFound(path.display().to_string())
            } else {
                io(e)
            }
        })?;
        Json::parse(&t).map_err(|e| StoreError::Corrupt(format!("{}: {}", path.display(), e)))
    }

    fn files_in(dir: &Path, suffix: &str) -> Result<Vec<PathBuf>, StoreError> {
        let mut v: Vec<PathBuf> = fs::read_dir(dir)
            .map_err(io)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.is_file() && p.file_name().and_then(|n| n.to_str()).map(|n| n.ends_with(suffix) && !n.starts_with('.')).unwrap_or(false)
            })
            .collect();
        v.sort();
        Ok(v)
    }
}

impl Store for FsStore {
    fn kind(&self) -> &'static str {
        "filesystem"
    }

    fn list_projects(&self) -> Result<Vec<ProjectMeta>, StoreError> {
        let mut v = Vec::new();
        for p in Self::files_in(&self.root.join("projects"), ".vigix.json")? {
            // un proyecto ilegible no impide listar los demás: se omite (se informa al abrirlo)
            if let Ok(j) = Self::read_json(&p) {
                v.push(ProjectMeta::of(&j));
            }
        }
        Ok(v)
    }

    fn load_project(&self, id: &str) -> Result<Json, StoreError> {
        Self::read_json(&self.project_path(id)?)
    }

    fn save_project(&mut self, id: &str, project: &Json) -> Result<(), StoreError> {
        let path = self.project_path(id)?;
        write_atomic(&path, project.to_pretty().as_bytes())
    }

    fn delete_project(&mut self, id: &str) -> Result<(), StoreError> {
        let path = self.project_path(id)?;
        if !path.exists() {
            return Err(StoreError::NotFound(id.to_string()));
        }
        let stamp = self.read_audit().ok().and_then(|a| a.last().map(|e| e.seq)).unwrap_or(0);
        let dest = self.root.join("trash").join(format!("{}.{}.vigix.json", id, stamp));
        fs::rename(&path, &dest).or_else(|_| fs::copy(&path, &dest).and_then(|_| fs::remove_file(&path))).map_err(io)
    }

    fn append_audit(&mut self, e: &Entry) -> Result<(), StoreError> {
        let path = self.root.join("audit.jsonl");
        let mut f = fs::OpenOptions::new().create(true).append(true).open(&path).map_err(io)?;
        let mut line = e.to_json().to_compact();
        line.push('\n');
        f.write_all(line.as_bytes()).map_err(io)?;
        f.sync_data().map_err(io)
    }

    fn read_audit(&self) -> Result<Vec<Entry>, StoreError> {
        let path = self.root.join("audit.jsonl");
        let t = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(io(e)),
        };
        let mut v = Vec::new();
        for (i, line) in t.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let j = Json::parse(line).map_err(|e| StoreError::Corrupt(format!("auditoría línea {}: {}", i + 1, e)))?;
            v.push(Entry::from_json(&j).ok_or_else(|| StoreError::Corrupt(format!("auditoría línea {}: entrada inválida", i + 1)))?);
        }
        Ok(v)
    }

    fn get_setting(&self, key: &str) -> Result<Option<Json>, StoreError> {
        let path = self.root.join("settings.json");
        if !path.exists() {
            return Ok(None);
        }
        Ok(Self::read_json(&path)?.get(key).cloned())
    }

    fn set_setting(&mut self, key: &str, value: &Json) -> Result<(), StoreError> {
        let path = self.root.join("settings.json");
        let mut cur = if path.exists() { Self::read_json(&path)? } else { Json::obj() };
        cur.set(key, value.clone());
        write_atomic(&path, cur.to_pretty().as_bytes())
    }

    fn list_profiles(&self) -> Result<Vec<Json>, StoreError> {
        Self::files_in(&self.root.join("profiles"), ".json")?.iter().map(|p| Self::read_json(p)).collect()
    }

    fn save_profile(&mut self, code: &str, version: &str, pkg: &Json) -> Result<(), StoreError> {
        if !valid_id(code) || !valid_id(version) {
            return Err(StoreError::Invalid(format!("{}@{}", code, version)));
        }
        write_atomic(&self.root.join("profiles").join(format!("{}@{}.json", code, version)), pkg.to_pretty().as_bytes())
    }

    fn create_backup(&mut self, label: &str) -> Result<String, StoreError> {
        if !valid_id(label) {
            return Err(StoreError::Invalid(label.to_string()));
        }
        let dir = self.root.join("backups").join(label);
        if dir.exists() {
            return Err(StoreError::Invalid(format!("la copia '{}' ya existe", label)));
        }
        let mut manifest = Json::obj();
        for (sub, suffix) in [("projects", ".vigix.json"), ("profiles", ".json")] {
            for p in Self::files_in(&self.root.join(sub), suffix)? {
                let bytes = fs::read(&p).map_err(io)?;
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                write_atomic(&dir.join(sub).join(&name), &bytes)?;
                manifest.set(format!("{}/{}", sub, name), Json::str(sha256_hex(&bytes)));
            }
        }
        for f in ["settings.json", "audit.jsonl"] {
            let p = self.root.join(f);
            if p.exists() {
                let bytes = fs::read(&p).map_err(io)?;
                write_atomic(&dir.join(f), &bytes)?;
                manifest.set(f, Json::str(sha256_hex(&bytes)));
            }
        }
        write_atomic(
            &dir.join("MANIFEST.json"),
            Json::obj().with("schema", Json::str("vigix.backup/1")).with("files", manifest).to_pretty().as_bytes(),
        )?;
        Ok(label.to_string())
    }

    fn list_backups(&self) -> Result<Vec<String>, StoreError> {
        let mut v: Vec<String> = fs::read_dir(self.root.join("backups"))
            .map_err(io)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().join("MANIFEST.json").exists())
            .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
            .collect();
        v.sort();
        Ok(v)
    }

    fn restore_backup(&mut self, name: &str) -> Result<usize, StoreError> {
        if !valid_id(name) {
            return Err(StoreError::Invalid(name.to_string()));
        }
        let dir = self.root.join("backups").join(name);
        let manifest = Self::read_json(&dir.join("MANIFEST.json"))?;
        let files = manifest.field("files").as_obj().ok_or_else(|| StoreError::Corrupt("manifiesto sin 'files'".into()))?;
        // 1) verificar TODO antes de tocar nada
        for (rel, h) in files {
            if rel.contains("..") {
                return Err(StoreError::Corrupt(format!("ruta sospechosa en el manifiesto: {}", rel)));
            }
            let bytes = fs::read(dir.join(rel)).map_err(|_| StoreError::Corrupt(format!("falta {}", rel)))?;
            if Some(sha256_hex(&bytes).as_str()) != h.as_str() {
                return Err(StoreError::Corrupt(format!("huella no coincide: {}", rel)));
            }
        }
        // 2) copia de seguridad previa del estado actual
        let pre = format!("pre-restauracion-{}", self.read_audit().map(|a| a.len()).unwrap_or(0));
        if !self.root.join("backups").join(&pre).exists() {
            self.create_backup(&pre)?;
        }
        // 3) superponer proyectos, perfiles y ajustes (jamás la auditoría)
        let mut n = 0;
        for rel in files.keys() {
            if rel == "audit.jsonl" {
                continue;
            }
            let bytes = fs::read(dir.join(rel)).map_err(io)?;
            write_atomic(&self.root.join(rel), &bytes)?;
            if rel.starts_with("projects/") {
                n += 1;
            }
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{next_entry, verify_chain};
    use crate::project::new_project;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "vigix-test-{}-{}-{}",
            name,
            std::process::id(),
            &crate::hash::sha256_hex(name.as_bytes())[..8]
        ));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn proj(id: &str, name: &str) -> Json {
        new_project(id, name, "es", "SI", "2026-10-28T00:00:00Z")
    }

    #[test]
    fn identificadores_seguros() {
        for ok in ["P1", "proyecto-2026", "a.b_c", "X"] {
            assert!(valid_id(ok), "{}", ok);
        }
        for bad in [
            "",
            "..",
            "../x",
            "a/b",
            "a\\b",
            ".oculto",
            "a..b",
            "CON",
            "con.txt",
            "NUL",
            "COM1",
            "lpt9",
            "a b",
            "ñandú",
            &"x".repeat(65),
            "fin.",
            "C:evil",
        ] {
            assert!(!valid_id(bad), "{:?}", bad);
        }
    }

    fn contrato_comun(s: &mut dyn Store) {
        assert!(s.list_projects().unwrap().is_empty());
        s.save_project("P1", &proj("P1", "Uno")).unwrap();
        s.save_project("P2", &proj("P2", "Dos")).unwrap();
        let l = s.list_projects().unwrap();
        assert_eq!(l.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), vec!["Uno", "Dos"]);
        assert_eq!(s.load_project("P1").unwrap().at("meta.name").and_then(Json::as_str), Some("Uno"));
        assert_eq!(s.load_project("P9").unwrap_err().code(), "store.not_found");
        assert_eq!(s.load_project("../etc/passwd").unwrap_err().code(), "store.invalid");
        assert_eq!(s.save_project("..", &proj("x", "x")).unwrap_err().code(), "store.invalid");
        // sobrescribir
        s.save_project("P1", &proj("P1", "Uno v2")).unwrap();
        assert_eq!(s.load_project("P1").unwrap().at("meta.name").and_then(Json::as_str), Some("Uno v2"));
        // ajustes
        assert_eq!(s.get_setting("lang").unwrap(), None);
        s.set_setting("lang", &Json::str("en")).unwrap();
        s.set_setting("n", &Json::num(3.0)).unwrap();
        assert_eq!(s.get_setting("lang").unwrap(), Some(Json::str("en")));
        assert_eq!(s.get_setting("n").unwrap(), Some(Json::num(3.0)));
        // perfiles
        s.save_profile("DEMO", "0.1.0", &Json::obj().with("a", Json::num(1.0))).unwrap();
        assert_eq!(s.list_profiles().unwrap().len(), 1);
        assert!(s.save_profile("../x", "1", &Json::obj()).is_err());
        // auditoría append-only encadenada
        let e1 = next_entry(None, "2026-10-28T00:00:00Z", "ana", "designer", "0.1.0", "", "project.create", "P1", Json::Null);
        s.append_audit(&e1).unwrap();
        let e2 = next_entry(Some(&e1), "2026-10-28T00:01:00Z", "ana", "designer", "0.1.0", "", "project.save", "P1", Json::Null);
        s.append_audit(&e2).unwrap();
        let all = s.read_audit().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(verify_chain(&all), Ok(()));
        assert_eq!(s.last_audit().unwrap().unwrap().seq, 2);
        // copias
        let b = s.create_backup("c1").unwrap();
        s.delete_project("P1").unwrap();
        assert_eq!(s.load_project("P1").unwrap_err().code(), "store.not_found");
        assert_eq!(s.list_backups().unwrap(), vec![b.clone()]);
        let n = s.restore_backup(&b).unwrap();
        assert_eq!(n, 2);
        assert_eq!(s.load_project("P1").unwrap().at("meta.name").and_then(Json::as_str), Some("Uno v2"));
        // la restauración NO reescribe la auditoría
        assert_eq!(s.read_audit().unwrap().len(), 2);
        assert_eq!(s.restore_backup("no-existe").unwrap_err().code(), "store.not_found");
    }

    #[test]
    fn memstore_cumple_el_contrato() {
        contrato_comun(&mut MemStore::new());
    }

    #[test]
    fn fsstore_cumple_el_contrato_y_persiste() {
        let d = tmpdir("fs1");
        {
            let mut s = FsStore::open(&d).unwrap();
            contrato_comun(&mut s);
        }
        // reabrir: todo sigue ahí
        let s = FsStore::open(&d).unwrap();
        assert_eq!(s.list_projects().unwrap().len(), 2);
        assert_eq!(s.read_audit().unwrap().len(), 2);
        assert!(d.join("trash").read_dir().unwrap().count() == 1, "el borrado es lógico (papelera)");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn guardado_atomico_sin_residuos_y_archivo_corrupto_se_informa() {
        let d = tmpdir("fs2");
        let mut s = FsStore::open(&d).unwrap();
        s.save_project("P1", &proj("P1", "A")).unwrap();
        let leftovers: Vec<_> = fs::read_dir(d.join("projects"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "no deben quedar temporales");
        // corromper el archivo
        fs::write(d.join("projects").join("P1.vigix.json"), "{ esto no es json").unwrap();
        assert_eq!(s.load_project("P1").unwrap_err().code(), "store.corrupt");
        // un proyecto corrupto no impide listar los demás
        s.save_project("P2", &proj("P2", "B")).unwrap();
        assert_eq!(s.list_projects().unwrap().len(), 1);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn restauracion_verifica_el_manifiesto_y_crea_copia_previa() {
        let d = tmpdir("fs3");
        let mut s = FsStore::open(&d).unwrap();
        s.save_project("P1", &proj("P1", "Original")).unwrap();
        s.create_backup("b1").unwrap();
        s.save_project("P1", &proj("P1", "Modificado")).unwrap();
        // alterar un archivo de la copia: la restauración debe negarse y no tocar nada
        let victim = d.join("backups/b1/projects/P1.vigix.json");
        let mut t = fs::read_to_string(&victim).unwrap();
        t = t.replace("Original", "Manipulado");
        fs::write(&victim, t).unwrap();
        let e = s.restore_backup("b1").unwrap_err();
        assert_eq!(e.code(), "store.corrupt");
        assert_eq!(s.load_project("P1").unwrap().at("meta.name").and_then(Json::as_str), Some("Modificado"));
        // copia íntegra: restaura y deja una copia previa
        s.save_project("P2", &proj("P2", "Otro")).unwrap();
        s.create_backup("b2").unwrap();
        s.save_project("P2", &proj("P2", "Otro cambiado")).unwrap();
        assert_eq!(s.restore_backup("b2").unwrap(), 2);
        assert_eq!(s.load_project("P2").unwrap().at("meta.name").and_then(Json::as_str), Some("Otro"));
        assert!(s.list_backups().unwrap().iter().any(|b| b.starts_with("pre-restauracion-")));
        assert!(s.create_backup("b2").is_err(), "no se sobrescriben copias existentes");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn auditoria_manipulada_en_disco_se_detecta() {
        let d = tmpdir("fs4");
        let mut s = FsStore::open(&d).unwrap();
        let mut prev: Option<Entry> = None;
        for i in 0..4 {
            let e = next_entry(
                prev.as_ref(),
                "2026-10-28T00:00:00Z",
                "ana",
                "designer",
                "0.1.0",
                "",
                "input.changed",
                "P1",
                Json::obj().with("i", Json::num(i as f64)),
            );
            s.append_audit(&e).unwrap();
            prev = Some(e);
        }
        let log = d.join("audit.jsonl");
        let t = fs::read_to_string(&log).unwrap().replacen("\"i\":2", "\"i\":99", 1);
        fs::write(&log, t).unwrap();
        let all = s.read_audit().unwrap();
        assert!(verify_chain(&all).is_err());
        // línea basura => corrupción explícita
        fs::write(&log, "no json\n").unwrap();
        assert_eq!(s.read_audit().unwrap_err().code(), "store.corrupt");
        let _ = fs::remove_dir_all(&d);
    }
}
