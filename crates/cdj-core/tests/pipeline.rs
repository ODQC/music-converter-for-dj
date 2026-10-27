//! Prueba de punta a punta con FFmpeg real: genera archivos, los etiqueta, los analiza,
//! los convierte y revisa el resultado. Se omite si FFmpeg no está instalado.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;

use cdj_core::convert::{run_jobs, Status};
use cdj_core::ffmpeg::Ffmpeg;
use cdj_core::plan::{plan_jobs, Action, ConvertSettings, OutputFormat};
use cdj_core::scan::{analyze_all, scan};
use cdj_core::{find_profile, probe_file, AudioFormat};
use lofty::config::WriteOptions;
use lofty::file::TaggedFileExt;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::tag::{Tag, TagType};

fn ffmpeg() -> Option<Ffmpeg> {
    Ffmpeg::locate(&[])
}

fn generate(ff: &Ffmpeg, out: &Path, args: &[&str]) {
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    let status = Command::new(&ff.path)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2",
        ])
        .args(args)
        .arg(out)
        .status()
        .unwrap();
    assert!(status.success());
}

fn cover(ff: &Ffmpeg, dir: &Path) -> Vec<u8> {
    let path = dir.join("cover.jpg");
    let status = Command::new(&ff.path)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=red:s=64x64",
            "-frames:v",
            "1",
        ])
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::read(path).unwrap()
}

fn tag(path: &Path, tag_type: TagType, jpeg: &[u8]) {
    let mut tag = Tag::new(tag_type);
    tag.set_artist("Artista Prueba".into());
    tag.set_title("Canción Ñandú".into());
    tag.insert_text(
        if tag_type == TagType::Id3v2 {
            ItemKey::IntegerBpm
        } else {
            ItemKey::Bpm
        },
        "128".into(),
    );
    tag.insert_text(ItemKey::InitialKey, "8A".into());
    tag.insert_text(ItemKey::Comment, "Comentario DJ".into());
    tag.push_picture(
        Picture::unchecked(jpeg.to_vec())
            .pic_type(PictureType::CoverFront)
            .mime_type(MimeType::Jpeg)
            .build(),
    );
    tag.save_to_path(path, WriteOptions::default()).unwrap();
}

fn assert_tags(path: &Path) {
    let file = lofty::probe::Probe::open(path)
        .unwrap()
        .guess_file_type()
        .unwrap()
        .read()
        .unwrap();
    let tag = file
        .primary_tag()
        .or_else(|| file.first_tag())
        .unwrap_or_else(|| panic!("sin etiquetas: {}", path.display()));
    assert_eq!(
        tag.artist().as_deref(),
        Some("Artista Prueba"),
        "{}",
        path.display()
    );
    assert_eq!(tag.title().as_deref(), Some("Canción Ñandú"));
    assert_eq!(
        tag.get_string(ItemKey::IntegerBpm)
            .or(tag.get_string(ItemKey::Bpm)),
        Some("128"),
        "BPM en {}",
        path.display()
    );
    assert_eq!(tag.get_string(ItemKey::InitialKey), Some("8A"));
    assert_eq!(tag.pictures().len(), 1, "portada en {}", path.display());
}

fn wav_format_tag(path: &Path) -> u16 {
    let bytes = std::fs::read(path).unwrap();
    let pos = bytes.windows(4).position(|w| w == b"fmt ").unwrap();
    u16::from_le_bytes([bytes[pos + 8], bytes[pos + 9]])
}

fn settings(out: PathBuf) -> ConvertSettings {
    ConvertSettings {
        output_format: OutputFormat::Keep,
        fallback_format: AudioFormat::Aiff,
        bit_depth: None,
        sample_rate: None,
        lossy_bitrate_kbps: 320,
        convert_all: false,
        copy_compatible: true,
        output_dir: out,
    }
}

#[test]
fn converts_library_for_cdj_2000nxs() {
    let Some(ff) = ffmpeg() else {
        eprintln!("FFmpeg no encontrado: prueba omitida");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let usb = dir.path().join("USB");
    let jpeg = cover(&ff, dir.path());

    let hires = usb.join("Contents/Artista/hires.wav");
    generate(&ff, &hires, &["-ar", "96000", "-c:a", "pcm_s24le"]);
    tag(&hires, TagType::Id3v2, &jpeg);

    let float = usb.join("Contents/Artista/float.wav");
    generate(&ff, &float, &["-ar", "44100", "-c:a", "pcm_f32le"]);
    tag(&float, TagType::Id3v2, &jpeg);

    let flac = usb.join("Contents/Otro/tema.flac");
    generate(&ff, &flac, &["-ar", "44100", "-sample_fmt", "s16"]);
    tag(&flac, TagType::VorbisComments, &jpeg);

    let ok = usb.join("Contents/Otro/ok.mp3");
    generate(&ff, &ok, &["-ar", "44100", "-b:a", "320k"]);

    std::fs::create_dir_all(usb.join("PIONEER/rekordbox")).unwrap();
    let original = std::fs::read(&hires).unwrap();

    let out = dir.path().join("Salida");
    let profile = find_profile("cdj-2000nxs").unwrap();
    let scanned = scan(std::slice::from_ref(&usb), Some(&out));
    assert_eq!(scanned.entries.len(), 4);
    assert_eq!(scanned.rekordbox_roots, vec![usb.clone()]);

    let reports = analyze_all(&scanned.entries, &profile, |_, _| {});
    let incompatible: Vec<_> = reports
        .iter()
        .filter(|r| !r.compatible)
        .map(|r| r.path.file_name().unwrap().to_owned())
        .collect();
    assert_eq!(incompatible.len(), 3, "{reports:#?}");

    let jobs = plan_jobs(&reports, &profile, &settings(out.clone())).unwrap();
    assert_eq!(jobs.iter().filter(|j| j.action == Action::Copy).count(), 1);

    let summary = run_jobs(&jobs, &ff, 2, &AtomicBool::new(false), |_| {});
    assert_eq!(
        (summary.converted, summary.copied, summary.failed),
        (3, 1, 0),
        "{summary:?}"
    );

    // El original queda intacto.
    assert_eq!(std::fs::read(&hires).unwrap(), original);

    let hires_out = out.join("Contents/Artista/hires.wav");
    let info = probe_file(&hires_out).unwrap();
    assert_eq!((info.sample_rate, info.bit_depth), (Some(48000), Some(24)));
    assert_eq!(wav_format_tag(&hires_out), 1, "cabecera PCM clásica");
    let decoded = Command::new(&ff.path)
        .args(["-v", "error", "-i"])
        .arg(&hires_out)
        .args(["-f", "null", "-"])
        .output()
        .unwrap();
    assert!(
        decoded.status.success() && decoded.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&decoded.stderr)
    );
    assert_tags(&hires_out);

    let float_out = out.join("Contents/Artista/float.wav");
    let info = probe_file(&float_out).unwrap();
    assert_eq!((info.bit_depth, info.codec_issue.clone()), (Some(24), None));
    assert_tags(&float_out);

    let aiff_out = out.join("Contents/Otro/tema.aiff");
    let info = probe_file(&aiff_out).unwrap();
    assert_eq!(
        (info.format, info.sample_rate, info.bit_depth),
        (AudioFormat::Aiff, Some(44100), Some(16))
    );
    assert_tags(&aiff_out);

    assert!(out.join("Contents/Otro/ok.mp3").exists());

    // Todo lo convertido es compatible.
    let again = analyze_all(
        &scan(std::slice::from_ref(&out), None).entries,
        &profile,
        |_, _| {},
    );
    assert!(again.iter().all(|r| r.compatible), "{again:#?}");

    // Una segunda pasada no pisa nada: numera los archivos nuevos.
    let summary = run_jobs(
        &plan_jobs(&reports, &profile, &settings(out.clone())).unwrap(),
        &ff,
        2,
        &AtomicBool::new(false),
        |_| {},
    );
    assert_eq!(summary.failed, 0);
    assert!(out.join("Contents/Artista/hires (2).wav").exists());
}

#[test]
fn converts_to_alac_mp3_and_flac_for_cdj_3000() {
    let Some(ff) = ffmpeg() else { return };
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("in");
    let jpeg = cover(&ff, dir.path());

    let flac = src.join("hires.flac");
    generate(
        &ff,
        &flac,
        &[
            "-ar",
            "192000",
            "-sample_fmt",
            "s32",
            "-bits_per_raw_sample",
            "24",
        ],
    );
    tag(&flac, TagType::VorbisComments, &jpeg);

    let ogg = src.join("tema.ogg");
    generate(&ff, &ogg, &["-ar", "44100", "-c:a", "libvorbis"]);

    let profile = find_profile("cdj-3000").unwrap();
    let reports = analyze_all(
        &scan(std::slice::from_ref(&src), None).entries,
        &profile,
        |_, _| {},
    );
    assert!(reports.iter().all(|r| !r.compatible));

    for (format, ext) in [
        (OutputFormat::Alac, "m4a"),
        (OutputFormat::Flac, "flac"),
        (OutputFormat::Mp3, "mp3"),
    ] {
        let out = dir.path().join(format!("out-{ext}"));
        let mut s = settings(out.clone());
        s.output_format = format;
        let jobs = plan_jobs(&reports, &profile, &s).unwrap();
        let summary = run_jobs(&jobs, &ff, 2, &AtomicBool::new(false), |e| {
            if e.status == Some(Status::Failed) {
                panic!("{:?}", e.message)
            }
        });
        assert_eq!(summary.converted, 2);
        let hires = out.join(format!("hires.{ext}"));
        let info = probe_file(&hires).unwrap();
        assert!(info.codec_issue.is_none(), "{info:?}");
        let expected_rate = if ext == "mp3" { 48000 } else { 96000 };
        assert_eq!(info.sample_rate, Some(expected_rate), "{ext}");
        if ext != "mp3" {
            assert_eq!(info.bit_depth, Some(24), "{ext}");
        }
        assert_tags(&hires);
        let again = analyze_all(
            &scan(std::slice::from_ref(&out), None).entries,
            &profile,
            |_, _| {},
        );
        assert!(again.iter().all(|r| r.compatible), "{again:#?}");
    }
}
