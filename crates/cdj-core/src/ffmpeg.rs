//! Llamadas a FFmpeg: ubicar el binario, armar los argumentos de cada conversión y seguir
//! el progreso.

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::plan::TargetSpec;
use crate::{AudioFormat, Error, Result};

#[derive(Debug, Clone)]
pub struct Ffmpeg {
    pub path: PathBuf,
    pub version: String,
    /// Si está compilado con libsoxr (remuestreo de mejor calidad).
    pub has_soxr: bool,
}

impl Ffmpeg {
    /// Busca FFmpeg en `candidates` (p. ej. el que viene dentro de la app) y luego en el PATH
    /// y en las carpetas habituales de Homebrew.
    pub fn locate(candidates: &[PathBuf]) -> Option<Ffmpeg> {
        let exe = if cfg!(windows) {
            "ffmpeg.exe"
        } else {
            "ffmpeg"
        };
        let mut all: Vec<PathBuf> = candidates.to_vec();
        all.push(PathBuf::from(exe));
        if cfg!(target_os = "macos") {
            all.push(PathBuf::from("/opt/homebrew/bin/ffmpeg"));
            all.push(PathBuf::from("/usr/local/bin/ffmpeg"));
        }
        all.into_iter().find_map(|p| Self::from_path(p).ok())
    }

    pub fn from_path(path: PathBuf) -> Result<Ffmpeg> {
        let output = command(&path).args(["-hide_banner", "-version"]).output()?;
        if !output.status.success() {
            return Err(Error::Ffmpeg("no responde".into()));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let version = text.lines().next().unwrap_or_default().to_string();
        let has_soxr = text.contains("--enable-libsoxr");
        Ok(Ffmpeg {
            path,
            version,
            has_soxr,
        })
    }

    /// Convierte `input` en `output` según `target`. Escribe primero un archivo temporal y lo
    /// renombra al final, así un corte a medias nunca deja un archivo roto con el nombre final.
    /// `on_progress` recibe el avance de 0.0 a 1.0.
    pub fn convert(
        &self,
        input: &Path,
        output: &Path,
        target: &TargetSpec,
        duration_secs: f64,
        cancel: &AtomicBool,
        mut on_progress: impl FnMut(f64),
    ) -> Result<()> {
        let tmp = temp_path(output);
        let _ = std::fs::remove_file(&tmp);
        let mut child = command(&self.path)
            .args(build_args(input, &tmp, target, self.has_soxr))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        // stderr se lee en otro hilo para que FFmpeg no se bloquee si se llena el búfer.
        let mut stderr = child.stderr.take().expect("stderr");
        let stderr_reader = std::thread::spawn(move || {
            let mut buf = String::new();
            let _ = stderr.read_to_string(&mut buf);
            buf
        });

        let stdout = child.stdout.take().expect("stdout");
        let mut cancelled = false;
        for line in BufReader::new(stdout).lines() {
            if cancel.load(Ordering::Relaxed) {
                cancelled = true;
                let _ = child.kill();
                break;
            }
            let Ok(line) = line else { break };
            if let Some(us) = line
                .strip_prefix("out_time_us=")
                .and_then(|v| v.trim().parse::<f64>().ok())
            {
                if duration_secs > 0.0 {
                    on_progress((us / 1_000_000.0 / duration_secs).clamp(0.0, 1.0));
                }
            }
        }

        let status = child.wait()?;
        let log = stderr_reader.join().unwrap_or_default();
        if cancelled || cancel.load(Ordering::Relaxed) {
            let _ = std::fs::remove_file(&tmp);
            return Err(Error::Cancelled);
        }
        if !status.success() {
            let _ = std::fs::remove_file(&tmp);
            let tail: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).collect();
            let tail = tail[tail.len().saturating_sub(3)..].join(" | ");
            return Err(Error::Ffmpeg(tail));
        }

        if target.format == AudioFormat::Wav {
            if let Err(err) = fix_wav_header(&tmp) {
                let _ = std::fs::remove_file(&tmp);
                return Err(err);
            }
        }
        on_progress(1.0);
        Ok(())
    }
}

/// Ruta del archivo temporal: oculto y en la misma carpeta que el destino.
pub fn temp_path(output: &Path) -> PathBuf {
    let name = output
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("audio");
    output.with_file_name(format!(".{name}.part"))
}

fn command(path: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Sin ventana de consola en Windows.
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

pub fn build_args(input: &Path, output: &Path, target: &TargetSpec, has_soxr: bool) -> Vec<String> {
    let bits = target.bit_depth.unwrap_or(16);
    // (códec, formato de muestra que pide el códec, muxer)
    let (codec, sample_fmt, muxer) = match target.format {
        AudioFormat::Wav => (
            if bits == 24 { "pcm_s24le" } else { "pcm_s16le" },
            if bits == 24 { "s32" } else { "s16" },
            "wav",
        ),
        AudioFormat::Aiff => (
            if bits == 24 { "pcm_s24be" } else { "pcm_s16be" },
            if bits == 24 { "s32" } else { "s16" },
            "aiff",
        ),
        AudioFormat::Flac => ("flac", if bits == 24 { "s32" } else { "s16" }, "flac"),
        AudioFormat::Alac => ("alac", if bits == 24 { "s32p" } else { "s16p" }, "ipod"),
        AudioFormat::Mp3 => ("libmp3lame", "fltp", "mp3"),
        AudioFormat::Aac => ("aac", "fltp", "ipod"),
        AudioFormat::Other => unreachable!("no se convierte a un formato desconocido"),
    };

    // Remuestreo y reducción de bits con dither triangular. Con soxr usamos su máxima
    // precisión; si no está, el remuestreador de FFmpeg con un filtro largo.
    let resampler = if has_soxr {
        "resampler=soxr:precision=28"
    } else {
        "filter_size=64:phase_shift=10:cutoff=0.97"
    };
    let filter = format!(
        "aresample={resampler}:osr={}:osf={sample_fmt}:dither_method=triangular",
        target.sample_rate
    );

    let mut args: Vec<String> = [
        "-hide_banner",
        "-nostdin",
        "-n",
        "-nostats",
        "-progress",
        "pipe:1",
        "-i",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.push(input.to_string_lossy().into_owned());
    args.extend(
        [
            "-map",
            "0:a:0",
            "-map_metadata",
            "-1",
            "-map_chapters",
            "-1",
            "-fflags",
            "+bitexact",
            "-flags:a",
            "+bitexact",
            "-af",
            &filter,
            "-ac",
            &target.channels.to_string(),
            "-c:a",
            codec,
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    if bits == 24 && matches!(target.format, AudioFormat::Flac | AudioFormat::Alac) {
        args.extend(["-bits_per_raw_sample".into(), "24".into()]);
    }
    if let Some(kbps) = target.bitrate_kbps {
        args.extend(["-b:a".into(), format!("{kbps}k")]);
    }
    if target.format == AudioFormat::Mp3 {
        // Sin ID3 de FFmpeg: las etiquetas se copian después desde el original.
        args.extend([
            "-id3v2_version".into(),
            "0".into(),
            "-write_id3v1".into(),
            "0".into(),
        ]);
    }
    args.extend([
        "-f".into(),
        muxer.into(),
        output.to_string_lossy().into_owned(),
    ]);
    args
}

const WAVE_FORMAT_PCM: u16 = 0x0001;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// FFmpeg escribe los WAV de 24 bits o de más de 48 kHz con cabecera WAVE_FORMAT_EXTENSIBLE,
/// que algunos CDJ antiguos no leen. La cambiamos en el mismo lugar por una cabecera PCM
/// clásica (16 bytes) precedida de un bloque JUNK que ocupa el espacio sobrante.
pub fn fix_wav_header(path: &Path) -> Result<()> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let mut riff = [0u8; 12];
    file.read_exact(&mut riff)?;
    if &riff[0..4] != b"RIFF" || &riff[8..12] != b"WAVE" {
        return Ok(()); // RF64 u otro: lo dejamos como está.
    }
    loop {
        let chunk_start = file.stream_position()?;
        let mut header = [0u8; 8];
        if file.read_exact(&mut header).is_err() {
            return Ok(());
        }
        let size = u32::from_le_bytes(header[4..8].try_into().unwrap()) as u64;
        if &header[0..4] != b"fmt " {
            file.seek(SeekFrom::Current((size + size % 2) as i64))?;
            continue;
        }
        if size != 40 {
            return Ok(());
        }
        let mut fmt = [0u8; 40];
        file.read_exact(&mut fmt)?;
        let tag = u16::from_le_bytes([fmt[0], fmt[1]]);
        // El subformato (GUID) empieza con el código de formato real.
        let sub = u16::from_le_bytes([fmt[24], fmt[25]]);
        if tag != WAVE_FORMAT_EXTENSIBLE || sub != WAVE_FORMAT_PCM {
            return Ok(());
        }
        let mut out = Vec::with_capacity(48);
        out.extend_from_slice(b"JUNK");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&[0u8; 16]);
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&WAVE_FORMAT_PCM.to_le_bytes());
        out.extend_from_slice(&fmt[2..16]); // canales, frecuencia, bytes/s, alineación, bits
        debug_assert_eq!(out.len(), 48);
        file.seek(SeekFrom::Start(chunk_start))?;
        file.write_all(&out)?;
        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_24_args() {
        let target = TargetSpec {
            format: AudioFormat::Wav,
            sample_rate: 48000,
            bit_depth: Some(24),
            bitrate_kbps: None,
            channels: 2,
        };
        let args = build_args(Path::new("in.flac"), Path::new("out.part"), &target, true).join(" ");
        assert!(args.contains("-c:a pcm_s24le"));
        assert!(args.contains(
            "aresample=resampler=soxr:precision=28:osr=48000:osf=s32:dither_method=triangular"
        ));
        assert!(args.contains("-n "), "nunca sobrescribe");
        assert!(args.ends_with("-f wav out.part"));
    }

    #[test]
    fn mp3_args() {
        let target = TargetSpec {
            format: AudioFormat::Mp3,
            sample_rate: 44100,
            bit_depth: None,
            bitrate_kbps: Some(320),
            channels: 2,
        };
        let args = build_args(Path::new("in.ogg"), Path::new("out.part"), &target, false).join(" ");
        assert!(args.contains("-c:a libmp3lame"));
        assert!(args.contains("-b:a 320k"));
        assert!(args.contains("filter_size=64"));
    }
}
