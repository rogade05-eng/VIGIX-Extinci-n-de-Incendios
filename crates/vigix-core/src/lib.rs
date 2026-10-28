//! # vigix-core
//!
//! Motor de VIGIX EXTINCIÓN FIJA: dominio, perfiles normativos, validación, cálculo
//! determinista y exportación. **Sin dependencias externas** (contrato 8.1 y 8.4).
//!
//! Principios de diseño que este crate hace cumplir:
//! * Ningún valor normativo (factor, distancia, densidad, presión, tiempo, concentración)
//!   está escrito en el código: todos vienen de un *perfil normativo* versionado.
//! * Ningún dato faltante se sustituye por cero ni por un supuesto oculto: el cálculo se
//!   bloquea y devuelve una incidencia que localiza el dato.
//! * Mismas entradas + mismo perfil => mismo resultado y misma huella SHA-256.

#![forbid(unsafe_code)]
// `Issue` lleva ruta, parámetros y referencia (diagnóstico completo). Es el valor de error de los
// accesores de perfil; el camino de error no es crítico en rendimiento.
#![allow(clippy::result_large_err)]

pub mod agents;
pub mod audit;
pub mod budget;
pub mod calc;
pub mod cause_effect;
pub mod csv;
pub mod export;
pub mod extinguishers;
pub mod gates;
pub mod geometry;
pub mod hash;
pub mod hydraulics;
pub mod i18n;
pub mod issues;
pub mod json;
pub mod license;
pub mod permissions;
pub mod profile;
pub mod project;
pub mod reader;
pub mod selftest;
pub mod store;
pub mod timeutil;
pub mod units;

/// Versión del motor de cálculo. Los resultados guardados se marcan obsoletos si cambia
/// (contrato 9.1).
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Nombre comercial del producto (decisión del cliente).
pub const PRODUCT_NAME: &str = "VIGIX EXTINCIÓN FIJA";
