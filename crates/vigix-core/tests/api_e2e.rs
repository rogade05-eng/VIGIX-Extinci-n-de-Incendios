//! Pruebas de extremo a extremo de la capa de aplicación (API) con el proyecto de ejemplo y el
//! perfil de DEMOSTRACIÓN. Escribe muestras de exportación en `target/tmp/samples` para que los
//! validadores independientes (Python: openpyxl, ezdxf, pypdf, ifcopenshell) las abran.

use std::fs;
use std::path::PathBuf;
use vigix_core::api::{decode_export, App};
use vigix_core::hash::{base64_encode, sha256};
use vigix_core::json::Json;
use vigix_core::license::{payload_bytes, TestOnlyVerifier, PRODUCT_ID};
use vigix_core::store::MemStore;
use vigix_core::timeutil::FixedClock;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn app() -> App {
    let a =
        App::new(Box::new(MemStore::new()), Box::new(FixedClock(1_793_145_600)), Box::new(TestOnlyVerifier), Some("MAQUINA-PRUEBA".into()));
    let loaded = a.load_bundled_profiles(&root().join("profiles"));
    assert!(loaded.iter().any(|p| p == "DEMO-NO-NORMATIVO@0.1.0"), "{:?}", loaded);
    a
}

fn call(a: &App, cmd: &str, args: Json) -> Json {
    let r = Json::obj().with("cmd", Json::str(cmd)).with("args", args);
    let out = Json::parse(&a.handle(&r.to_compact())).unwrap();
    assert_eq!(out.field("ok").as_bool(), Some(true), "{} falló: {}", cmd, out.field("error").to_compact());
    out.field("result").clone()
}

fn call_err(a: &App, cmd: &str, args: Json) -> Json {
    let r = Json::obj().with("cmd", Json::str(cmd)).with("args", args);
    let out = Json::parse(&a.handle(&r.to_compact())).unwrap();
    assert_eq!(out.field("ok").as_bool(), Some(false), "{} debía fallar", cmd);
    out.field("error").clone()
}

fn example() -> Json {
    Json::parse(&fs::read_to_string(root().join("examples/ejemplo_edificio_demo.json")).unwrap()).unwrap()
}

/// Importa el ejemplo, lo guarda y calcula todo; devuelve el proyecto con resultados y guardado.
fn calculated(a: &App) -> Json {
    let imp = call(a, "project.import", Json::obj().with("text", Json::str(example().to_compact())));
    let mut p = imp.field("project").clone();
    let saved = call(a, "project.save", Json::obj().with("project", p.clone()));
    p = saved.field("project").clone();
    let r = call(a, "calc.all", Json::obj().with("project", p.clone()));
    p.set("results", r.field("results").clone());
    call(a, "project.save", Json::obj().with("project", p)).field("project").clone()
}

fn status_of(env: &Json) -> &str {
    env.field("status").as_str().unwrap_or("?")
}

fn codes(env: &Json) -> Vec<String> {
    env.field("issues").items().iter().filter_map(|i| i.field("code").as_str().map(|s| s.to_string())).collect()
}

#[test]
fn flujo_completo_con_el_proyecto_de_ejemplo() {
    let a = app();
    let info = call(&a, "app.info", Json::obj());
    assert_eq!(info.field("network").as_str().map(|s| s.contains("offline")), Some(true));
    assert_eq!(info.at("session.role").and_then(Json::as_str), Some("admin"));
    let p = calculated(&a);
    let res = p.field("results");
    // todos los cálculos se ejecutaron y ninguno quedó bloqueado
    for path in ["extinguishers", "cause_effect", "budget"] {
        assert_eq!(status_of(res.field(path)), "ok", "{}: {:?}", path, codes(res.field(path)));
    }
    for (kind, key) in [
        ("hydraulic", "NET1:SC1"),
        ("hydraulic", "NET1:SC2"),
        ("foam", "FM1"),
        ("gas", "G1"),
        ("dry_chemical", "DC1"),
        ("wet_chemical", "K1"),
        ("aerosol", "AE1"),
    ] {
        let env = res.field(kind).field(key);
        assert_eq!(status_of(env), "ok", "{}:{} => {:?}", kind, key, codes(env));
        assert_eq!(env.at("profile.demo").and_then(Json::as_bool), Some(true));
        assert_eq!(env.at("profile.verified").and_then(Json::as_bool), Some(false));
        assert_eq!(env.field("input_fingerprint").as_str().map(str::len), Some(64));
    }
    // hidráulica: punto de operación y presión requerida
    let sc1 = res.field("hydraulic").field("NET1:SC1");
    assert_eq!(sc1.at("values.convergence.converged").and_then(Json::as_bool), Some(true));
    assert!(sc1.at("values.summary.total_demand_lpm").and_then(Json::as_f64).unwrap() > 100.0);
    let sc2 = res.field("hydraulic").field("NET1:SC2");
    assert!(sc2.at("values.required.required_pressure_bar").and_then(Json::as_f64).unwrap() > 0.5);
    // extintores: el ejemplo contiene hallazgos reales que el motor debe señalar
    let ec = codes(res.field("extinguishers"));
    assert!(ec.contains(&"ext.unsuitable_agent_present".to_string()), "agua en zona con clase B: {:?}", ec);
    assert!(ec.contains(&"ext.insufficient_by_area".to_string()), "{:?}", ec);
    // la compuerta: borrador por perfil de demostración
    let st = call(&a, "project.status", Json::obj().with("project", p.clone()));
    assert_eq!(st.at("gate.status").and_then(Json::as_str), Some("draft"));
    let gcodes: Vec<String> =
        st.at("gate.issues").unwrap().items().iter().filter_map(|i| i.field("code").as_str().map(|s| s.to_string())).collect();
    for c in ["gate.profile_demo", "gate.profile_not_verified", "gate.assumption_unconfirmed"] {
        assert!(gcodes.contains(&c.to_string()), "falta {}: {:?}", c, gcodes);
    }
    // los resultados recién calculados NO están obsoletos
    assert!(!gcodes.contains(&"gate.result_stale".to_string()), "{:?}", gcodes);
    // reabrir desde el almacén conserva todo
    let opened = call(&a, "project.open", Json::obj().with("id", p.at("meta.id").cloned().unwrap()));
    assert_eq!(opened.at("project.results").unwrap().canonical(), res.canonical());
}

#[test]
fn cambiar_un_dato_deja_el_resultado_obsoleto() {
    let a = app();
    let mut p = calculated(&a);
    p.set_at("systems.water.networks.0.links.1.diameter_mm", Json::num(52.0));
    let st = call(&a, "project.status", Json::obj().with("project", p.clone()));
    let stale: Vec<String> = st
        .at("gate.issues")
        .unwrap()
        .items()
        .iter()
        .filter(|i| i.field("code").as_str() == Some("gate.result_stale"))
        .filter_map(|i| i.at("params.result").and_then(Json::as_str).map(|s| s.to_string()))
        .collect();
    assert!(stale.iter().any(|r| r.starts_with("hydraulic:NET1")), "{:?}", stale);
    assert!(!stale.iter().any(|r| r == "foam:FM1"), "un cambio en la red no debe invalidar la espuma: {:?}", stale);
    // recalcular lo deja vigente otra vez
    let r = call(&a, "calc.all", Json::obj().with("project", p.clone()));
    p.set("results", r.field("results").clone());
    let st = call(&a, "project.status", Json::obj().with("project", p));
    assert!(!st.at("gate.issues").unwrap().items().iter().any(|i| i.field("code").as_str() == Some("gate.result_stale")));
}

#[test]
fn roles_se_hacen_cumplir_y_cada_cambio_queda_en_la_auditoria() {
    let a = app();
    let admin_name = call(&a, "session.get", Json::obj()).field("user").as_str().unwrap().to_string();
    let p = calculated(&a);
    let id = p.at("meta.id").and_then(Json::as_str).unwrap().to_string();
    call(&a, "users.save", Json::obj().with("name", Json::str("lola")).with("role", Json::str("viewer")));
    call(&a, "users.save", Json::obj().with("name", Json::str("rafa")).with("role", Json::str("reviewer")));
    call(&a, "users.save", Json::obj().with("name", Json::str("dani")).with("role", Json::str("designer")));
    // lectura: no puede escribir ni calcular, sí exportar
    call(&a, "session.set", Json::obj().with("user", Json::str("lola")));
    assert_eq!(call_err(&a, "project.save", Json::obj().with("project", p.clone())).field("code").as_str(), Some("auth.denied"));
    assert_eq!(call_err(&a, "calc.all", Json::obj().with("project", p.clone())).field("code").as_str(), Some("auth.denied"));
    assert_eq!(
        call_err(&a, "users.save", Json::obj().with("name", Json::str("x")).with("role", Json::str("admin"))).field("code").as_str(),
        Some("auth.denied")
    );
    call(&a, "export.run", Json::obj().with("project", p.clone()).with("kind", Json::str("json")));
    // el revisor no puede cambiar entradas, pero puede revisar
    call(&a, "session.set", Json::obj().with("user", Json::str("rafa")));
    let mut edited = p.clone();
    edited.set_at("site.height_m", Json::num(99.0));
    let e = call_err(&a, "project.save", Json::obj().with("project", edited));
    assert_eq!(e.field("code").as_str(), Some("auth.forbidden_change"));
    // no puede aprobar mientras haya bloqueos
    let e = call_err(&a, "review.add", Json::obj().with("id", Json::str(id.as_str())).with("decision", Json::str("approve")));
    assert_eq!(e.field("code").as_str(), Some("review.cannot_approve_with_blocks"));
    let rv = call(
        &a,
        "review.add",
        Json::obj()
            .with("id", Json::str(id.as_str()))
            .with("decision", Json::str("comment"))
            .with("notes", Json::str("Falta verificar datos de suministro.")),
    );
    assert_eq!(rv.at("reviews.0.role").and_then(Json::as_str), Some("reviewer"));
    // el proyectista no puede firmar revisiones ni auto-aprobar excepciones
    call(&a, "session.set", Json::obj().with("user", Json::str("dani")));
    assert_eq!(
        call_err(
            &a,
            "review.add",
            Json::obj().with("id", Json::str(id.as_str())).with("decision", Json::str("comment")).with("notes", Json::str("x"))
        )
        .field("code")
        .as_str(),
        Some("auth.denied")
    );
    let exd = call(
        &a,
        "exception.add",
        Json::obj()
            .with("id", Json::str(id.as_str()))
            .with("code", Json::str("ext.insufficient_by_area"))
            .with("object", Json::str("R3"))
            .with("reason", Json::str("Se instalará una unidad adicional en la obra.")),
    );
    assert!(exd.at("exceptions.0.approved_by").unwrap().is_null());
    assert_eq!(
        call_err(&a, "exception.approve", Json::obj().with("id", Json::str(id.as_str())).with("exception_id", Json::str("EX-1")))
            .field("code")
            .as_str(),
        Some("auth.denied")
    );
    call(&a, "session.set", Json::obj().with("user", Json::str("rafa")));
    let appr = call(&a, "exception.approve", Json::obj().with("id", Json::str(id.as_str())).with("exception_id", Json::str("EX-1")));
    assert_eq!(appr.at("exceptions.0.approved_by").and_then(Json::as_str), Some("rafa"));
    // auditoría: cadena íntegra y con las acciones esperadas
    call(&a, "session.set", Json::obj().with("user", Json::str(admin_name.as_str())));
    let v = call(&a, "audit.verify", Json::obj());
    assert_eq!(v.field("ok").as_bool(), Some(true), "{}", v.to_compact());
    let list = call(&a, "audit.list", Json::obj());
    let actions: Vec<String> =
        list.field("entries").items().iter().filter_map(|e| e.field("action").as_str().map(|s| s.to_string())).collect();
    for want in [
        "app.start",
        "project.save",
        "calc.run",
        "session.change",
        "user.save",
        "review.add",
        "exception.add",
        "exception.approve",
        "export.generate",
    ] {
        assert!(actions.iter().any(|x| x == want), "falta la acción {} en {:?}", want, actions);
    }
    // el guardado registra qué cambió (rutas), no solo que cambió
    let saves: Vec<&Json> = list.field("entries").items().iter().filter(|e| e.field("action").as_str() == Some("project.save")).collect();
    assert!(saves.iter().any(|e| !e.at("detail.changes").map(Json::items).unwrap_or(&[]).is_empty()));
}

#[test]
fn exportaciones_reales_con_marca_de_licencia_y_estado_de_borrador() {
    let a = app();
    let p = calculated(&a);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("samples");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let mut texts = std::collections::BTreeMap::new();
    for (kind, lang) in [("pdf", "es"), ("pdf", "en"), ("xlsx", "es"), ("csv", "es"), ("dxf", "es"), ("json", "es"), ("ifc", "es")] {
        let r = call(&a, "export.run", Json::obj().with("project", p.clone()).with("kind", Json::str(kind)).with("lang", Json::str(lang)));
        let bytes = decode_export(&r).expect("base64 válido");
        assert_eq!(r.field("size").as_f64(), Some(bytes.len() as f64));
        assert_eq!(r.field("sha256").as_str().map(|s| s.to_string()), Some(vigix_core::hash::sha256_hex(&bytes)));
        assert_eq!(r.field("document_status").as_str(), Some("draft"));
        assert!(
            r.field("filename").as_str().unwrap().contains("BORRADOR"),
            "el nombre del archivo declara el borrador: {}",
            r.field("filename").to_compact()
        );
        assert_eq!(r.at("manifest.document_status").and_then(Json::as_str), Some("draft"));
        assert_eq!(r.at("manifest.profile.demo").and_then(Json::as_bool), Some(true));
        let fname = format!("{}_{}", lang, r.field("filename").as_str().unwrap());
        fs::write(dir.join(&fname), &bytes).unwrap();
        texts.insert(fname, bytes);
    }
    let get = |k: &str| texts.iter().find(|(n, _)| n.contains(k)).map(|(_, b)| b.clone()).unwrap();
    let pdf = get("es_EJEMPLO-DEMO-001_memoria");
    let pdf_txt = String::from_utf8_lossy(&pdf).to_string();
    assert!(pdf_txt.starts_with("%PDF-1.4"));
    assert!(pdf_txt.contains("FREEMIUM"), "sin licencia PRO el PDF lleva marca de evaluación");
    assert!(pdf_txt.contains("BORRADOR"), "el PDF lleva el banner de borrador");
    let en_pdf = String::from_utf8_lossy(&get("en_EJEMPLO-DEMO-001_memoria")).to_string();
    assert!(en_pdf.contains("DRAFT / NOT FOR CONSTRUCTION"));
    // el DXF y el IFC declaran el estado
    assert!(String::from_utf8_lossy(&get("_plano_L1")).contains("BORRADOR"));
    let ifc = String::from_utf8_lossy(&get(".ifc")).to_string();
    assert!(
        ifc.starts_with("ISO-10303-21;")
            && ifc.contains("IFCFIRESUPPRESSIONTERMINAL")
            && ifc.contains("IFCPIPESEGMENT")
            && ifc.contains("IFCSPACE")
    );
    // el JSON exportado es autocontenido y reimportable
    let j = Json::parse(&String::from_utf8(get(".json")).unwrap()).unwrap();
    assert_eq!(j.field("format").as_str(), Some("vigix.export/1"));
    assert_eq!(j.at("profile_package.profile_code").and_then(Json::as_str), Some("DEMO-NO-NORMATIVO"));
    let back = call(&a, "project.import", Json::obj().with("text", Json::str(String::from_utf8(get(".json")).unwrap())));
    assert_eq!(back.at("project.meta.id").and_then(Json::as_str), Some("EJEMPLO-DEMO-001"));
    // XLSX y CSV son ZIP con la estructura esperada
    let xlsx = vigix_core::export::zip::read_stored(&get(".xlsx")).unwrap();
    assert!(xlsx.iter().any(|f| f.0 == "xl/workbook.xml") && xlsx.len() > 10);
    let csv = vigix_core::export::zip::read_stored(&get("_csv")).unwrap();
    let names: Vec<&str> = csv.iter().map(|f| f.0.as_str()).collect();
    assert!(names.contains(&"MANIFEST.json") && names.contains(&"issues.csv") && names.contains(&"hyd_links.csv"), "{:?}", names);
    let manifest = Json::parse(std::str::from_utf8(&csv.iter().find(|f| f.0 == "MANIFEST.json").unwrap().1).unwrap()).unwrap();
    for f in manifest.field("files").items() {
        let name = f.field("name").as_str().unwrap();
        let body = &csv.iter().find(|x| x.0 == name).unwrap().1;
        assert_eq!(f.field("sha256").as_str(), Some(vigix_core::hash::sha256_hex(body).as_str()), "huella de {}", name);
    }
    // con una licencia PRO firmada desaparece la marca de evaluación (no el estado de borrador)
    let mut lic = Json::obj()
        .with("product", Json::str(PRODUCT_ID))
        .with("edition", Json::str("pro"))
        .with("licensee", Json::str("Estudio"))
        .with("issued", Json::str("2026-10-28"))
        .with("machine_id_hash", Json::Null);
    let sig = sha256(&payload_bytes(&lic));
    lic.set("signature", Json::str(base64_encode(&sig)));
    let st = call(&a, "license.activate", Json::obj().with("text", Json::str(lic.to_compact())));
    assert_eq!(st.field("edition").as_str(), Some("pro"));
    let r = call(&a, "export.run", Json::obj().with("project", p.clone()).with("kind", Json::str("pdf")));
    let pdf2 = String::from_utf8_lossy(&decode_export(&r).unwrap()).to_string();
    assert!(!pdf2.contains("FREEMIUM") && pdf2.contains("BORRADOR"));
    // licencia manipulada: se rechaza y vuelve a Freemium sin bloquear la aplicación
    let bad = lic.to_compact().replace("Estudio", "Otro");
    let st = call(&a, "license.activate", Json::obj().with("text", Json::str(bad)));
    assert_eq!(st.field("edition").as_str(), Some("freemium"));
    assert_eq!(st.field("reason").as_str(), Some("license.bad_signature"));
}

#[test]
fn copias_ajustes_y_perfiles() {
    let a = app();
    let p = calculated(&a);
    let id = p.at("meta.id").and_then(Json::as_str).unwrap().to_string();
    // copia y restauración (exige confirmación explícita)
    let name = call(&a, "backup.create", Json::obj().with("label", Json::str("c1")));
    assert_eq!(name.as_str(), Some("c1"));
    call(&a, "project.delete", Json::obj().with("id", Json::str(id.as_str())));
    assert_eq!(call(&a, "project.list", Json::obj()).items().len(), 0);
    assert_eq!(
        call_err(&a, "backup.restore", Json::obj().with("name", Json::str("c1"))).field("code").as_str(),
        Some("api.confirmation_required")
    );
    let r = call(&a, "backup.restore", Json::obj().with("name", Json::str("c1")).with("confirm", Json::Bool(true)));
    assert_eq!(r.field("projects_restored").as_f64(), Some(1.0));
    assert_eq!(call(&a, "project.list", Json::obj()).items().len(), 1);
    // ajustes validados
    call(&a, "settings.set", Json::obj().with("key", Json::str("language")).with("value", Json::str("en")));
    assert_eq!(call(&a, "settings.get", Json::obj().with("key", Json::str("language"))).as_str(), Some("en"));
    assert_eq!(
        call_err(&a, "settings.set", Json::obj().with("key", Json::str("language")).with("value", Json::str("fr"))).field("code").as_str(),
        Some("data.bad_enum")
    );
    assert_eq!(
        call_err(&a, "settings.set", Json::obj().with("key", Json::str("clave_inventada")).with("value", Json::num(1.0)))
            .field("code")
            .as_str(),
        Some("settings.unknown_key")
    );
    assert_eq!(
        call_err(&a, "settings.set", Json::obj().with("key", Json::str("autosave_seconds")).with("value", Json::num(-5.0)))
            .field("code")
            .as_str(),
        Some("data.bad_enum")
    );
    // un proyecto nuevo hereda el idioma de los ajustes
    let np = call(&a, "project.new", Json::obj().with("name", Json::str("Nuevo")));
    assert_eq!(np.at("display.language").and_then(Json::as_str), Some("en"));
    assert!(np.at("meta.id").and_then(Json::as_str).unwrap().starts_with("PRJ-20261028-"));
    // perfiles: la plantilla vacía no se puede importar; un paquete con un caso incorrecto se degrada
    let tpl = call(&a, "app.resource", Json::obj().with("name", Json::str("profile.template")));
    assert!(call_err(&a, "profile.import", Json::obj().with("package", tpl)).field("code").as_str().unwrap().starts_with("profile."));
    let demo = call(&a, "profile.get", Json::obj().with("code", Json::str("DEMO-NO-NORMATIVO")).with("version", Json::str("0.1.0")));
    let mut pkg = demo.field("package").clone();
    pkg.set_at("profile_code", Json::str("DEMO-MALO"));
    pkg.set_at("test_cases.1.expected.0.value", Json::num(1.0));
    let sealed = call(&a, "profile.seal", Json::obj().with("package", pkg));
    let imported = call(&a, "profile.import", Json::obj().with("package", sealed.field("package").clone()));
    let issues: Vec<String> =
        imported.at("summary.issues").unwrap().items().iter().filter_map(|i| i.field("code").as_str().map(|s| s.to_string())).collect();
    assert!(issues.contains(&"profile.selftest_failed".to_string()), "{:?}", issues);
    let list = call(&a, "profile.list", Json::obj());
    assert!(list.items().len() >= 2);
    // reimportar el mismo código y versión con otro contenido se rechaza (perfil publicado = inmutable)
    let mut other = sealed.field("package").clone();
    other.set_at("parameters.hydraulics.fluid.density.value", Json::num(999.0));
    let other = call(&a, "profile.seal", Json::obj().with("package", other));
    assert_eq!(
        call_err(&a, "profile.import", Json::obj().with("package", other.field("package").clone())).field("code").as_str(),
        Some("profile.version_conflict")
    );
}

#[test]
fn entradas_hostiles_y_errores_explicitos() {
    let a = app();
    assert_eq!(Json::parse(&a.handle("esto no es json")).unwrap().at("error.code").and_then(Json::as_str), Some("api.bad_request"));
    assert_eq!(
        Json::parse(&a.handle(r#"{"cmd":"formatear.disco","args":{}}"#)).unwrap().at("error.code").and_then(Json::as_str),
        Some("api.unknown_command")
    );
    assert_eq!(Json::parse(&a.handle(r#"{"args":{}}"#)).unwrap().at("error.code").and_then(Json::as_str), Some("api.bad_request"));
    let deep = format!("{}1{}", "[".repeat(500), "]".repeat(500));
    assert_eq!(
        Json::parse(&a.handle(&format!(r#"{{"cmd":"project.validate","args":{{"project":{}}}}}"#, deep)))
            .unwrap()
            .at("error.code")
            .and_then(Json::as_str),
        Some("api.bad_request")
    );
    // ids con rutas maliciosas
    assert_eq!(
        call_err(&a, "project.open", Json::obj().with("id", Json::str("../../etc/passwd"))).field("code").as_str(),
        Some("store.invalid")
    );
    // esquema desconocido y versión más nueva
    let e = call_err(&a, "project.import", Json::obj().with("text", Json::str(r#"{"schema":"vigix.project/9"}"#)));
    assert_eq!(e.field("code").as_str(), Some("project.newer_schema"));
    let e = call_err(&a, "project.import", Json::obj().with("text", Json::str("{ roto")));
    assert_eq!(e.field("code").as_str(), Some("project.file_corrupt"));
    // un cálculo sin perfil devuelve un resultado BLOQUEADO (no valores)
    let mut p = example();
    p.set_at("jurisdiction.profile_code", Json::Null);
    let r = call(&a, "calc.extinguishers", Json::obj().with("project", p));
    assert_eq!(r.at("envelope.status").and_then(Json::as_str), Some("blocked"));
    assert!(r.at("envelope.values").unwrap().is_null());
    assert_eq!(r.at("envelope.issues.0.code").and_then(Json::as_str), Some("project.no_profile"));
    // el último administrador no se puede degradar ni borrar
    let me = call(&a, "session.get", Json::obj());
    let name = me.field("user").as_str().unwrap().to_string();
    assert_eq!(
        call_err(&a, "users.save", Json::obj().with("name", Json::str(name.as_str())).with("role", Json::str("viewer")))
            .field("code")
            .as_str(),
        Some("users.last_admin")
    );
    // recursos embebidos disponibles sin red
    for res in ["units", "i18n.es", "i18n.en", "profile.template"] {
        assert!(call(&a, "app.resource", Json::obj().with("name", Json::str(res))).as_obj().is_some(), "{}", res);
    }
    // estimación previa de tamaño
    let est = call(&a, "calc.estimate", Json::obj().with("project", example()).with("network_id", Json::str("NET1")));
    assert_eq!(est.field("nodes").as_f64(), Some(8.0));
    assert!(est.field("estimated_memory_bytes").as_f64().unwrap() > 0.0);
}

#[test]
fn el_calculo_hidraulico_es_cancelable() {
    let a = app();
    // Un identificador de trabajo desconocido no falla; cancelar uno activo se prueba con el flag directo
    let r = call(&a, "calc.cancel", Json::obj().with("job_id", Json::str("inexistente")));
    assert_eq!(r.field("cancelled").as_bool(), Some(false));
}
