import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api } from "./api";
import { describeInfo, duration, fileName, FORMAT_LABELS, khz, LOSSY } from "./format";
import type { AudioFormat, CdjProfile, ConvertSettings, FfmpegInfo, FileReport, Job, JobEvent, OutputFormat, Summary } from "./types";

type Filter = "all" | "incompatible" | "compatible";
type Theme = "dark" | "light";
type RowState = { status: JobEvent["status"]; fraction: number; message: string | null; output: string | null; running: boolean };

const AUDIO_EXTENSIONS = ["mp3", "m4a", "mp4", "aac", "wav", "aif", "aiff", "aifc", "flac", "fla", "alac", "ogg", "oga", "opus", "wma", "wv", "ape", "mpc", "dsf", "dff"];

const DEFAULT_SETTINGS: ConvertSettings = {
  output_format: "keep",
  fallback_format: "aiff",
  bit_depth: null,
  sample_rate: null,
  lossy_bitrate_kbps: 320,
  convert_all: false,
  copy_compatible: false,
  output_dir: "",
};

function load<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : fallback;
  } catch {
    return fallback;
  }
}

function save(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* sin almacenamiento: no pasa nada */
  }
}

const supportedFormats = (p: CdjProfile | undefined) => (p ? (Object.keys(p.formats) as AudioFormat[]) : []);

function sampleRates(p: CdjProfile | undefined, format: OutputFormat): number[] {
  if (!p) return [];
  const formats = format === "keep" ? supportedFormats(p) : [format];
  const set = new Set<number>();
  formats.forEach((f) => p.formats[f]?.variants.forEach((v) => v.sample_rates.forEach((r) => r >= 32000 && set.add(r))));
  return [...set].sort((a, b) => a - b);
}

function bitDepths(p: CdjProfile | undefined, format: OutputFormat): number[] {
  if (!p) return [];
  const formats = format === "keep" ? supportedFormats(p) : [format];
  const set = new Set<number>();
  formats.forEach((f) => p.formats[f]?.variants.forEach((v) => v.bit_depths.forEach((b) => set.add(b))));
  return [...set].sort((a, b) => a - b);
}

export default function App() {
  const [theme, setTheme] = useState<Theme>(() => load("theme", "dark"));
  const [profiles, setProfiles] = useState<CdjProfile[]>([]);
  const [profileId, setProfileId] = useState<string>(() => load("profileId", "cdj-2000nxs"));
  const [settings, setSettings] = useState<ConvertSettings>(() => ({ ...DEFAULT_SETTINGS, ...load("settings", {}), output_dir: "" }));
  const [outputTouched, setOutputTouched] = useState(false);
  const [ffmpeg, setFfmpeg] = useState<FfmpegInfo | null | undefined>(undefined);

  const [inputs, setInputs] = useState<string[]>([]);
  const [reports, setReports] = useState<FileReport[]>([]);
  const [rekordboxRoots, setRekordboxRoots] = useState<string[]>([]);
  const [analyzing, setAnalyzing] = useState<{ done: number; total: number } | null>(null);
  const [jobs, setJobs] = useState<Job[]>([]);
  const [planError, setPlanError] = useState<string | null>(null);

  const [running, setRunning] = useState(false);
  const [rows, setRows] = useState<Record<number, RowState>>({});
  const [summary, setSummary] = useState<Summary | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState<Filter>("incompatible");
  const [dragging, setDragging] = useState(false);
  const [selected, setSelected] = useState<number | null>(null);

  const profile = profiles.find((p) => p.id === profileId);
  const analyzeSeq = useRef(0);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    save("theme", theme);
  }, [theme]);

  useEffect(() => {
    api.getProfiles().then(setProfiles).catch((e) => setError(String(e)));
    api.ffmpegInfo().then(setFfmpeg).catch(() => setFfmpeg(null));
  }, []);

  useEffect(() => save("profileId", profileId), [profileId]);
  useEffect(() => {
    const { output_dir: _ignored, ...rest } = settings;
    save("settings", rest);
  }, [settings]);

  // Si el modelo cambia, quitamos opciones que ese CDJ no acepta.
  useEffect(() => {
    if (!profile) return;
    setSettings((s) => {
      const formats = supportedFormats(profile);
      const next = { ...s };
      if (next.output_format !== "keep" && !formats.includes(next.output_format)) next.output_format = "keep";
      if (!formats.includes(next.fallback_format)) next.fallback_format = formats.includes("aiff") ? "aiff" : "wav";
      if (next.sample_rate && !sampleRates(profile, next.output_format).includes(next.sample_rate)) next.sample_rate = null;
      if (next.bit_depth && !bitDepths(profile, next.output_format).includes(next.bit_depth)) next.bit_depth = null;
      return next;
    });
  }, [profile]);

  const runAnalysis = useCallback(
    async (paths: string[], outputDir: string) => {
      if (!paths.length) return;
      const seq = ++analyzeSeq.current;
      setAnalyzing({ done: 0, total: 0 });
      setError(null);
      setSummary(null);
      setRows({});
      setSelected(null);
      try {
        const result = await api.analyze(paths, profileId, outputDir || null);
        if (seq !== analyzeSeq.current) return;
        setReports(result.reports);
        setRekordboxRoots(result.rekordboxRoots);
        if (result.reports.length === 0) setError("No se encontraron archivos de audio en lo que agregaste.");
      } catch (e) {
        if (seq === analyzeSeq.current) setError(String(e));
      } finally {
        if (seq === analyzeSeq.current) setAnalyzing(null);
      }
    },
    [profileId],
  );

  const addPaths = useCallback(
    async (paths: string[]) => {
      if (running || !paths.length) return;
      const merged = [...new Set([...inputs, ...paths])];
      setInputs(merged);
      let out = settings.output_dir;
      if (!outputTouched) {
        out = await api.defaultOutputDir(merged, profileId).catch(() => "");
        setSettings((s) => ({ ...s, output_dir: out }));
      }
      runAnalysis(merged, out);
    },
    [inputs, running, outputTouched, settings.output_dir, profileId, runAnalysis],
  );

  // Volver a analizar al cambiar de modelo.
  useEffect(() => {
    if (!inputs.length) return;
    (async () => {
      let out = settings.output_dir;
      if (!outputTouched) {
        out = await api.defaultOutputDir(inputs, profileId).catch(() => "");
        setSettings((s) => ({ ...s, output_dir: out }));
      }
      runAnalysis(inputs, out);
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [profileId]);

  // Arrastrar y soltar desde Finder / Explorador.
  const addPathsRef = useRef(addPaths);
  addPathsRef.current = addPaths;
  useEffect(() => {
    const unlisten = getCurrentWebview().onDragDropEvent((event) => {
      const type = event.payload.type;
      if (type === "enter" || type === "over") setDragging(true);
      else if (type === "leave") setDragging(false);
      else if (type === "drop") {
        setDragging(false);
        addPathsRef.current(event.payload.paths);
      }
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  useEffect(() => {
    const a = listen<{ done: number; total: number }>("analyze-progress", (e) => setAnalyzing(e.payload));
    const c = listen<JobEvent>("convert-progress", (e) => {
      const ev = e.payload;
      setRows((r) => ({
        ...r,
        [ev.index]: { status: ev.status, fraction: ev.fraction, message: ev.message, output: ev.output, running: ev.status === null },
      }));
    });
    return () => {
      a.then((f) => f());
      c.then((f) => f());
    };
  }, []);

  // Vista previa de lo que se hará con cada archivo.
  useEffect(() => {
    if (!reports.length || !profile) {
      setJobs([]);
      return;
    }
    const s = settings.output_dir ? settings : { ...settings, output_dir: "-" };
    api
      .previewPlan(reports, profileId, s)
      .then((j) => {
        setJobs(j);
        setPlanError(null);
      })
      .catch((e) => setPlanError(String(e)));
  }, [reports, settings, profileId, profile]);

  const pickFolder = async () => {
    const result = await open({ directory: true, multiple: true, title: "Elige carpetas o tu USB" });
    if (result) addPaths(Array.isArray(result) ? result : [result]);
  };
  const pickFiles = async () => {
    const result = await open({ multiple: true, title: "Elige canciones", filters: [{ name: "Audio", extensions: AUDIO_EXTENSIONS }] });
    if (result) addPaths(Array.isArray(result) ? result : [result]);
  };
  const pickOutput = async () => {
    const result = await open({ directory: true, multiple: false, title: "Carpeta donde guardar las copias", defaultPath: settings.output_dir || undefined });
    if (typeof result === "string") {
      setOutputTouched(true);
      setSettings((s) => ({ ...s, output_dir: result }));
    }
  };
  const clearAll = () => {
    analyzeSeq.current++;
    setInputs([]);
    setReports([]);
    setJobs([]);
    setRows({});
    setSummary(null);
    setRekordboxRoots([]);
    setError(null);
    setSelected(null);
    setAnalyzing(null);
    if (!outputTouched) setSettings((s) => ({ ...s, output_dir: "" }));
  };

  const startConvert = async () => {
    if (!profile) return;
    setRunning(true);
    setRows({});
    setSummary(null);
    setError(null);
    try {
      setSummary(await api.convert(reports, profileId, settings));
    } catch (e) {
      setError(String(e));
    } finally {
      setRunning(false);
    }
  };

  const counts = useMemo(() => {
    const incompatible = reports.filter((r) => !r.compatible).length;
    return { total: reports.length, incompatible, compatible: reports.length - incompatible };
  }, [reports]);

  const work = useMemo(() => {
    const convert = jobs.filter((j) => j.action === "convert");
    const copy = jobs.filter((j) => j.action === "copy").length;
    return { convert: convert.length, copy, seconds: convert.reduce((a, j) => a + j.duration_secs, 0) };
  }, [jobs]);

  const finished = useMemo(() => Object.values(rows).filter((r) => r.status !== null).length, [rows]);
  const totalWork = work.convert + work.copy;
  const overall = useMemo(() => {
    if (!totalWork) return 0;
    const sum = Object.values(rows).reduce((a, r) => a + (r.status ? 1 : r.fraction), 0);
    return Math.min(1, sum / totalWork);
  }, [rows, totalWork]);

  const visible = useMemo(
    () =>
      reports
        .map((r, i) => ({ r, i }))
        .filter(({ r }) => (filter === "all" ? true : filter === "compatible" ? r.compatible : !r.compatible)),
    [reports, filter],
  );

  const formats = supportedFormats(profile);
  const rates = sampleRates(profile, settings.output_format);
  const depths = bitDepths(profile, settings.output_format);
  const outputIsLossy = settings.output_format !== "keep" && LOSSY.includes(settings.output_format);
  const canConvert = !running && !analyzing && !!ffmpeg && reports.length > 0 && totalWork > 0 && !!settings.output_dir && !planError;
  const set = <K extends keyof ConvertSettings>(key: K, value: ConvertSettings[K]) => setSettings((s) => ({ ...s, [key]: value }));
  const detail = selected !== null ? reports[selected] : null;

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          <div className="logo" aria-hidden>
            <span />
          </div>
          <div>
            <h1>CDJ Converter</h1>
            <p>Revisa tu música y convierte solo lo que tu CDJ no puede reproducir</p>
          </div>
        </div>
        <button className="ghost icon" onClick={() => setTheme(theme === "dark" ? "light" : "dark")} title="Cambiar tema">
          {theme === "dark" ? "☀︎ Claro" : "☾ Oscuro"}
        </button>
      </header>

      <div className="layout">
        <aside className="sidebar">
          <section>
            <h2>1. Tu equipo</h2>
            <div className="models">
              {profiles.map((p) => (
                <button key={p.id} className={`model ${p.id === profileId ? "active" : ""}`} onClick={() => !running && setProfileId(p.id)} disabled={running}>
                  <strong>{p.name}</strong>
                  <small>{supportedFormats(p).map((f) => FORMAT_LABELS[f]).join(" · ")}</small>
                </button>
              ))}
            </div>
            {profile && !profile.verified && <p className="hint warn">Datos de {profile.name}: {profile.source}.</p>}
          </section>

          <section>
            <h2>2. Formato de salida</h2>
            <label>
              Formato
              <select value={settings.output_format} onChange={(e) => set("output_format", e.target.value as OutputFormat)} disabled={running}>
                <option value="keep">Mantener el original si es compatible</option>
                {formats.map((f) => (
                  <option key={f} value={f}>
                    {FORMAT_LABELS[f]}
                  </option>
                ))}
              </select>
            </label>
            {settings.output_format === "keep" && (
              <label>
                Si el formato no es compatible, convertir a
                <select value={settings.fallback_format} onChange={(e) => set("fallback_format", e.target.value as AudioFormat)} disabled={running}>
                  {formats.map((f) => (
                    <option key={f} value={f}>
                      {FORMAT_LABELS[f]}
                    </option>
                  ))}
                </select>
              </label>
            )}
            <div className="row2">
              <label>
                Sample rate
                <select value={settings.sample_rate ?? ""} onChange={(e) => set("sample_rate", e.target.value ? Number(e.target.value) : null)} disabled={running}>
                  <option value="">Automático</option>
                  {rates.map((r) => (
                    <option key={r} value={r}>
                      {khz(r)}
                    </option>
                  ))}
                </select>
              </label>
              {outputIsLossy ? (
                <label>
                  Bitrate
                  <select value={settings.lossy_bitrate_kbps} onChange={(e) => set("lossy_bitrate_kbps", Number(e.target.value))} disabled={running}>
                    {[320, 256, 192].map((b) => (
                      <option key={b} value={b}>
                        {b} kbps
                      </option>
                    ))}
                  </select>
                </label>
              ) : (
                <label>
                  Bit depth
                  <select value={settings.bit_depth ?? ""} onChange={(e) => set("bit_depth", e.target.value ? Number(e.target.value) : null)} disabled={running}>
                    <option value="">Automático</option>
                    {depths.map((b) => (
                      <option key={b} value={b}>
                        {b} bits
                      </option>
                    ))}
                  </select>
                </label>
              )}
            </div>
            <p className="hint">Automático conserva la calidad original cuando el CDJ la acepta; si no, usa la más alta compatible.</p>
            <label className="check">
              <input type="checkbox" checked={settings.convert_all} onChange={(e) => set("convert_all", e.target.checked)} disabled={running} />
              Convertir también los que ya son compatibles
            </label>
            <label className="check">
              <input type="checkbox" checked={settings.copy_compatible} onChange={(e) => set("copy_compatible", e.target.checked)} disabled={running} />
              Copiar los compatibles a la carpeta de salida (colección completa)
            </label>
          </section>

          <section>
            <h2>3. Dónde guardar las copias</h2>
            <button className="path" onClick={pickOutput} disabled={running} title={settings.output_dir}>
              {settings.output_dir || "Elegir carpeta…"}
            </button>
            <p className="hint">Los originales nunca se modifican. Si un archivo ya existe se guarda con otro nombre.</p>
          </section>

          <div className="actions">
            {running ? (
              <button className="danger big" onClick={() => api.cancel()}>
                Cancelar
              </button>
            ) : (
              <button className="primary big" onClick={startConvert} disabled={!canConvert}>
                {totalWork ? `Convertir ${work.convert} ${work.convert === 1 ? "archivo" : "archivos"}${work.copy ? ` y copiar ${work.copy}` : ""}` : "Convertir"}
              </button>
            )}
            {ffmpeg === null && <p className="hint warn">No se encontró FFmpeg. Reinstala la aplicación.</p>}
            {planError && <p className="hint warn">{planError}</p>}
          </div>
        </aside>

        <main className="content">
          {rekordboxRoots.length > 0 && (
            <div className="banner">
              <strong>USB de rekordbox detectada.</strong> Las copias convertidas se guardan aparte y la base de datos de la USB no se toca. Para usarlas en la
              USB, impórtalas en rekordbox, analízalas y vuelve a exportar.
            </div>
          )}
          {error && <div className="banner error">{error}</div>}

          {reports.length === 0 && !analyzing ? (
            <div className={`dropzone ${dragging ? "over" : ""}`}>
              <div className="dz-icon" aria-hidden>
                ⤓
              </div>
              <h3>Arrastra aquí canciones, carpetas o tu USB</h3>
              <p>MP3, WAV, AIFF, FLAC, ALAC, AAC, OGG y más. Se revisan contra el {profile?.name ?? "CDJ elegido"}.</p>
              <div className="dz-buttons">
                <button className="primary" onClick={pickFolder}>
                  Elegir carpeta o USB
                </button>
                <button className="ghost" onClick={pickFiles}>
                  Elegir canciones
                </button>
              </div>
            </div>
          ) : (
            <>
              <div className="toolbar">
                <div className="stats">
                  <span className="stat">{counts.total} archivos</span>
                  <span className="stat bad">{counts.incompatible} incompatibles</span>
                  <span className="stat good">{counts.compatible} compatibles</span>
                </div>
                <div className="tabs">
                  {(["incompatible", "compatible", "all"] as Filter[]).map((f) => (
                    <button key={f} className={filter === f ? "active" : ""} onClick={() => setFilter(f)}>
                      {f === "all" ? "Todos" : f === "compatible" ? "Compatibles" : "Incompatibles"}
                    </button>
                  ))}
                </div>
                <div className="tool-actions">
                  <button className="ghost" onClick={pickFolder} disabled={running}>
                    + Agregar
                  </button>
                  <button className="ghost" onClick={clearAll} disabled={running}>
                    Limpiar
                  </button>
                </div>
              </div>

              {(running || summary) && (
                <div className="progress-card">
                  <div className="progress-head">
                    <span>
                      {running
                        ? `Procesando ${finished} de ${totalWork}…`
                        : summary &&
                          `Listo: ${summary.converted} convertidos, ${summary.copied} copiados${summary.failed ? `, ${summary.failed} con error` : ""}${summary.cancelled ? `, ${summary.cancelled} cancelados` : ""}.`}
                    </span>
                    {!running && summary && (
                      <button className="ghost" onClick={() => revealItemInDir(settings.output_dir)}>
                        Abrir carpeta
                      </button>
                    )}
                  </div>
                  <div className="bar">
                    <div style={{ width: `${(running ? overall : 1) * 100}%` }} />
                  </div>
                </div>
              )}

              {analyzing && (
                <div className="progress-card">
                  <div className="progress-head">
                    <span>{analyzing.total ? `Analizando ${analyzing.done} de ${analyzing.total}…` : "Buscando archivos…"}</span>
                  </div>
                  <div className="bar indeterminate">
                    <div style={{ width: analyzing.total ? `${(analyzing.done / analyzing.total) * 100}%` : "30%" }} />
                  </div>
                </div>
              )}

              <div className={`table-wrap ${dragging ? "over" : ""}`}>
                <table>
                  <thead>
                    <tr>
                      <th>Archivo</th>
                      <th>Actual</th>
                      <th>Estado</th>
                      <th>Resultado</th>
                    </tr>
                  </thead>
                  <tbody>
                    {visible.map(({ r, i }) => {
                      const job = jobs[i];
                      const row = rows[i];
                      return (
                        <tr key={r.path} className={selected === i ? "selected" : ""} onClick={() => setSelected(selected === i ? null : i)}>
                          <td className="file">
                            <span className="name">{fileName(r.path)}</span>
                            <span className="sub">
                              {r.rel_path}
                              {r.info?.duration_secs ? ` · ${duration(r.info.duration_secs)}` : ""}
                            </span>
                          </td>
                          <td className="mono">{r.info ? describeInfo(r.info) : "—"}</td>
                          <td>
                            {r.compatible ? (
                              <span className="chip good">Compatible</span>
                            ) : (
                              <span className="chip bad" title={r.reasons.join("\n")}>
                                {r.reasons[0] ?? "Incompatible"}
                              </span>
                            )}
                            {r.warnings.length > 0 && (
                              <span className="chip warn" title={r.warnings.join("\n")}>
                                !
                              </span>
                            )}
                          </td>
                          <td>
                            <JobCell job={job} row={row} />
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
                {visible.length === 0 && !analyzing && (
                  <p className="empty">{filter === "incompatible" ? "Todo es compatible con este CDJ." : "Nada que mostrar."}</p>
                )}
              </div>

              {detail && (
                <div className="detail">
                  <div className="detail-head">
                    <strong>{fileName(detail.path)}</strong>
                    <button className="ghost" onClick={() => setSelected(null)}>
                      Cerrar
                    </button>
                  </div>
                  <p className="mono small">{detail.path}</p>
                  {detail.info && (
                    <p>
                      {detail.info.codec} · {describeInfo(detail.info)}
                      {detail.info.channels ? ` · ${detail.info.channels} canales` : ""}
                    </p>
                  )}
                  {detail.error && <p className="warn">{detail.error}</p>}
                  {detail.reasons.map((x) => (
                    <p key={x} className="bad-text">
                      ✕ {x}
                    </p>
                  ))}
                  {detail.warnings.map((x) => (
                    <p key={x} className="warn">
                      ! {x}
                    </p>
                  ))}
                  {jobs[selected!]?.notes.map((x) => (
                    <p key={x} className="hint">
                      {x}
                    </p>
                  ))}
                  {jobs[selected!]?.output && <p className="mono small">→ {jobs[selected!].output}</p>}
                  {rows[selected!]?.message && <p className="warn">{rows[selected!].message}</p>}
                </div>
              )}
            </>
          )}

          <footer className="foot">
            {ffmpeg ? `${ffmpeg.version.replace(/ Copyright.*/, "")}${ffmpeg.hasSoxr ? " · remuestreo SoX" : ""}` : ffmpeg === undefined ? "" : "FFmpeg no disponible"}
          </footer>
        </main>
      </div>
    </div>
  );
}

function JobCell({ job, row }: { job: Job | undefined; row: RowState | undefined }) {
  if (row?.running) {
    return (
      <div className="mini-bar">
        <div style={{ width: `${row.fraction * 100}%` }} />
      </div>
    );
  }
  if (row?.status) {
    const label = { converted: "✓ Convertido", copied: "✓ Copiado", skipped: "Sin cambios", failed: "✕ Error", cancelled: "Cancelado" }[row.status];
    return (
      <span className={`result ${row.status}`} title={row.message ?? row.output ?? ""}>
        {label}
        {row.message && row.status !== "failed" ? " (aviso)" : ""}
      </span>
    );
  }
  if (!job) return null;
  if (job.action === "skip") return <span className="muted">{job.notes[0] ?? "Se deja igual"}</span>;
  return (
    <span className={job.action === "convert" ? "target" : "muted"} title={job.notes.join("\n")}>
      {job.action === "convert" ? `→ ${job.target_label}` : "Copia sin cambios"}
      {job.notes.length > 0 && " *"}
    </span>
  );
}
