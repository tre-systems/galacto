import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { spawnSync } from "node:child_process";
import { take, takeNumber, hasFlag, run } from "./cli.mjs";
import { ensureExecutable } from "./preflight.mjs";

function usage() {
  console.error(`Usage:
  npm run video:captions -- --input in.mp4 --output out.mp4 [options]

Options:
  --start-title "Galacto"
  --start-subtitle "Self-gravitating N-body galaxy simulation"
  --end-title "Galacto"
  --end-subtitle "galacto.org\\nSimulation and sound: Multivibrator"
  --start-at 1.7
  --start-duration 4
  --end-duration 7
  --fade 0.8
  --bitrate 55M
  --video-codec auto|h264|hevc|libx264|hevc_videotoolbox
`);
}

function required(name) {
  const value = take(name);
  if (!value) {
    usage();
    throw new Error(`${name} is required`);
  }
  return value;
}

function probe(path) {
  const json = run("ffprobe", [
    "-hide_banner",
    "-v",
    "error",
    "-show_entries",
    "format=duration:stream=codec_type,width,height",
    "-of",
    "json",
    path,
  ]);
  const data = JSON.parse(json);
  const video = data.streams.find((stream) => stream.codec_type === "video");
  if (!video) throw new Error(`No video stream found in ${path}`);
  return {
    duration: Number(data.format.duration),
    width: Number(video.width),
    height: Number(video.height),
  };
}

function escapeXml(value) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function textBlock(lines, { x, y, fontSize, lineHeight, weight = 500 }) {
  if (lines.length === 0) return "";
  const [first, ...rest] = lines;
  const spans = [
    `<tspan x="${x}" dy="0">${escapeXml(first)}</tspan>`,
    ...rest.map((line) => `<tspan x="${x}" dy="${lineHeight}">${escapeXml(line)}</tspan>`),
  ].join("");
  return `
    <text
      x="${x}"
      y="${y}"
      text-anchor="middle"
      font-family="Avenir, Avenir Next, Helvetica, Arial, sans-serif"
      font-size="${fontSize}"
      font-weight="${weight}"
      fill="#f7f9ff"
      stroke="#02030a"
      stroke-width="${Math.max(2, Math.round(fontSize * 0.06))}"
      stroke-opacity="0.8"
      paint-order="stroke fill"
      letter-spacing="0"
    >${spans}</text>
  `;
}

function writeCaptionSvg(path, { width, height, title, subtitle, titleY, titleSize, subtitleSize }) {
  const subtitleLines = subtitle.split("\n").map((line) => line.trim()).filter(Boolean);
  const titleLines = title.split("\n").map((line) => line.trim()).filter(Boolean);
  const subtitleY = titleY + titleSize * 1.25;

  // A soft dark scrim behind the text — a radial darkening that fades to nothing at
  // the edges — so the white caption stays legible even over the galaxy's bright core
  // or bloom, without a hard box. It fades in/out with the caption overlay.
  const scrimTop = titleY - titleSize * (1.0 + (titleLines.length - 1) * 1.15);
  const scrimBottom =
    subtitleY + (subtitleLines.length - 1) * subtitleSize * 1.35 + subtitleSize * 0.7;
  const scrimCy = (scrimTop + scrimBottom) / 2;
  const scrimRy = (scrimBottom - scrimTop) / 2 + titleSize * 0.7;
  const scrimRx = width * 0.44;

  const svg = `<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}">
  <defs>
    <radialGradient id="scrim" cx="50%" cy="50%" r="50%">
      <stop offset="0%" stop-color="#01020a" stop-opacity="0.8"/>
      <stop offset="55%" stop-color="#01020a" stop-opacity="0.6"/>
      <stop offset="100%" stop-color="#01020a" stop-opacity="0"/>
    </radialGradient>
  </defs>
  <rect width="100%" height="100%" fill="none"/>
  <ellipse cx="${width / 2}" cy="${scrimCy}" rx="${scrimRx}" ry="${scrimRy}" fill="url(#scrim)"/>
  <g opacity="0.98">
    ${textBlock(titleLines, {
      x: width / 2,
      y: titleY,
      fontSize: titleSize,
      lineHeight: titleSize * 1.15,
      weight: 700,
    })}
    ${textBlock(subtitleLines, {
      x: width / 2,
      y: subtitleY,
      fontSize: subtitleSize,
      lineHeight: subtitleSize * 1.35,
      weight: 500,
    })}
  </g>
</svg>
`;
  writeFileSync(path, svg);
}

function renderPng(svgPath, pngPath, width, height) {
  run("rsvg-convert", [
    "--format",
    "png",
    "--width",
    String(width),
    "--height",
    String(height),
    "--output",
    pngPath,
    svgPath,
  ]);
}

function overlayFilter({ inputIndex, streamName, duration, start, end, fade }) {
  const effectiveFade = Math.max(0.01, Math.min(fade, duration / 2));
  const fadeOutStart = Math.max(0, duration - effectiveFade);
  return `[${inputIndex}:v]format=rgba,fade=t=in:st=0:d=${effectiveFade}:alpha=1,fade=t=out:st=${fadeOutStart}:d=${effectiveFade}:alpha=1,setpts=PTS+${start}/TB[${streamName}];`;
}

function codecCandidates(requested) {
  const normalized = requested.trim().toLowerCase();
  if (!normalized || normalized === "auto") {
    return process.platform === "darwin" ? ["hevc_videotoolbox", "libx264"] : ["libx264"];
  }
  if (normalized === "h264" || normalized === "x264") {
    return process.platform === "darwin" ? ["libx264", "h264_videotoolbox"] : ["libx264"];
  }
  if (normalized === "hevc" || normalized === "h265") {
    return process.platform === "darwin" ? ["hevc_videotoolbox", "libx265"] : ["libx265"];
  }
  if (normalized === "x265") return ["libx265"];
  return [requested];
}

function videoCodecArgs(encoder, bitrate) {
  const args = ["-c:v", encoder];
  if (encoder === "libx264" || encoder === "libx265") {
    args.push("-preset", "slow");
  }
  args.push("-b:v", bitrate, "-maxrate", "70M", "-bufsize", "110M");
  if (isHevcEncoder(encoder)) {
    args.push("-tag:v", "hvc1");
  }
  return args;
}

function isHevcEncoder(encoder) {
  return /hevc|h265|x265/i.test(encoder);
}

function encodeWithFallback({ ffmpegArgs, filter, bitrate, requestedCodec, output }) {
  const candidates = codecCandidates(requestedCodec);
  const failures = [];
  for (const encoder of candidates) {
    const args = [
      ...ffmpegArgs,
      "-filter_complex",
      filter,
      "-map",
      "[v]",
      "-map",
      "0:a?",
      ...videoCodecArgs(encoder, bitrate),
      "-pix_fmt",
      "yuv420p",
      "-color_primaries",
      "bt709",
      "-color_trc",
      "bt709",
      "-colorspace",
      "bt709",
      "-c:a",
      "copy",
      "-movflags",
      "+faststart",
      output,
    ];
    console.log(`Video codec: ${encoder}`);
    const result = spawnSync("ffmpeg", args, {
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
      stdio: ["ignore", "pipe", "pipe"],
    });
    if (!result.error && result.status === 0) return encoder;

    rmSync(output, { force: true });
    failures.push(`${encoder}: ${summarizeFailure(result)}`);
    if (candidates.length > 1) {
      console.warn(`ffmpeg failed with ${encoder}; trying next codec...`);
    }
  }
  throw new Error(`Caption encode failed with codec candidates ${candidates.join(", ")}:\n${failures.join("\n")}`);
}

function summarizeFailure(result) {
  if (result.error) return result.error.message;
  const output = `${result.stdout || ""}\n${result.stderr || ""}`.trim();
  const tail = output.split("\n").filter(Boolean).slice(-12).join("\n");
  return tail || `ffmpeg exited with status ${result.status}`;
}

if (hasFlag("--help")) {
  usage();
  process.exit(0);
}

const input = required("--input");
const output = required("--output");
const startTitle = take("--start-title", "Galacto");
const startSubtitle = take("--start-subtitle", "Self-gravitating N-body galaxy simulation").replaceAll("\\n", "\n");
const endTitle = take("--end-title", "Galacto");
const endSubtitle = take("--end-subtitle", "galacto.org\nSimulation and sound: Multivibrator").replaceAll("\\n", "\n");
const startAt = takeNumber("--start-at", 1.7);
const startDuration = takeNumber("--start-duration", 4.0);
const endDuration = takeNumber("--end-duration", 7.0);
const fade = takeNumber("--fade", 0.8);
const bitrate = take("--bitrate", "55M");
const videoCodec = take("--video-codec", "auto");

if (!existsSync(input)) throw new Error(`Input does not exist: ${input}`);
ensureExecutable("ffmpeg", {
  args: ["-version"],
  label: "ffmpeg",
  installHint: "install ffmpeg (macOS: brew install ffmpeg)",
});
ensureExecutable("ffprobe", {
  args: ["-version"],
  label: "ffprobe",
  installHint: "install ffmpeg (macOS: brew install ffmpeg)",
});
ensureExecutable("rsvg-convert", {
  args: ["--version"],
  label: "rsvg-convert",
  installHint: "install librsvg (macOS: brew install librsvg)",
});
const meta = probe(input);
if (!Number.isFinite(meta.duration)) throw new Error(`Could not read video duration for ${input}`);
mkdirSync(dirname(output), { recursive: true });

const tmp = mkdtempSync(join(tmpdir(), "galacto-captions-"));

try {
  const titleSize = Math.max(56, Math.round(meta.height / 21));
  const subtitleSize = Math.max(30, Math.round(meta.height / 42));
  const startEnd = Math.min(meta.duration - 0.15, startAt + startDuration);
  const endStart = Math.max(startEnd + 1, meta.duration - endDuration);
  const endEnd = meta.duration - 0.15;
  const overlays = [];

  if (startEnd - startAt >= 0.1 && (startTitle.trim() || startSubtitle.trim())) {
    const svgPath = join(tmp, "start-caption.svg");
    const pngPath = join(tmp, "start-caption.png");
    writeCaptionSvg(svgPath, {
      width: meta.width,
      height: meta.height,
      title: startTitle,
      subtitle: startSubtitle,
      // Lower third, dropped a little further down, clear of the bright centre.
      titleY: meta.height * 0.77,
      titleSize,
      subtitleSize,
    });
    renderPng(svgPath, pngPath, meta.width, meta.height);
    overlays.push({ pngPath, start: startAt, end: startEnd, duration: startEnd - startAt });
  }

  if (endEnd - endStart >= 0.1 && (endTitle.trim() || endSubtitle.trim())) {
    const svgPath = join(tmp, "end-caption.svg");
    const pngPath = join(tmp, "end-caption.png");
    writeCaptionSvg(svgPath, {
      width: meta.width,
      height: meta.height,
      title: endTitle,
      subtitle: endSubtitle,
      // Upper third, nudged down toward the middle but still clear of the centre.
      titleY: meta.height * 0.33,
      titleSize,
      subtitleSize,
    });
    renderPng(svgPath, pngPath, meta.width, meta.height);
    overlays.push({ pngPath, start: endStart, end: endEnd, duration: endEnd - endStart });
  }

  if (overlays.length === 0) throw new Error("No captions fit inside the input duration");

  const ffmpegArgs = ["-hide_banner", "-y", "-i", input];
  for (const overlay of overlays) {
    ffmpegArgs.push("-loop", "1", "-t", String(overlay.duration), "-i", overlay.pngPath);
  }

  let filter = "";
  let base = "[0:v]";
  overlays.forEach((overlay, index) => {
    const streamName = `caption${index}`;
    filter += overlayFilter({
      inputIndex: index + 1,
      streamName,
      duration: overlay.duration,
      start: overlay.start,
      end: overlay.end,
      fade,
    });
    const outName = index === overlays.length - 1 ? "[v]" : `[v${index}]`;
    filter += `${base}[${streamName}]overlay=0:0:enable='between(t,${overlay.start},${overlay.end})'${outName}`;
    if (index !== overlays.length - 1) filter += ";";
    base = outName;
  });

  console.log(`Input: ${basename(input)} (${meta.width}x${meta.height}, ${meta.duration.toFixed(3)}s)`);
  console.log(`Output: ${output}`);
  console.log(`Opening caption: ${startAt.toFixed(2)}s-${startEnd.toFixed(2)}s`);
  console.log(`End caption: ${endStart.toFixed(2)}s-${endEnd.toFixed(2)}s`);

  encodeWithFallback({
    ffmpegArgs,
    filter,
    bitrate,
    requestedCodec: videoCodec,
    output,
  });
} finally {
  rmSync(tmp, { recursive: true, force: true });
}
