//! Decide qué hacer con cada archivo: convertirlo (y a qué), copiarlo tal cual u omitirlo,
//! y dónde escribir el resultado.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::analyze::{khz, FileReport};
use crate::profile::CdjProfile;
use crate::{AudioFormat, Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    /// Mantener el formato original si el CDJ lo acepta; si no, usar `fallback_format`.
    Keep,
    Wav,
    Aiff,
    Flac,
    Alac,
    Mp3,
    Aac,
}

impl OutputFormat {
    fn as_audio_format(self) -> Option<AudioFormat> {
        Some(match self {
            OutputFormat::Keep => return None,
            OutputFormat::Wav => AudioFormat::Wav,
            OutputFormat::Aiff => AudioFormat::Aiff,
            OutputFormat::Flac => AudioFormat::Flac,
            OutputFormat::Alac => AudioFormat::Alac,
            OutputFormat::Mp3 => AudioFormat::Mp3,
            OutputFormat::Aac => AudioFormat::Aac,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvertSettings {
    pub output_format: OutputFormat,
    /// Formato para los archivos cuyo formato original el CDJ no acepta (con `Keep`).
    pub fallback_format: AudioFormat,
    /// `None` = automático (la más alta compatible sin pasarse de la original).
    pub bit_depth: Option<u8>,
    /// `None` = automático (la original si es compatible; si no, la más cercana).
    pub sample_rate: Option<u32>,
    /// Bitrate para MP3/AAC.
    pub lossy_bitrate_kbps: u32,
    /// Convertir también los archivos que ya son compatibles.
    pub convert_all: bool,
    /// Copiar los archivos compatibles a la carpeta de salida, para tener la colección completa.
    pub copy_compatible: bool,
    pub output_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Convert,
    Copy,
    Skip,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TargetSpec {
    pub format: AudioFormat,
    pub sample_rate: u32,
    /// Solo para formatos sin pérdida.
    pub bit_depth: Option<u8>,
    /// Solo para MP3/AAC.
    pub bitrate_kbps: Option<u32>,
    pub channels: u8,
}

impl TargetSpec {
    pub fn describe(&self) -> String {
        let mut parts = vec![self.format.label().to_string(), khz(self.sample_rate)];
        if let Some(bits) = self.bit_depth {
            parts.push(format!("{bits} bits"));
        }
        if let Some(kbps) = self.bitrate_kbps {
            parts.push(format!("{kbps} kbps"));
        }
        parts.join(" · ")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub source: PathBuf,
    pub output: Option<PathBuf>,
    pub action: Action,
    pub target: Option<TargetSpec>,
    pub target_label: Option<String>,
    /// Aclaraciones para el usuario (p. ej. "no recupera calidad").
    pub notes: Vec<String>,
    pub duration_secs: f64,
}

pub fn validate_settings(settings: &ConvertSettings, profile: &CdjProfile) -> Result<()> {
    if let Some(format) = settings.output_format.as_audio_format() {
        if !profile.supports(format) {
            return Err(Error::Invalid(format!(
                "El {} no reproduce {}",
                profile.name,
                format.label()
            )));
        }
    }
    if settings.output_format == OutputFormat::Keep && !profile.supports(settings.fallback_format) {
        return Err(Error::Invalid(format!(
            "El {} no reproduce {}",
            profile.name,
            settings.fallback_format.label()
        )));
    }
    if settings.output_dir.as_os_str().is_empty() {
        return Err(Error::Invalid("Elige una carpeta de salida".into()));
    }
    Ok(())
}

pub fn plan_jobs(
    reports: &[FileReport],
    profile: &CdjProfile,
    settings: &ConvertSettings,
) -> Result<Vec<Job>> {
    validate_settings(settings, profile)?;
    let mut taken = HashSet::new();
    Ok(reports
        .iter()
        .map(|r| plan_one(r, profile, settings, &mut taken))
        .collect())
}

fn plan_one(
    report: &FileReport,
    profile: &CdjProfile,
    settings: &ConvertSettings,
    taken: &mut HashSet<PathBuf>,
) -> Job {
    let info = report.info.as_ref();
    let source_format = info.map(|i| i.format).unwrap_or(AudioFormat::Other);
    let mut job = Job {
        source: report.path.clone(),
        output: None,
        action: Action::Skip,
        target: None,
        target_label: None,
        notes: Vec::new(),
        duration_secs: info.map(|i| i.duration_secs).unwrap_or(0.0),
    };

    if let Some(issue) = info.and_then(|i| i.codec_issue.as_deref()) {
        if issue.contains("DRM") {
            job.notes
                .push("Los archivos con DRM no se pueden convertir".into());
            return job;
        }
    }

    let copy_as_is = |job: &mut Job, taken: &mut HashSet<PathBuf>| {
        job.action = Action::Copy;
        job.output = Some(unique_output(
            &settings.output_dir.join(&report.rel_path),
            taken,
        ));
        job.target_label = Some("Copia sin cambios".into());
    };

    if report.compatible && !settings.convert_all {
        if settings.copy_compatible {
            copy_as_is(&mut job, taken);
        }
        return job;
    }

    let format = match settings.output_format.as_audio_format() {
        Some(f) => f,
        None if source_format != AudioFormat::Other && profile.supports(source_format) => {
            source_format
        }
        None => settings.fallback_format,
    };

    let sample_rate = resolve_sample_rate(
        info.and_then(|i| i.sample_rate),
        settings.sample_rate,
        &profile.sample_rates(format),
        &mut job.notes,
    );
    let bit_depth = if format.is_lossy() {
        None
    } else {
        let source_bits = if source_format.is_lossy() {
            None
        } else {
            info.and_then(|i| i.bit_depth)
        };
        Some(resolve_bit_depth(
            source_bits,
            settings.bit_depth,
            &profile.bit_depths(format),
            &mut job.notes,
        ))
    };
    let bitrate_kbps = format.is_lossy().then(|| {
        let max = profile.max_bitrate_kbps(format, sample_rate).unwrap_or(320);
        settings.lossy_bitrate_kbps.min(max)
    });
    let channels = info
        .and_then(|i| i.channels)
        .unwrap_or(2)
        .clamp(1, profile.max_channels);

    let target = TargetSpec {
        format,
        sample_rate,
        bit_depth,
        bitrate_kbps,
        channels,
    };

    // Con "convertir todos", un archivo que ya está exactamente así solo se copia.
    if report.compatible {
        if let Some(i) = info {
            let same_rate = i.sample_rate == Some(sample_rate);
            let same_bits = format.is_lossy() || i.bit_depth == bit_depth;
            if i.format == format && same_rate && same_bits && i.channels == Some(channels) {
                copy_as_is(&mut job, taken);
                return job;
            }
        }
    }

    if source_format.is_lossy() && !format.is_lossy() {
        job.notes
            .push("Pasar de un formato con pérdida a uno sin pérdida no recupera calidad".into());
    } else if source_format.is_lossy() && format.is_lossy() {
        job.notes
            .push("Se vuelve a comprimir: hay una pequeña pérdida de calidad".into());
    }

    let output = settings
        .output_dir
        .join(&report.rel_path)
        .with_extension(format.output_extension());
    job.action = Action::Convert;
    job.output = Some(unique_output(&output, taken));
    job.target_label = Some(target.describe());
    job.target = Some(target);
    job
}

/// La frecuencia pedida por el usuario si el formato la acepta; si no, la original si es
/// compatible; si no, la más alta compatible de la misma familia (44.1 o 48 kHz) sin
/// pasarse de la original, para que el remuestreo sea lo más limpio posible.
fn resolve_sample_rate(
    source: Option<u32>,
    requested: Option<u32>,
    allowed: &[u32],
    notes: &mut Vec<String>,
) -> u32 {
    if let Some(rate) = requested {
        if allowed.contains(&rate) {
            return rate;
        }
        notes.push(format!(
            "{} no es compatible con este formato; se elige automáticamente",
            khz(rate)
        ));
    }
    let Some(source) = source else {
        return pick(allowed, 44100);
    };
    if allowed.contains(&source) {
        return source;
    }
    let family = |r: u32| if r.is_multiple_of(11025) { 0 } else { 1 };
    allowed
        .iter()
        .copied()
        .filter(|&r| r <= source && family(r) == family(source))
        .max()
        .or_else(|| allowed.iter().copied().filter(|&r| r <= source).max())
        .or_else(|| allowed.iter().copied().min())
        .unwrap_or(44100)
}

fn resolve_bit_depth(
    source: Option<u8>,
    requested: Option<u8>,
    allowed: &[u8],
    notes: &mut Vec<String>,
) -> u8 {
    if let Some(bits) = requested {
        if allowed.contains(&bits) {
            return bits;
        }
        notes.push(format!(
            "{bits} bits no es compatible con este formato; se elige automáticamente"
        ));
    }
    let Some(source) = source else {
        return pick(allowed, 16);
    };
    if allowed.contains(&source) {
        return source;
    }
    allowed
        .iter()
        .copied()
        .filter(|&b| b <= source)
        .max()
        .or_else(|| allowed.iter().copied().min())
        .unwrap_or(16)
}

fn pick<T: Copy + PartialEq>(allowed: &[T], preferred: T) -> T {
    if allowed.contains(&preferred) || allowed.is_empty() {
        preferred
    } else {
        allowed[0]
    }
}

/// Agrega " (2)", " (3)"... si el archivo ya existe o ya fue asignado a otro trabajo, para no
/// sobrescribir nunca nada.
fn unique_output(path: &Path, taken: &mut HashSet<PathBuf>) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("audio")
        .to_string();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    let mut candidate = path.to_path_buf();
    let mut n = 2;
    while taken.contains(&candidate) || candidate.exists() {
        candidate = path.with_file_name(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    taken.insert(candidate.clone());
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::find_profile;
    use crate::probe::AudioInfo;

    fn report(
        name: &str,
        format: AudioFormat,
        rate: u32,
        bits: Option<u8>,
        compatible: bool,
    ) -> FileReport {
        FileReport {
            path: PathBuf::from("/music").join(name),
            root: PathBuf::from("/music"),
            rel_path: PathBuf::from(name),
            info: Some(AudioInfo {
                format,
                codec: String::new(),
                extension: Path::new(name)
                    .extension()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .into(),
                sample_rate: Some(rate),
                bit_depth: bits,
                bitrate_kbps: None,
                channels: Some(2),
                duration_secs: 10.0,
                codec_issue: None,
            }),
            error: None,
            compatible,
            reasons: vec![],
            warnings: vec![],
        }
    }

    fn settings() -> ConvertSettings {
        ConvertSettings {
            output_format: OutputFormat::Keep,
            fallback_format: AudioFormat::Aiff,
            bit_depth: None,
            sample_rate: None,
            lossy_bitrate_kbps: 320,
            convert_all: false,
            copy_compatible: false,
            output_dir: PathBuf::from("/nonexistent-out"),
        }
    }

    #[test]
    fn keeps_wav_and_downsamples_96k_to_48k_for_nexus() {
        let nxs = find_profile("cdj-2000nxs").unwrap();
        let jobs = plan_jobs(
            &[report("a/b.wav", AudioFormat::Wav, 96000, Some(24), false)],
            &nxs,
            &settings(),
        )
        .unwrap();
        let target = jobs[0].target.as_ref().unwrap();
        assert_eq!(jobs[0].action, Action::Convert);
        assert_eq!(target.format, AudioFormat::Wav);
        assert_eq!(target.sample_rate, 48000);
        assert_eq!(target.bit_depth, Some(24));
        assert_eq!(
            jobs[0].output.as_deref(),
            Some(Path::new("/nonexistent-out/a/b.wav"))
        );
    }

    #[test]
    fn flac_falls_back_to_aiff_on_nexus_and_192k_goes_to_44k1_family() {
        let nxs = find_profile("cdj-2000nxs").unwrap();
        let jobs = plan_jobs(
            &[report("x.flac", AudioFormat::Flac, 176400, Some(24), false)],
            &nxs,
            &settings(),
        )
        .unwrap();
        let target = jobs[0].target.as_ref().unwrap();
        assert_eq!(target.format, AudioFormat::Aiff);
        assert_eq!(target.sample_rate, 44100);
        assert_eq!(
            jobs[0].output.as_deref(),
            Some(Path::new("/nonexistent-out/x.aiff"))
        );
    }

    #[test]
    fn user_choice_wins_and_float_32_becomes_24() {
        let cdj3000 = find_profile("cdj-3000").unwrap();
        let mut s = settings();
        s.sample_rate = Some(44100);
        let jobs = plan_jobs(
            &[report("x.wav", AudioFormat::Wav, 48000, Some(32), false)],
            &cdj3000,
            &s,
        )
        .unwrap();
        let target = jobs[0].target.as_ref().unwrap();
        assert_eq!((target.sample_rate, target.bit_depth), (44100, Some(24)));
    }

    #[test]
    fn compatible_files_are_skipped_or_copied() {
        let nxs = find_profile("cdj-2000nxs").unwrap();
        let reports = [report("ok.mp3", AudioFormat::Mp3, 44100, None, true)];
        assert_eq!(
            plan_jobs(&reports, &nxs, &settings()).unwrap()[0].action,
            Action::Skip
        );
        let mut s = settings();
        s.copy_compatible = true;
        assert_eq!(
            plan_jobs(&reports, &nxs, &s).unwrap()[0].action,
            Action::Copy
        );
        s.convert_all = true;
        assert_eq!(
            plan_jobs(&reports, &nxs, &s).unwrap()[0].action,
            Action::Copy
        );
    }

    #[test]
    fn colliding_outputs_get_numbered() {
        let nxs = find_profile("cdj-2000nxs").unwrap();
        let reports = [
            report("song.flac", AudioFormat::Flac, 44100, Some(16), false),
            report("song.wav", AudioFormat::Wav, 96000, Some(16), false),
        ];
        let mut s = settings();
        s.output_format = OutputFormat::Aiff;
        let jobs = plan_jobs(&reports, &nxs, &s).unwrap();
        assert_eq!(
            jobs[0].output.as_deref(),
            Some(Path::new("/nonexistent-out/song.aiff"))
        );
        assert_eq!(
            jobs[1].output.as_deref(),
            Some(Path::new("/nonexistent-out/song (2).aiff"))
        );
    }

    #[test]
    fn rejects_formats_the_cdj_cannot_play() {
        let nxs = find_profile("cdj-2000nxs").unwrap();
        let mut s = settings();
        s.output_format = OutputFormat::Flac;
        assert!(plan_jobs(&[], &nxs, &s).is_err());
    }
}
