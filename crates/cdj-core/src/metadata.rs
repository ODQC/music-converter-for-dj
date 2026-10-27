//! Copia de etiquetas y portadas del archivo original al convertido.
//!
//! Si el original y el destino usan ID3v2 (MP3, WAV y AIFF) se copia la etiqueta ID3v2
//! completa, con todos sus frames (incluidos los binarios de otros programas de DJ). En los
//! demás casos se traducen los campos conocidos (título, artista, BPM, key, comentarios,
//! portada...) al tipo de etiqueta del formato de destino.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::id3::v2::Id3v2Tag;
use lofty::iff::aiff::AiffFile;
use lofty::iff::wav::WavFile;
use lofty::mpeg::MpegFile;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, Tag, TagExt};

use crate::Result;

fn file_type(path: &Path) -> Result<Option<FileType>> {
    Ok(Probe::open(path)?.guess_file_type()?.file_type())
}

fn uses_id3v2(ft: Option<FileType>) -> bool {
    matches!(ft, Some(FileType::Mpeg | FileType::Wav | FileType::Aiff))
}

fn read_id3v2(path: &Path, ft: FileType) -> Result<Option<Id3v2Tag>> {
    let mut reader = BufReader::new(File::open(path)?);
    let options = ParseOptions::new().read_properties(false);
    Ok(match ft {
        FileType::Mpeg => MpegFile::read_from(&mut reader, options)?.id3v2().cloned(),
        FileType::Wav => WavFile::read_from(&mut reader, options)?.id3v2().cloned(),
        FileType::Aiff => AiffFile::read_from(&mut reader, options)?.id3v2().cloned(),
        _ => None,
    })
}

/// Copia las etiquetas de `source` a `dest`. `dest` puede tener cualquier nombre (se detecta
/// el formato por el contenido).
pub fn copy_metadata(source: &Path, dest: &Path) -> Result<()> {
    let source_type = file_type(source)?;
    let dest_type = file_type(dest)?;

    if uses_id3v2(source_type) && uses_id3v2(dest_type) {
        if let Some(tag) = read_id3v2(source, source_type.unwrap())? {
            if !tag.is_empty() {
                tag.save_to_path(dest, WriteOptions::default())?;
                return Ok(());
            }
        }
    }

    let Some(dest_type) = dest_type else {
        return Ok(());
    };
    let source_file = Probe::open(source)?.guess_file_type()?.read()?;

    // Empezamos por la etiqueta principal y completamos con lo que tengan las demás
    // (p. ej. RIFF INFO en un WAV sin ID3v2).
    let mut tags: Vec<&Tag> = source_file.tags().iter().collect();
    let primary = source_file.primary_tag_type();
    tags.sort_by_key(|t| t.tag_type() != primary);

    let mut out = Tag::new(dest_type.primary_tag_type());
    let mut bpm = None;
    for tag in tags {
        for item in tag.items() {
            if matches!(item.key(), ItemKey::Bpm | ItemKey::IntegerBpm) && bpm.is_none() {
                bpm = item.value().text().map(str::to_owned);
            }
            if out.get(item.key()).is_none() {
                out.push(item.clone());
            }
        }
        if out.pictures().is_empty() {
            for picture in tag.pictures() {
                out.push_picture(picture.clone());
            }
        }
    }

    // ID3v2 y MP4 guardan el BPM como entero (TBPM, tmpo) y Vorbis como texto ("BPM"); lofty
    // los trata como claves distintas, así que escribimos las dos para que no se pierda.
    if let Some(bpm) = bpm {
        if out.get(ItemKey::Bpm).is_none() {
            out.insert_text(ItemKey::Bpm, bpm.clone());
        }
        if out.get(ItemKey::IntegerBpm).is_none() {
            if let Ok(value) = bpm.trim().parse::<f64>() {
                out.insert_text(ItemKey::IntegerBpm, (value.round() as u32).to_string());
            }
        }
    }

    if !out.is_empty() {
        out.save_to_path(dest, WriteOptions::default())?;
    }
    Ok(())
}
