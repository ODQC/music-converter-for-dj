//! Revisión de cada archivo contra el perfil del CDJ elegido.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::probe::{probe_file, AudioInfo};
use crate::profile::CdjProfile;
use crate::AudioFormat;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileReport {
    pub path: PathBuf,
    /// Carpeta que se escaneó (o la carpeta del archivo si se agregó suelto).
    pub root: PathBuf,
    /// Ruta relativa a `root`, que se respeta al escribir la copia.
    pub rel_path: PathBuf,
    pub info: Option<AudioInfo>,
    /// Si no se pudo leer el archivo.
    pub error: Option<String>,
    pub compatible: bool,
    /// Motivos por los que el CDJ no lo reproduce.
    pub reasons: Vec<String>,
    /// Avisos que no impiden reproducirlo.
    pub warnings: Vec<String>,
}

pub fn analyze_file(path: &Path, root: &Path, profile: &CdjProfile) -> FileReport {
    let rel_path = path
        .strip_prefix(root)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| path.file_name().map(PathBuf::from).unwrap_or_default());
    let mut report = FileReport {
        path: path.to_path_buf(),
        root: root.to_path_buf(),
        rel_path,
        info: None,
        error: None,
        compatible: false,
        reasons: Vec::new(),
        warnings: Vec::new(),
    };

    match probe_file(path) {
        Ok(info) => {
            let (reasons, warnings) = check(&info, profile);
            report.compatible = reasons.is_empty();
            report.reasons = reasons;
            report.warnings = warnings;
            report.info = Some(info);
        }
        Err(err) => {
            report.error = Some(err.to_string());
            report.reasons.push("No se pudo leer el archivo".into());
        }
    }

    if let Some(max) = profile.max_folder_depth {
        // Las carpetas que contienen al archivo, contando desde la raíz de la USB.
        let depth = report.rel_path.components().count().saturating_sub(1);
        if depth > max {
            report.warnings.push(format!(
                "Está a {depth} niveles de carpetas; el {} solo lee hasta {max}",
                profile.name
            ));
        }
    }

    report
}

/// Devuelve (motivos de incompatibilidad, avisos).
pub fn check(info: &AudioInfo, profile: &CdjProfile) -> (Vec<String>, Vec<String>) {
    let mut reasons = Vec::new();
    let mut warnings = Vec::new();

    if let Some(issue) = &info.codec_issue {
        reasons.push(issue.clone());
    }

    let Some(support) = profile.formats.get(&info.format) else {
        let name = if info.format == AudioFormat::Other {
            info.codec.clone()
        } else {
            info.format.label().to_string()
        };
        reasons.push(format!("El {} no reproduce archivos {name}", profile.name));
        return (reasons, warnings);
    };

    if !support.extensions.iter().any(|e| e == &info.extension) {
        reasons.push(format!(
            "Extensión .{} no reconocida por el {} (usa .{})",
            info.extension,
            profile.name,
            support.extensions.join(", .")
        ));
    }

    let rate_ok = |rate: u32| {
        support
            .variants
            .iter()
            .any(|v| v.sample_rates.contains(&rate))
    };
    match info.sample_rate {
        Some(rate) if !rate_ok(rate) => reasons.push(format!(
            "Frecuencia de muestreo {} no compatible (acepta {})",
            khz(rate),
            profile
                .sample_rates(info.format)
                .into_iter()
                .map(khz)
                .collect::<Vec<_>>()
                .join(", ")
        )),
        None => warnings.push("No se pudo leer la frecuencia de muestreo".into()),
        _ => {}
    }

    if !info.format.is_lossy() {
        let depths = profile.bit_depths(info.format);
        match info.bit_depth {
            Some(bits) if !depths.contains(&bits) => reasons.push(format!(
                "{bits} bits no compatible (acepta {})",
                depths
                    .iter()
                    .map(|b| format!("{b} bits"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            None => warnings.push("No se pudo leer la profundidad de bits".into()),
            _ => {}
        }
    } else if let (Some(rate), Some(kbps)) = (info.sample_rate, info.bitrate_kbps) {
        // El bitrate medido de un VBR puede caer fuera del rango sin que importe: solo avisamos.
        let in_range = support
            .variants
            .iter()
            .filter(|v| v.sample_rates.contains(&rate))
            .filter_map(|v| v.bitrate_kbps)
            .any(|[min, max]| kbps >= min && kbps <= max + 8);
        if rate_ok(rate) && !in_range {
            warnings.push(format!(
                "Bitrate de {kbps} kbps fuera del rango indicado en el manual"
            ));
        }
    }

    if let Some(channels) = info.channels {
        if channels > profile.max_channels {
            reasons.push(format!(
                "{channels} canales (los CDJ reproducen hasta estéreo)"
            ));
        }
    }

    (reasons, warnings)
}

pub fn khz(rate: u32) -> String {
    let k = rate as f64 / 1000.0;
    if rate.is_multiple_of(1000) {
        format!("{k:.0} kHz")
    } else {
        format!("{} kHz", format!("{k:.2}").trim_end_matches('0'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::find_profile;

    fn wav(rate: u32, bits: u8) -> AudioInfo {
        AudioInfo {
            format: AudioFormat::Wav,
            codec: "PCM".into(),
            extension: "wav".into(),
            sample_rate: Some(rate),
            bit_depth: Some(bits),
            bitrate_kbps: None,
            channels: Some(2),
            duration_secs: 1.0,
            codec_issue: None,
        }
    }

    #[test]
    fn nxs_rejects_96k_wav_but_3000_accepts_it() {
        let nxs = find_profile("cdj-2000nxs").unwrap();
        let cdj3000 = find_profile("cdj-3000").unwrap();
        let info = wav(96000, 24);
        assert_eq!(check(&info, &nxs).0.len(), 1);
        assert!(check(&info, &cdj3000).0.is_empty());
    }

    #[test]
    fn rejects_32bit_float_and_flac_on_nxs() {
        let cdj3000 = find_profile("cdj-3000").unwrap();
        let mut info = wav(44100, 32);
        info.codec_issue = Some("PCM de punto flotante".into());
        assert_eq!(check(&info, &cdj3000).0.len(), 2);

        let nxs = find_profile("cdj-2000nxs").unwrap();
        let flac = AudioInfo {
            format: AudioFormat::Flac,
            extension: "flac".into(),
            ..wav(44100, 16)
        };
        let (reasons, _) = check(&flac, &nxs);
        assert_eq!(
            reasons,
            vec!["El CDJ-2000NXS no reproduce archivos FLAC".to_string()]
        );
    }

    #[test]
    fn mp3_mpeg2_only_on_nexus() {
        let mp3 = AudioInfo {
            format: AudioFormat::Mp3,
            extension: "mp3".into(),
            bit_depth: None,
            bitrate_kbps: Some(128),
            ..wav(22050, 16)
        };
        assert!(check(&mp3, &find_profile("cdj-2000nxs").unwrap())
            .0
            .is_empty());
        assert!(!check(&mp3, &find_profile("cdj-3000").unwrap()).0.is_empty());
    }

    #[test]
    fn khz_formatting() {
        assert_eq!(khz(44100), "44.1 kHz");
        assert_eq!(khz(48000), "48 kHz");
        assert_eq!(khz(22050), "22.05 kHz");
    }
}
