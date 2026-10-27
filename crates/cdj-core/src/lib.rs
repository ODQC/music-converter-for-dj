//! Núcleo de CDJ Converter: escanea carpetas, revisa cada archivo contra el perfil del
//! CDJ elegido y convierte con FFmpeg los que no son compatibles, copiando etiquetas y
//! portadas al archivo nuevo. Nunca modifica los originales.

pub mod analyze;
pub mod convert;
pub mod ffmpeg;
pub mod metadata;
pub mod plan;
pub mod probe;
pub mod profile;
pub mod scan;

use serde::{Deserialize, Serialize};

pub use analyze::{analyze_file, FileReport};
pub use plan::{plan_jobs, Action, ConvertSettings, Job, OutputFormat};
pub use probe::{probe_file, AudioInfo};
pub use profile::{builtin_profiles, find_profile, CdjProfile};

/// Formatos de audio que entienden los CDJ. Lo demás (OGG, Opus, WMA...) se trata como
/// `Other` y siempre se convierte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    Mp3,
    Aac,
    Wav,
    Aiff,
    Flac,
    Alac,
    Other,
}

impl AudioFormat {
    pub fn label(self) -> &'static str {
        match self {
            AudioFormat::Mp3 => "MP3",
            AudioFormat::Aac => "AAC",
            AudioFormat::Wav => "WAV",
            AudioFormat::Aiff => "AIFF",
            AudioFormat::Flac => "FLAC",
            AudioFormat::Alac => "ALAC",
            AudioFormat::Other => "Otro",
        }
    }

    pub fn is_lossy(self) -> bool {
        matches!(self, AudioFormat::Mp3 | AudioFormat::Aac)
    }

    /// Extensión que usamos al escribir un archivo de este formato.
    pub fn output_extension(self) -> &'static str {
        match self {
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Aac | AudioFormat::Alac => "m4a",
            AudioFormat::Wav => "wav",
            AudioFormat::Aiff => "aiff",
            AudioFormat::Flac => "flac",
            AudioFormat::Other => "bin",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("No se pudo leer el archivo: {0}")]
    Parse(#[from] lofty::error::FileParseError),
    #[error("No se pudieron escribir las etiquetas: {0}")]
    Encode(#[from] lofty::error::FileEncodingError),
    #[error("FFmpeg falló: {0}")]
    Ffmpeg(String),
    #[error("Conversión cancelada")]
    Cancelled,
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;
