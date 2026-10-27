// Tipos que reflejan las estructuras del núcleo en Rust (crates/cdj-core).

export type AudioFormat = "mp3" | "aac" | "wav" | "aiff" | "flac" | "alac" | "other";
export type OutputFormat = "keep" | Exclude<AudioFormat, "other">;

export interface FormatVariant {
  label: string;
  sample_rates: number[];
  bit_depths: number[];
  bitrate_kbps: [number, number] | null;
}

export interface CdjProfile {
  id: string;
  name: string;
  source: string;
  verified: boolean;
  max_channels: number;
  max_folder_depth: number | null;
  formats: Partial<Record<AudioFormat, { extensions: string[]; variants: FormatVariant[] }>>;
}

export interface AudioInfo {
  format: AudioFormat;
  codec: string;
  extension: string;
  sample_rate: number | null;
  bit_depth: number | null;
  bitrate_kbps: number | null;
  channels: number | null;
  duration_secs: number;
  codec_issue: string | null;
}

export interface FileReport {
  path: string;
  root: string;
  rel_path: string;
  info: AudioInfo | null;
  error: string | null;
  compatible: boolean;
  reasons: string[];
  warnings: string[];
}

export interface ConvertSettings {
  output_format: OutputFormat;
  fallback_format: AudioFormat;
  bit_depth: number | null;
  sample_rate: number | null;
  lossy_bitrate_kbps: number;
  convert_all: boolean;
  copy_compatible: boolean;
  output_dir: string;
}

export type Action = "convert" | "copy" | "skip";

export interface Job {
  source: string;
  output: string | null;
  action: Action;
  target_label: string | null;
  notes: string[];
  duration_secs: number;
}

export type JobStatus = "converted" | "copied" | "skipped" | "failed" | "cancelled";

export interface JobEvent {
  index: number;
  status: JobStatus | null;
  fraction: number;
  output: string | null;
  message: string | null;
}

export interface Summary {
  converted: number;
  copied: number;
  skipped: number;
  failed: number;
  cancelled: number;
}

export interface AnalyzeResult {
  reports: FileReport[];
  rekordboxRoots: string[];
}

export interface FfmpegInfo {
  path: string;
  version: string;
  hasSoxr: boolean;
}
