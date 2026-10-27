//! Ejecución en lote de los trabajos planeados, en paralelo.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;
use serde::Serialize;

use crate::ffmpeg::{temp_path, Ffmpeg};
use crate::metadata::copy_metadata;
use crate::plan::{Action, Job};
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Converted,
    Copied,
    Skipped,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobEvent {
    pub index: usize,
    /// `None` mientras el trabajo sigue en curso.
    pub status: Option<Status>,
    pub fraction: f64,
    pub output: Option<PathBuf>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub converted: usize,
    pub copied: usize,
    pub skipped: usize,
    pub failed: usize,
    pub cancelled: usize,
}

pub fn run_jobs(
    jobs: &[Job],
    ffmpeg: &Ffmpeg,
    threads: usize,
    cancel: &AtomicBool,
    on_event: impl Fn(JobEvent) + Sync,
) -> Summary {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads.max(1))
        .build()
        .expect("hilos");
    let results: Vec<Status> = pool.install(|| {
        jobs.par_iter()
            .enumerate()
            .map(|(index, job)| {
                let event = |status, fraction, output: Option<PathBuf>, message| {
                    on_event(JobEvent {
                        index,
                        status,
                        fraction,
                        output,
                        message,
                    })
                };
                if job.action == Action::Skip {
                    return Status::Skipped;
                }
                if cancel.load(Ordering::Relaxed) {
                    event(Some(Status::Cancelled), 0.0, None, None);
                    return Status::Cancelled;
                }
                event(None, 0.0, None, None);
                match run_one(job, ffmpeg, cancel, |f| event(None, f, None, None)) {
                    Ok((status, output, warning)) => {
                        event(Some(status), 1.0, Some(output), warning);
                        status
                    }
                    Err(Error::Cancelled) => {
                        event(Some(Status::Cancelled), 0.0, None, None);
                        Status::Cancelled
                    }
                    Err(err) => {
                        event(Some(Status::Failed), 0.0, None, Some(err.to_string()));
                        Status::Failed
                    }
                }
            })
            .collect()
    });
    let count = |s| results.iter().filter(|&&r| r == s).count();
    Summary {
        converted: count(Status::Converted),
        copied: count(Status::Copied),
        skipped: count(Status::Skipped),
        failed: count(Status::Failed),
        cancelled: count(Status::Cancelled),
    }
}

/// Devuelve (estado, ruta final, aviso).
fn run_one(
    job: &Job,
    ffmpeg: &Ffmpeg,
    cancel: &AtomicBool,
    on_progress: impl FnMut(f64),
) -> Result<(Status, PathBuf, Option<String>)> {
    let output = job
        .output
        .as_ref()
        .ok_or_else(|| Error::Invalid("Sin ruta de salida".into()))?;
    if output == &job.source {
        return Err(Error::Invalid(
            "La salida no puede ser el archivo original".into(),
        ));
    }
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = temp_path(output);

    match job.action {
        Action::Copy => {
            std::fs::copy(&job.source, &tmp)?;
            let final_path = finish(&tmp, output)?;
            Ok((Status::Copied, final_path, None))
        }
        Action::Convert => {
            let target = job
                .target
                .as_ref()
                .ok_or_else(|| Error::Invalid("Sin formato de destino".into()))?;
            ffmpeg.convert(
                &job.source,
                output,
                target,
                job.duration_secs,
                cancel,
                on_progress,
            )?;
            let warning = copy_metadata(&job.source, &tmp)
                .err()
                .map(|e| format!("Convertido, pero no se pudieron copiar las etiquetas: {e}"));
            let final_path = finish(&tmp, output)?;
            Ok((Status::Converted, final_path, warning))
        }
        Action::Skip => unreachable!(),
    }
}

/// Renombra el temporal al nombre final sin pisar nunca un archivo existente.
fn finish(tmp: &Path, output: &Path) -> Result<PathBuf> {
    let mut target = output.to_path_buf();
    let stem = output
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("audio")
        .to_string();
    let ext = output
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    let mut n = 2;
    while target.exists() {
        target = output.with_file_name(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    if let Err(err) = std::fs::rename(tmp, &target) {
        let _ = std::fs::remove_file(tmp);
        return Err(err.into());
    }
    Ok(target)
}
