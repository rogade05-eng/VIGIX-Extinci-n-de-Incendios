//! Licencias perpetuas (contrato 4.1 y 8.3).
//!
//! * **Freemium**: sin caducidad, todas las funciones de cálculo, con marca de evaluación en los
//!   documentos exportados.
//! * **PRO**: activación manual/offline con un archivo de licencia **firmado** (Ed25519). La clave
//!   privada vive solo en poder del emisor (nunca en el repositorio, CI ni artefactos); la
//!   aplicación incluye únicamente la clave pública.
//! * Ninguna decisión depende del reloj del equipo ni de internet. Una licencia inválida **no**
//!   bloquea la aplicación: vuelve a Freemium y explica el motivo.
//!
//! Este crate no tiene dependencias, por eso la verificación criptográfica se inyecta mediante el
//! rasgo [`SignatureVerifier`] (la implementación real con `ed25519-dalek` está en `src-tauri`).

use crate::hash::{base64_decode, sha256_hex};
use crate::json::Json;

pub const PRODUCT_ID: &str = "VIGIX-EXTINCION-FIJA";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edition {
    Freemium,
    Pro,
}

impl Edition {
    pub fn as_str(self) -> &'static str {
        match self {
            Edition::Freemium => "freemium",
            Edition::Pro => "pro",
        }
    }
}

pub trait SignatureVerifier: Send + Sync {
    /// ¿Es `signature` una firma válida de `message` con la clave pública del emisor?
    fn verify(&self, message: &[u8], signature: &[u8]) -> bool;
}

#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub edition: Edition,
    pub licensee: Option<String>,
    pub issued: Option<String>,
    /// Motivo por el que una licencia presentada no se aceptó (código de incidencia), si aplica.
    pub reason: Option<String>,
    pub watermark: bool,
}

impl Status {
    pub fn freemium(reason: Option<&str>) -> Status {
        Status { edition: Edition::Freemium, licensee: None, issued: None, reason: reason.map(|s| s.to_string()), watermark: true }
    }
    pub fn to_json(&self) -> Json {
        Json::obj()
            .with("edition", Json::str(self.edition.as_str()))
            .with("licensee", Json::opt_str(self.licensee.as_deref()))
            .with("issued", Json::opt_str(self.issued.as_deref()))
            .with("reason", Json::opt_str(self.reason.as_deref()))
            .with("watermark", Json::Bool(self.watermark))
            .with("perpetual", Json::Bool(true))
    }
}

/// Mensaje firmado: JSON canónico de la licencia **sin** el campo `signature`.
pub fn payload_bytes(lic: &Json) -> Vec<u8> {
    let mut c = lic.clone();
    if let Json::Obj(m) = &mut c {
        m.remove("signature");
    }
    c.canonical().into_bytes()
}

/// Código de solicitud para la activación manual: huella del identificador del equipo.
/// El emisor lo copia en `machine_id_hash` de la licencia que firma.
pub fn request_code(machine_id: &str) -> String {
    sha256_hex(format!("{}|{}", PRODUCT_ID, machine_id).as_bytes())
}

/// Evalúa un texto de licencia. `machine_id` es el identificador local del equipo (si la
/// plataforma lo ofrece); solo se usa si la licencia declara `machine_id_hash`.
pub fn check(license_text: &str, verifier: &dyn SignatureVerifier, machine_id: Option<&str>) -> Status {
    let Ok(lic) = Json::parse(license_text) else { return Status::freemium(Some("license.malformed")) };
    if lic.field("product").as_str() != Some(PRODUCT_ID) {
        return Status::freemium(Some("license.wrong_product"));
    }
    if lic.field("edition").as_str() != Some("pro") {
        return Status::freemium(Some("license.not_pro"));
    }
    let Some(sig) = lic.field("signature").as_str().and_then(base64_decode) else { return Status::freemium(Some("license.no_signature")) };
    if !verifier.verify(&payload_bytes(&lic), &sig) {
        return Status::freemium(Some("license.bad_signature"));
    }
    if let Some(h) = lic.field("machine_id_hash").as_str() {
        match machine_id {
            Some(m) if request_code(m) == h => {}
            _ => return Status::freemium(Some("license.machine_mismatch")),
        }
    }
    Status {
        edition: Edition::Pro,
        licensee: lic.field("licensee").as_str().map(|s| s.to_string()),
        issued: lic.field("issued").as_str().map(|s| s.to_string()),
        reason: None,
        watermark: false,
    }
}

/// Verificador de PRUEBA: acepta la firma = SHA-256(mensaje). Solo para tests; no es seguro.
pub struct TestOnlyVerifier;
impl SignatureVerifier for TestOnlyVerifier {
    fn verify(&self, message: &[u8], signature: &[u8]) -> bool {
        crate::hash::sha256(message).as_slice() == signature
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::{base64_encode, sha256};

    fn signed(mut lic: Json) -> String {
        let sig = sha256(&payload_bytes(&lic));
        lic.set("signature", Json::str(base64_encode(&sig)));
        lic.to_compact()
    }

    fn base() -> Json {
        Json::obj()
            .with("product", Json::str(PRODUCT_ID))
            .with("edition", Json::str("pro"))
            .with("licensee", Json::str("Estudio de Ingeniería"))
            .with("issued", Json::str("2026-10-28"))
            .with("machine_id_hash", Json::Null)
    }

    #[test]
    fn sin_licencia_es_freemium_perpetuo_con_marca() {
        let s = check("", &TestOnlyVerifier, None);
        assert_eq!(s.edition, Edition::Freemium);
        assert!(s.watermark);
        assert_eq!(s.to_json().field("perpetual").as_bool(), Some(true));
    }

    #[test]
    fn licencia_pro_firmada_quita_la_marca() {
        let s = check(&signed(base()), &TestOnlyVerifier, None);
        assert_eq!(s.edition, Edition::Pro);
        assert!(!s.watermark);
        assert_eq!(s.licensee.as_deref(), Some("Estudio de Ingeniería"));
        assert!(s.reason.is_none());
    }

    #[test]
    fn alterar_cualquier_campo_invalida_la_firma() {
        let text = signed(base());
        let tampered = text.replace("Estudio de Ingeniería", "Otro Estudio");
        let s = check(&tampered, &TestOnlyVerifier, None);
        assert_eq!(s.edition, Edition::Freemium);
        assert_eq!(s.reason.as_deref(), Some("license.bad_signature"));
        // el orden de claves del texto no afecta (JSON canónico)
        let reordered = Json::parse(&text).unwrap().to_pretty();
        assert_eq!(check(&reordered, &TestOnlyVerifier, None).edition, Edition::Pro);
    }

    #[test]
    fn rechazos_explicitos_sin_bloquear_la_aplicacion() {
        let r = |text: &str| check(text, &TestOnlyVerifier, None).reason;
        assert_eq!(r("no es json").as_deref(), Some("license.malformed"));
        assert_eq!(r(&signed(base().with("product", Json::str("OTRO")))).as_deref(), Some("license.wrong_product"));
        assert_eq!(r(&signed(base().with("edition", Json::str("freemium")))).as_deref(), Some("license.not_pro"));
        assert_eq!(r(&base().to_compact()).as_deref(), Some("license.no_signature"));
        let mut bad = base();
        bad.set("signature", Json::str("AAAA"));
        assert_eq!(r(&bad.to_compact()).as_deref(), Some("license.bad_signature"));
    }

    #[test]
    fn activacion_offline_ligada_al_equipo() {
        let code = request_code("MACHINE-GUID-123");
        assert_eq!(code.len(), 64);
        assert_ne!(code, request_code("MACHINE-GUID-124"));
        let lic = signed(base().with("machine_id_hash", Json::str(code.as_str())));
        assert_eq!(check(&lic, &TestOnlyVerifier, Some("MACHINE-GUID-123")).edition, Edition::Pro);
        let other = check(&lic, &TestOnlyVerifier, Some("OTRO-EQUIPO"));
        assert_eq!(other.reason.as_deref(), Some("license.machine_mismatch"));
        assert_eq!(check(&lic, &TestOnlyVerifier, None).edition, Edition::Freemium);
    }
}
