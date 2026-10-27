#!/usr/bin/env node
// Descarga un FFmpeg estático y lo deja donde Tauri espera el "sidecar":
//   src-tauri/binaries/ffmpeg-<target-triple>[.exe]
//
// Uso: node scripts/fetch-ffmpeg.mjs [--target <triple>]
//   Sin --target se usa la plataforma actual (rustc -vV).
//   Targets: x86_64-pc-windows-msvc, aarch64-apple-darwin, x86_64-apple-darwin,
//            universal-apple-darwin, x86_64-unknown-linux-gnu (usa el ffmpeg del sistema).
//   Se puede forzar la URL con FFMPEG_URL_WINDOWS, FFMPEG_URL_MAC_ARM64 o FFMPEG_URL_MAC_X64.

import { execFileSync } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const outDir = join(root, "src-tauri", "binaries");

const SOURCES = {
  windows: [process.env.FFMPEG_URL_WINDOWS, "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-lgpl.zip"],
  macArm64: [process.env.FFMPEG_URL_MAC_ARM64, "https://ffmpeg.martin-riedl.de/redirect/latest/macos/arm64/release/ffmpeg.zip"],
  macX64: [
    process.env.FFMPEG_URL_MAC_X64,
    "https://ffmpeg.martin-riedl.de/redirect/latest/macos/amd64/release/ffmpeg.zip",
    "https://evermeet.cx/ffmpeg/getrelease/zip",
  ],
};

function hostTriple() {
  const out = execFileSync("rustc", ["-vV"], { encoding: "utf8" });
  return out.match(/host: (\S+)/)[1];
}

async function download(urls, dest) {
  for (const url of urls.filter(Boolean)) {
    try {
      console.log(`Descargando ${url}`);
      const res = await fetch(url, { redirect: "follow" });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      writeFileSync(dest, Buffer.from(await res.arrayBuffer()));
      return;
    } catch (err) {
      console.warn(`  falló: ${err.message}`);
    }
  }
  throw new Error("No se pudo descargar FFmpeg de ninguna fuente");
}

function extract(zip, dir) {
  if (process.platform === "win32") {
    execFileSync("powershell", ["-NoProfile", "-Command", `Expand-Archive -Force -LiteralPath '${zip}' -DestinationPath '${dir}'`], { stdio: "inherit" });
  } else {
    execFileSync("unzip", ["-q", "-o", zip, "-d", dir], { stdio: "inherit" });
  }
}

function findFile(dir, name) {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) {
      const found = findFile(full, name);
      if (found) return found;
    } else if (entry === name) {
      return full;
    }
  }
  return null;
}

async function fetchBinary(urls, name, work) {
  const zip = join(work, `${Math.random().toString(36).slice(2)}.zip`);
  const dir = `${zip}.d`;
  await download(urls, zip);
  extract(zip, dir);
  const bin = findFile(dir, name);
  if (!bin) throw new Error(`No se encontró ${name} dentro del zip`);
  return bin;
}

function checkRuns(bin) {
  const out = execFileSync(bin, ["-hide_banner", "-version"], { encoding: "utf8" });
  console.log(out.split("\n")[0]);
  for (const lib of ["libmp3lame", "libsoxr"]) {
    console.log(`  ${lib}: ${out.includes(`--enable-${lib}`) ? "sí" : "no"}`);
  }
}

async function main() {
  const idx = process.argv.indexOf("--target");
  const target = idx > 0 ? process.argv[idx + 1] : hostTriple();
  mkdirSync(outDir, { recursive: true });
  const work = mkdtempSync(join(tmpdir(), "ffmpeg-"));
  const isWindows = target.includes("windows");
  const dest = join(outDir, `ffmpeg-${target}${isWindows ? ".exe" : ""}`);

  try {
    if (isWindows) {
      copyFileSync(await fetchBinary(SOURCES.windows, "ffmpeg.exe", work), dest);
    } else if (target === "universal-apple-darwin") {
      const arm = await fetchBinary(SOURCES.macArm64, "ffmpeg", work);
      const x64 = await fetchBinary(SOURCES.macX64, "ffmpeg", work);
      execFileSync("lipo", ["-create", arm, x64, "-output", dest], { stdio: "inherit" });
      // lipo invalida las firmas: firma ad-hoc para que Apple Silicon lo ejecute.
      execFileSync("codesign", ["--force", "--sign", "-", dest], { stdio: "inherit" });
    } else if (target === "aarch64-apple-darwin") {
      copyFileSync(await fetchBinary(SOURCES.macArm64, "ffmpeg", work), dest);
    } else if (target === "x86_64-apple-darwin") {
      copyFileSync(await fetchBinary(SOURCES.macX64, "ffmpeg", work), dest);
    } else if (target.includes("linux")) {
      const system = execFileSync("sh", ["-c", "command -v ffmpeg"], { encoding: "utf8" }).trim();
      if (!system) throw new Error("Instala ffmpeg en el sistema (apt install ffmpeg)");
      copyFileSync(system, dest);
    } else {
      throw new Error(`Target no soportado: ${target}`);
    }
    if (!isWindows) chmodSync(dest, 0o755);
    console.log(`FFmpeg listo en ${dest}`);
    const canRun =
      (isWindows && process.platform === "win32") ||
      (target.includes("apple") && process.platform === "darwin") ||
      (target.includes("linux") && process.platform === "linux");
    if (canRun && existsSync(dest)) checkRuns(dest);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

main().catch((err) => {
  console.error(err.message);
  process.exit(1);
});
