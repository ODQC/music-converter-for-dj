//! Perfiles de compatibilidad de cada modelo de CDJ.
//!
//! Cada perfil es un archivo JSON en `profiles/`, sacado de la sección de formatos
//! reproducibles del manual del equipo. Para agregar un modelo basta con agregar un JSON
//! y registrarlo en [`builtin_profiles`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::AudioFormat;

const BUILTIN: &[&str] = &[
    include_str!("../profiles/cdj-2000nxs.json"),
    include_str!("../profiles/cdj-2000nxs2.json"),
    include_str!("../profiles/cdj-3000.json"),
    include_str!("../profiles/cdj-3000x.json"),
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CdjProfile {
    pub id: String,
    pub name: String,
    /// De dónde salen los datos (manual, especificaciones, etc.).
    pub source: String,
    /// `false` cuando los datos no salen de un manual revisado.
    pub verified: bool,
    pub max_channels: u8,
    /// Niveles de carpetas que el equipo puede recorrer, si el manual lo indica.
    pub max_folder_depth: Option<usize>,
    pub formats: BTreeMap<AudioFormat, FormatSupport>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FormatSupport {
    /// Extensiones aceptadas, en minúsculas y sin punto.
    pub extensions: Vec<String>,
    pub variants: Vec<FormatVariant>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FormatVariant {
    pub label: String,
    pub sample_rates: Vec<u32>,
    /// Profundidades de bits aceptadas (solo formatos sin compresión o sin pérdida).
    #[serde(default)]
    pub bit_depths: Vec<u8>,
    /// Rango `[mín, máx]` de bitrate en kbps (solo formatos con pérdida).
    #[serde(default)]
    pub bitrate_kbps: Option<[u32; 2]>,
}

impl CdjProfile {
    pub fn supports(&self, format: AudioFormat) -> bool {
        self.formats.contains_key(&format)
    }

    /// Todas las frecuencias de muestreo aceptadas para un formato, ordenadas.
    pub fn sample_rates(&self, format: AudioFormat) -> Vec<u32> {
        let mut rates: Vec<u32> = self
            .formats
            .get(&format)
            .map(|f| {
                f.variants
                    .iter()
                    .flat_map(|v| v.sample_rates.iter().copied())
                    .collect()
            })
            .unwrap_or_default();
        rates.sort_unstable();
        rates.dedup();
        rates
    }

    /// Todas las profundidades de bits aceptadas para un formato, ordenadas.
    pub fn bit_depths(&self, format: AudioFormat) -> Vec<u8> {
        let mut depths: Vec<u8> = self
            .formats
            .get(&format)
            .map(|f| {
                f.variants
                    .iter()
                    .flat_map(|v| v.bit_depths.iter().copied())
                    .collect()
            })
            .unwrap_or_default();
        depths.sort_unstable();
        depths.dedup();
        depths
    }

    /// Bitrate máximo aceptado (kbps) para un formato con pérdida.
    pub fn max_bitrate_kbps(&self, format: AudioFormat, sample_rate: u32) -> Option<u32> {
        self.formats
            .get(&format)?
            .variants
            .iter()
            .filter(|v| v.sample_rates.contains(&sample_rate))
            .filter_map(|v| v.bitrate_kbps.map(|r| r[1]))
            .max()
    }
}

pub fn builtin_profiles() -> Vec<CdjProfile> {
    BUILTIN
        .iter()
        .map(|json| serde_json::from_str(json).expect("perfil de CDJ incluido inválido"))
        .collect()
}

pub fn find_profile(id: &str) -> Option<CdjProfile> {
    builtin_profiles().into_iter().find(|p| p.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_profiles_parse() {
        let profiles = builtin_profiles();
        assert_eq!(profiles.len(), 4);
        let nxs = find_profile("cdj-2000nxs").unwrap();
        assert!(!nxs.supports(AudioFormat::Flac));
        assert_eq!(nxs.sample_rates(AudioFormat::Wav), vec![44100, 48000]);
        let cdj3000 = find_profile("cdj-3000").unwrap();
        assert!(cdj3000.supports(AudioFormat::Alac));
        assert_eq!(cdj3000.bit_depths(AudioFormat::Flac), vec![16, 24]);
        assert_eq!(cdj3000.max_bitrate_kbps(AudioFormat::Mp3, 44100), Some(320));
    }
}
