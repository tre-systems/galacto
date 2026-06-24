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

const args = process.argv.slice(2);

function usage() {
  console.error(`Usage:
  npm run video:captions -- --input in.mp4 --output out.mp4 [options]

Options:
  --start-title "Galacto"
  --start-subtitle "Self-gravitating N-body galaxy simulation"
  --end-title "Galacto"
  --end-subtitle "galacto.org\\nSimulation and sound: Robert Gilks"
  --start-at 1.7
  --start-duration 4
  --end-duration 7
  --fade 0.8
  --bitrate 55M
`);
}

function take(name, fallback = null) {
  const index = args.indexOf(name);
  if (index === -1) return fallback;
  const value = args[index + 1];
  if (!value || value.startsWith("--")) {
    throw new Error(`${name} requires a value`);
  }
  return value;
}

function takeNumber(name, fallback) {
  const raw = take(name);
  if (raw == null) return fallback;
  const value = Number(raw);
  if (!Number.isFinite(value)) throw new Error(`${name} must be a number`);
  return value;
}

function hasFlag(name) {
  return args.includes(name);
}

function required(name) {
  const value = take(name);
  if (!value) {
    usage();
    throw new Error(`${name} is required`);
  }
  return value;
}

function run(command, commandArgs) {
  const result = spawnSync(command, commandArgs, {
    stdio: "pipe",
    encoding: "utf8",
  });
  if (result.status !== 0) {
    process.stderr.write(result.stdout);
    process.stderr.write(result.stderr);
    throw new Error(`${command} failed with status ${result.status}`);
  }
  return result.stdout;
}

function ensureTool(command, argsForVersion) {
  const result = spawnSync(command, argsForVersion, {
    stdio: "ignore",
  });
  if (result.status !== 0) {
    throw new Error(`${command} is required. Install it with: brew install librsvg`);
  }
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
      stroke-width="${Math.max(2, Math.round(fontSize * 0.045))}"
      stroke-opacity="0.55"
      paint-order="stroke fill"
      letter-spacing="0"
    >${spans}</text>
  `;
}

function writeCaptionSvg(path, { width, height, title, subtitle, titleY, titleSize, subtitleSize }) {
  const subtitleLines = subtitle.split("\n").map((line) => line.trim()).filter(Boolean);
  const titleLines = title.split("\n").map((line) => line.trim()).filter(Boolean);
  const subtitleY = titleY + titleSize * 1.25;
  const svg = `<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}">
  <rect width="100%" height="100%" fill="none"/>
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

if (hasFlag("--help")) {
  usage();
  process.exit(0);
}

const input = required("--input");
const output = required("--output");
const startTitle = take("--start-title", "Galacto");
const startSubtitle = take("--start-subtitle", "Self-gravitating N-body galaxy simulation").replaceAll("\\n", "\n");
const endTitle = take("--end-title", "Galacto");
const endSubtitle = take("--end-subtitle", "galacto.org\nSimulation and sound: Robert Gilks").replaceAll("\\n", "\n");
const startAt = takeNumber("--start-at", 1.7);
const startDuration = takeNumber("--start-duration", 4.0);
const endDuration = takeNumber("--end-duration", 7.0);
const fade = takeNumber("--fade", 0.8);
const bitrate = take("--bitrate", "55M");

if (!existsSync(input)) throw new Error(`Input does not exist: ${input}`);
ensureTool("rsvg-convert", ["--version"]);
const meta = probe(input);
if (!Number.isFinite(meta.duration)) throw new Error(`Could not read video duration for ${input}`);
mkdirSync(dirname(output), { recursive: true });

const tmp = mkdtempSync(join(tmpdir(), "galacto-captions-"));

try {
  const titleSize = Math.max(48, Math.round(meta.height / 27));
  const subtitleSize = Math.max(26, Math.round(meta.height / 54));
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
      titleY: meta.height * 0.66,
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
      titleY: meta.height * 0.38,
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

  run("ffmpeg", [
    ...ffmpegArgs,
    "-filter_complex",
    filter,
    "-map",
    "[v]",
    "-map",
    "0:a?",
    "-c:v",
    "hevc_videotoolbox",
    "-b:v",
    bitrate,
    "-maxrate",
    "70M",
    "-bufsize",
    "110M",
    "-tag:v",
    "hvc1",
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
  ]);
} finally {
  rmSync(tmp, { recursive: true, force: true });
}
