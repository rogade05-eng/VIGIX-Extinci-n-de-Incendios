//! Catálogos de traducción compartidos con la interfaz (`i18n/es.json`, `i18n/en.json`).
//!
//! Reglas del contrato (7): el motor emite **códigos y parámetros**, nunca texto visible; el texto
//! sale del catálogo. Una clave ausente en el idioma pedido se muestra con el texto español y la
//! marca `[ES]` (nunca como código interno sin aviso).

use crate::issues::Issue;
use crate::json::Json;
use std::collections::BTreeMap;
use std::sync::OnceLock;

const ES_JSON: &str = include_str!("../../../i18n/es.json");
const EN_JSON: &str = include_str!("../../../i18n/en.json");

pub struct Catalog {
    es: BTreeMap<String, String>,
    en: BTreeMap<String, String>,
}

fn load(txt: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    if let Ok(Json::Obj(o)) = Json::parse(txt) {
        for (k, v) in o {
            if let Json::Str(s) = v {
                m.insert(k, s);
            }
        }
    }
    m
}

pub fn catalog() -> &'static Catalog {
    static C: OnceLock<Catalog> = OnceLock::new();
    C.get_or_init(|| Catalog { es: load(ES_JSON), en: load(EN_JSON) })
}

/// Sustituye `{nombre}` por el parámetro correspondiente; un marcador sin valor se muestra como `?`.
pub fn fill(template: &str, params: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('}') {
            Some(j) if after[..j].chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && j > 0 => {
                out.push_str(params.get(&after[..j]).map(String::as_str).unwrap_or("?"));
                rest = &after[j + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

impl Catalog {
    pub fn has(&self, lang: &str, key: &str) -> bool {
        match lang {
            "en" => self.en.contains_key(key),
            _ => self.es.contains_key(key),
        }
    }

    pub fn keys(&self, lang: &str) -> Vec<&str> {
        match lang {
            "en" => self.en.keys().map(String::as_str).collect(),
            _ => self.es.keys().map(String::as_str).collect(),
        }
    }

    pub fn raw(&self, lang: &str, key: &str) -> Option<&str> {
        match lang {
            "en" => self.en.get(key).map(String::as_str),
            _ => self.es.get(key).map(String::as_str),
        }
    }

    /// Texto traducido. Si falta en `lang`, usa el español con la marca `[ES]`; si falta en ambos,
    /// devuelve `⟦clave⟧` (un defecto que las pruebas impiden).
    pub fn t(&self, lang: &str, key: &str, params: &BTreeMap<String, String>) -> String {
        if let Some(s) = self.raw(lang, key) {
            return fill(s, params);
        }
        if lang != "es" {
            if let Some(s) = self.es.get(key) {
                return format!("[ES] {}", fill(s, params));
            }
        }
        format!("⟦{}⟧", key)
    }

    pub fn tr(&self, lang: &str, key: &str) -> String {
        self.t(lang, key, &BTreeMap::new())
    }

    /// Mensaje y acción de una incidencia (el objeto y la ruta se ofrecen como `{object}` y `{path}`).
    pub fn issue_text(&self, lang: &str, i: &Issue) -> (String, String) {
        let mut p = i.params.clone();
        p.entry("object".to_string()).or_insert_with(|| i.object.clone().unwrap_or_default());
        p.entry("path".to_string()).or_insert_with(|| i.path.clone());
        let msg = self.t(lang, &format!("issue.{}", i.code), &p);
        let act = self.t(lang, &format!("issue.{}.action", i.code), &p);
        (msg, act)
    }
}

pub fn t(lang: &str, key: &str) -> String {
    catalog().tr(lang, key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholders(s: &str) -> Vec<String> {
        let mut v = Vec::new();
        let mut rest = s;
        while let Some(i) = rest.find('{') {
            let after = &rest[i + 1..];
            if let Some(j) = after.find('}') {
                v.push(after[..j].to_string());
                rest = &after[j + 1..];
            } else {
                break;
            }
        }
        v.sort();
        v
    }

    #[test]
    fn los_catalogos_tienen_las_mismas_claves_sin_valores_vacios() {
        let c = catalog();
        let es: Vec<&str> = c.keys("es");
        let en: Vec<&str> = c.keys("en");
        assert_eq!(es, en, "ES y EN deben tener exactamente las mismas claves");
        assert!(es.len() > 200);
        for k in &es {
            let (a, b) = (c.raw("es", k).unwrap(), c.raw("en", k).unwrap());
            assert!(!a.trim().is_empty() && !b.trim().is_empty(), "valor vacío en {}", k);
            assert_eq!(placeholders(a), placeholders(b), "los marcadores de {} difieren entre idiomas", k);
        }
    }

    #[test]
    fn sustitucion_de_parametros() {
        let mut p = BTreeMap::new();
        p.insert("field".to_string(), "diameter_mm".to_string());
        assert_eq!(fill("Falta «{field}».", &p), "Falta «diameter_mm».");
        assert_eq!(fill("sin marcador", &p), "sin marcador");
        assert_eq!(fill("falta {otro} aquí", &p), "falta ? aquí");
        assert_eq!(fill("llave { suelta y {field}", &p), "llave { suelta y diameter_mm");
        assert_eq!(fill("{}", &p), "{}");
    }

    #[test]
    fn incidencia_se_traduce_con_objeto_y_parametros() {
        let i = Issue::block("data.missing").module("water").path("a.b").object("P1").param("field", "diameter_mm");
        let (m, a) = catalog().issue_text("es", &i);
        assert_eq!(m, "Falta el dato obligatorio «diameter_mm».");
        assert!(a.contains("cero"));
        let (m, _) = catalog().issue_text("en", &i);
        assert_eq!(m, "Required data “diameter_mm” is missing.");
        let i = Issue::error("ext.no_serving_unit").object("R1").param("fire_class", "C").param("hazard_class", "demo_low");
        assert!(catalog().issue_text("es", &i).0.contains("«R1»"));
    }

    #[test]
    fn clave_ausente_en_ingles_se_marca_y_nunca_queda_cruda() {
        let c = catalog();
        assert_eq!(c.tr("es", "status.draft"), "BORRADOR / NO APTO PARA CONSTRUCCIÓN");
        assert_eq!(c.tr("en", "status.draft"), "DRAFT / NOT FOR CONSTRUCTION");
        assert_eq!(c.tr("es", "no.existe.esta.clave"), "⟦no.existe.esta.clave⟧");
        // simular clave solo en español
        let mut es = BTreeMap::new();
        es.insert("solo.es".to_string(), "texto".to_string());
        let cat = Catalog { es, en: BTreeMap::new() };
        assert_eq!(cat.tr("en", "solo.es"), "[ES] texto");
    }

    #[test]
    fn todo_codigo_de_incidencia_del_codigo_fuente_tiene_traduccion() {
        // Recorre los fuentes de producción y comprueba que cada código emitido (incidencias y errores
        // de API) tiene mensaje y acción en ambos idiomas.
        let sources = [
            include_str!("agents/aerosol.rs"),
            include_str!("agents/dry_chem.rs"),
            include_str!("agents/foam.rs"),
            include_str!("agents/gas.rs"),
            include_str!("agents/mod.rs"),
            include_str!("agents/wet_chem.rs"),
            include_str!("api.rs"),
            include_str!("audit.rs"),
            include_str!("budget.rs"),
            include_str!("cause_effect.rs"),
            include_str!("extinguishers.rs"),
            include_str!("gates.rs"),
            include_str!("hydraulics/model.rs"),
            include_str!("license.rs"),
            include_str!("permissions.rs"),
            include_str!("profile.rs"),
            include_str!("project.rs"),
            include_str!("reader.rs"),
            include_str!("selftest.rs"),
            include_str!("store.rs"),
        ];
        let markers = [
            "Issue::block(",
            "Issue::error(",
            "Issue::warning(",
            "Issue::info(",
            "Issue::pass(",
            "blk(",
            "inv(&mut issues, ",
            "ApiError::new(",
        ];
        let c = catalog();
        let mut checked = 0;
        for src in sources {
            let prod = src.split("#[cfg(test)]").next().unwrap_or(src);
            for m in markers {
                for (pos, _) in prod.match_indices(m) {
                    let after = prod[pos + m.len()..].trim_start();
                    let Some(rest) = after.strip_prefix('"') else { continue };
                    let Some(end) = rest.find('"') else { continue };
                    let code = &rest[..end];
                    // solo códigos con forma `modulo.nombre` (se descartan formatos como "{}.x")
                    if !code.contains('.')
                        || !code.chars().all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_' || ch.is_ascii_digit())
                    {
                        continue;
                    }
                    for lang in ["es", "en"] {
                        assert!(c.has(lang, &format!("issue.{}", code)), "falta issue.{} en {}", code, lang);
                        assert!(c.has(lang, &format!("issue.{}.action", code)), "falta issue.{}.action en {}", code, lang);
                    }
                    checked += 1;
                }
            }
        }
        // códigos formados con format!("{}.predischarge_alarm_missing", prefijo)
        for code in ["safety.predischarge_alarm_missing", "safety.evacuation_not_verified"] {
            assert!(c.has("es", &format!("issue.{}", code)) && c.has("en", &format!("issue.{}", code)));
        }
        assert!(checked > 150, "la comprobación debía cubrir > 150 códigos (cubrió {})", checked);
    }
}
