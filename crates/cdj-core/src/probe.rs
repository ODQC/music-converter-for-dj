//! Lectura de las propiedades técnicas de un archivo de audio (formato, códec, frecuencia
//! de muestreo, bits, bitrate) sin decodificar el audio.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use lofty::aac::AacFile;
use lofty::config::ParseOptions;
use lofty::file::{AudioFile, FileType};
use lofty::flac::FlacFile;
use lofty::iff::aiff::{AiffCompressionType, AiffFile};
use lofty::iff::wav::{WavFile, WavFormat};
use lofty::mp4::{AudioObjectType, Mp4Codec, Mp4File};
use lofty::mpeg::{Layer, MpegFile, MpegVersion};
use lofty::probe::Probe;
use serde::{Deserialize, Serialize};

use crate::{AudioFormat, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioInfo {
    pub format: AudioFormat,
    /// Descripción del códec para mostrar, p. ej. "PCM 32 bits flotante" o "MPEG-1 Layer 3".
    pub codec: String,
    /// Extensión del archivo en minúsculas, sin punto.
    pub extension: String,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub bitrate_kbps: Option<u32>,
    pub channels: Option<u8>,
    pub duration_secs: f64,
    /// Problema del códec que ningún CDJ acepta aunque el contenedor sí (PCM flotante,
    /// AAC que no es LC, MP3 que no es Layer 3, AIFF comprimido, DRM...).
    pub codec_issue: Option<String>,
}

fn options() -> ParseOptions {
    ParseOptions::new().read_tags(false).read_cover_art(false)
}

fn open(path: &Path) -> Result<BufReader<File>> {
    Ok(BufReader::new(File::open(path)?))
}

pub fn probe_file(path: &Path) -> Result<AudioInfo> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase();
    let file_type = Probe::open(path)?.guess_file_type()?.file_type();

    let mut info = AudioInfo {
        format: AudioFormat::Other,
        codec: String::new(),
        extension,
        sample_rate: None,
        bit_depth: None,
        bitrate_kbps: None,
        channels: None,
        duration_secs: 0.0,
        codec_issue: None,
    };

    match file_type {
        Some(FileType::Mpeg) => {
            let file = MpegFile::read_from(&mut open(path)?, options())?;
            let p = file.properties();
            let version = match p.version() {
                MpegVersion::V1 => "MPEG-1",
                MpegVersion::V2 => "MPEG-2",
                MpegVersion::V2_5 => "MPEG-2.5",
                MpegVersion::V4 => "MPEG-4",
            };
            let layer = match p.layer() {
                Layer::Layer1 => 1,
                Layer::Layer2 => 2,
                Layer::Layer3 => 3,
            };
            info.format = AudioFormat::Mp3;
            info.codec = format!("{version} Layer {layer}");
            if layer != 3 {
                info.codec_issue = Some(format!("Es MPEG Layer {layer}, no MP3 (Layer 3)"));
            }
            info.sample_rate = Some(p.sample_rate());
            info.bitrate_kbps = Some(p.audio_bitrate());
            info.channels = Some(p.channels());
            info.duration_secs = p.duration().as_secs_f64();
        }
        Some(FileType::Aac) => {
            let file = AacFile::read_from(&mut open(path)?, options())?;
            let p = file.properties();
            info.format = AudioFormat::Aac;
            set_aac_codec(&mut info, p.audio_object_type());
            info.sample_rate = Some(p.sample_rate());
            info.bitrate_kbps = Some(p.audio_bitrate());
            info.channels = Some(p.channels());
            info.duration_secs = p.duration().as_secs_f64();
        }
        Some(FileType::Mp4) => {
            let file = Mp4File::read_from(&mut open(path)?, options())?;
            let p = file.properties();
            match p.codec() {
                Some(Mp4Codec::AAC) => {
                    info.format = AudioFormat::Aac;
                    set_aac_codec(
                        &mut info,
                        p.audio_object_type()
                            .unwrap_or(AudioObjectType::AacLowComplexity),
                    );
                }
                Some(Mp4Codec::ALAC) => {
                    info.format = AudioFormat::Alac;
                    info.codec = "ALAC".into();
                    info.bit_depth = p.bit_depth();
                }
                Some(Mp4Codec::MP3) => {
                    info.codec = "MP3 dentro de MP4".into();
                    info.codec_issue = Some("MP3 dentro de un contenedor MP4".into());
                }
                Some(Mp4Codec::FLAC) => {
                    info.codec = "FLAC dentro de MP4".into();
                    info.codec_issue = Some("FLAC dentro de un contenedor MP4".into());
                }
                _ => {
                    info.codec = "Desconocido (MP4)".into();
                    info.codec_issue = Some("Códec MP4 no reconocido".into());
                }
            }
            if p.is_drm_protected() {
                info.codec_issue =
                    Some("Protegido con DRM (no se puede reproducir ni convertir)".into());
            }
            info.sample_rate = p.sample_rate();
            info.bitrate_kbps = p.audio_bitrate();
            info.channels = p.channels();
            info.duration_secs = p.duration().as_secs_f64();
        }
        Some(FileType::Wav) => {
            let file = WavFile::read_from(&mut open(path)?, options())?;
            let p = file.properties();
            info.format = AudioFormat::Wav;
            info.bit_depth = Some(p.bit_depth());
            info.bitrate_kbps = Some(p.bitrate());
            match p.format() {
                WavFormat::PCM => info.codec = format!("PCM {} bits", p.bit_depth()),
                WavFormat::IEEE_FLOAT => {
                    info.codec = format!("PCM {} bits flotante", p.bit_depth());
                    info.codec_issue =
                        Some("PCM de punto flotante (los CDJ solo leen PCM entero)".into());
                }
                WavFormat::Other(tag) => {
                    info.codec = format!("WAV comprimido (0x{tag:04X})");
                    info.codec_issue = Some("WAV comprimido (no es PCM)".into());
                }
            }
            info.sample_rate = Some(p.sample_rate());
            info.channels = Some(p.channels());
            info.duration_secs = p.duration().as_secs_f64();
        }
        Some(FileType::Aiff) => {
            let file = AiffFile::read_from(&mut open(path)?, options())?;
            let p = file.properties();
            info.format = AudioFormat::Aiff;
            info.bit_depth = u8::try_from(p.sample_size()).ok();
            info.codec = format!("PCM {} bits", p.sample_size());
            match p.compression_type() {
                None | Some(AiffCompressionType::None) => {}
                Some(AiffCompressionType::FL32) => {
                    info.codec = "PCM 32 bits flotante".into();
                    info.codec_issue =
                        Some("AIFF de punto flotante (los CDJ solo leen PCM entero)".into());
                }
                Some(other) => {
                    info.codec = format!("AIFF-C ({})", other.compression_name());
                    // "sowt" es PCM little-endian: lo tratamos como comprimido por precaución.
                    info.codec_issue =
                        Some(format!("AIFF comprimido ({})", other.compression_name()));
                }
            }
            info.sample_rate = Some(p.sample_rate());
            info.bitrate_kbps = Some(p.audio_bitrate());
            info.channels = u8::try_from(p.channels()).ok();
            info.duration_secs = p.duration().as_secs_f64();
        }
        Some(FileType::Flac) => {
            let file = FlacFile::read_from(&mut open(path)?, options())?;
            let p = file.properties();
            info.format = AudioFormat::Flac;
            info.codec = "FLAC".into();
            info.bit_depth = Some(p.bit_depth());
            info.sample_rate = Some(p.sample_rate());
            info.bitrate_kbps = Some(p.audio_bitrate());
            info.channels = Some(p.channels());
            info.duration_secs = p.duration().as_secs_f64();
        }
        other => {
            // Vorbis, Opus, WavPack, APE... los lee lofty pero ningún CDJ los reproduce.
            // WMA y otros que lofty no conoce también caen aquí: FFmpeg los convertirá.
            info.codec = match other {
                Some(FileType::Vorbis) => "Ogg Vorbis".into(),
                Some(FileType::Opus) => "Opus".into(),
                Some(FileType::WavPack) => "WavPack".into(),
                Some(FileType::Ape) => "Monkey's Audio".into(),
                Some(FileType::Mpc) => "Musepack".into(),
                _ => info.extension.to_uppercase(),
            };
            if let Some(ft) = other {
                if let Ok(tagged) = Probe::open(path)?
                    .options(options())
                    .set_file_type(ft)
                    .read()
                {
                    let p = tagged.properties();
                    info.sample_rate = p.sample_rate();
                    info.bit_depth = p.bit_depth();
                    info.bitrate_kbps = p.audio_bitrate();
                    info.channels = p.channels();
                    info.duration_secs = p.duration().as_secs_f64();
                }
            }
        }
    }

    Ok(info)
}

fn set_aac_codec(info: &mut AudioInfo, aot: AudioObjectType) {
    match aot {
        AudioObjectType::AacLowComplexity => info.codec = "AAC LC".into(),
        AudioObjectType::SpectralBandReplication => {
            info.codec = "HE-AAC".into();
            info.codec_issue = Some("HE-AAC (los CDJ solo leen AAC LC)".into());
        }
        other => {
            info.codec = format!("AAC ({other:?})");
            info.codec_issue = Some("Perfil AAC distinto de AAC LC".into());
        }
    }
}
