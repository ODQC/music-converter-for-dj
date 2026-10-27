import type { AudioFormat, AudioInfo } from "./types";

export const FORMAT_LABELS: Record<AudioFormat, string> = {
  mp3: "MP3",
  aac: "AAC",
  wav: "WAV",
  aiff: "AIFF",
  flac: "FLAC",
  alac: "ALAC",
  other: "Otro",
};

export const LOSSY: AudioFormat[] = ["mp3", "aac"];

export function khz(rate: number): string {
  const k = rate / 1000;
  return `${Number.isInteger(k) ? k : k.toFixed(2).replace(/0+$/, "")} kHz`;
}

export function describeInfo(info: AudioInfo): string {
  const parts: string[] = [info.format === "other" ? info.codec : FORMAT_LABELS[info.format]];
  if (info.sample_rate) parts.push(khz(info.sample_rate));
  if (info.bit_depth && !LOSSY.includes(info.format)) parts.push(`${info.bit_depth} bits`);
  if (info.bitrate_kbps && LOSSY.includes(info.format)) parts.push(`${info.bitrate_kbps} kbps`);
  return parts.join(" · ");
}

export function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

export function duration(secs: number): string {
  if (!secs) return "";
  const m = Math.floor(secs / 60);
  const s = Math.round(secs % 60);
  return `${m}:${String(s).padStart(2, "0")}`;
}
