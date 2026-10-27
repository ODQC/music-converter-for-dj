import { invoke } from "@tauri-apps/api/core";
import type { AnalyzeResult, CdjProfile, ConvertSettings, FfmpegInfo, FileReport, Job, Summary } from "./types";

export const api = {
  getProfiles: () => invoke<CdjProfile[]>("get_profiles"),
  ffmpegInfo: () => invoke<FfmpegInfo | null>("ffmpeg_info"),
  defaultOutputDir: (paths: string[], profileId: string) => invoke<string>("default_output_dir", { paths, profileId }),
  analyze: (paths: string[], profileId: string, exclude: string | null) => invoke<AnalyzeResult>("analyze", { paths, profileId, exclude }),
  previewPlan: (reports: FileReport[], profileId: string, settings: ConvertSettings) =>
    invoke<Job[]>("preview_plan", { reports, profileId, settings }),
  convert: (reports: FileReport[], profileId: string, settings: ConvertSettings) =>
    invoke<Summary>("convert", { reports, profileId, settings }),
  cancel: () => invoke<void>("cancel_convert"),
};
