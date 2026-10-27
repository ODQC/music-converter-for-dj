//! Búsqueda de archivos de audio en carpetas, archivos sueltos o una USB de rekordbox.

use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::Serialize;
use walkdir::WalkDir;

use crate::analyze::{analyze_file, FileReport};
use crate::profile::CdjProfile;

/// Extensiones que se consideran audio (incluye formatos que los CDJ no leen, para poder
/// convertirlos).
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "m4a", "mp4", "aac", "wav", "aif", "aiff", "aifc", "flac", "fla", "alac", "ogg", "oga",
    "opus", "wma", "wv", "ape", "mpc", "dsf", "dff",
];

/// Carpetas que rekordbox y los sistemas operativos crean en una USB y que no tienen música.
const SKIP_DIRS: &[&str] = &[
    "PIONEER",
    "System Volume Information",
    ".Spotlight-V100",
    ".Trashes",
    ".fseventsd",
    "$RECYCLE.BIN",
];

#[derive(Debug, Clone, Serialize)]
pub struct ScanEntry {
    pub path: PathBuf,
    pub root: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanResult {
    pub entries: Vec<ScanEntry>,
    /// Raíces que parecen una USB exportada desde rekordbox (tienen carpeta PIONEER).
    pub rekordbox_roots: Vec<PathBuf>,
}

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

fn is_hidden(name: &str) -> bool {
    // "._cancion.mp3" son archivos AppleDouble que macOS deja en USB FAT32/exFAT.
    name.starts_with('.')
}

/// Recorre `inputs`. Las carpetas se recorren completas; `exclude` (la carpeta de salida)
/// se salta para no volver a analizar lo ya convertido.
pub fn scan(inputs: &[PathBuf], exclude: Option<&Path>) -> ScanResult {
    let mut entries = Vec::new();
    let mut rekordbox_roots = Vec::new();

    for input in inputs {
        if input.is_file() {
            if is_audio(input) {
                let root = input.parent().map(Path::to_path_buf).unwrap_or_default();
                entries.push(ScanEntry {
                    path: input.clone(),
                    root,
                });
            }
            continue;
        }
        if !input.is_dir() {
            continue;
        }
        if input.join("PIONEER").is_dir() {
            rekordbox_roots.push(input.clone());
        }
        let walker = WalkDir::new(input)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                if e.depth() > 0 && is_hidden(&name) {
                    return false;
                }
                if e.file_type().is_dir() {
                    if SKIP_DIRS.contains(&name.as_ref()) {
                        return false;
                    }
                    if let Some(ex) = exclude {
                        if e.path() == ex {
                            return false;
                        }
                    }
                }
                true
            });
        for entry in walker.flatten() {
            if entry.file_type().is_file() && is_audio(entry.path()) {
                entries.push(ScanEntry {
                    path: entry.into_path(),
                    root: input.clone(),
                });
            }
        }
    }

    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries.dedup_by(|a, b| a.path == b.path);
    ScanResult {
        entries,
        rekordbox_roots,
    }
}

/// Analiza en paralelo todos los archivos encontrados. `on_progress(hechos, total)`.
pub fn analyze_all(
    entries: &[ScanEntry],
    profile: &CdjProfile,
    on_progress: impl Fn(usize, usize) + Sync,
) -> Vec<FileReport> {
    let total = entries.len();
    let done = std::sync::atomic::AtomicUsize::new(0);
    entries
        .par_iter()
        .map(|e| {
            let report = analyze_file(&e.path, &e.root, profile);
            let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            on_progress(n, total);
            report
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_pioneer_hidden_and_output() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for p in [
            "Contents/A/a.mp3",
            "Contents/A/._a.mp3",
            "PIONEER/rekordbox/x.mp3",
            "out/b.wav",
            "c.flac",
            "notes.txt",
        ] {
            let p = root.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"").unwrap();
        }
        let result = scan(&[root.to_path_buf()], Some(&root.join("out")));
        let names: Vec<_> = result
            .entries
            .iter()
            .map(|e| e.path.strip_prefix(root).unwrap().to_path_buf())
            .collect();
        assert_eq!(
            names,
            vec![PathBuf::from("Contents/A/a.mp3"), PathBuf::from("c.flac")]
        );
        assert_eq!(result.rekordbox_roots, vec![root.to_path_buf()]);
    }
}
