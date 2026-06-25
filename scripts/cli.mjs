// Shared CLI helpers for the production scripts (produce / capture / captions).
import { spawnSync } from "node:child_process";

export const args = process.argv.slice(2);

/** Value following `--name`, or `fallback`. Throws if the flag is given without one. */
export function take(name, fallback = null) {
  const index = args.indexOf(name);
  if (index === -1) return fallback;
  const value = args[index + 1];
  if (value == null || value.startsWith("--")) {
    throw new Error(`${name} requires a value`);
  }
  return value;
}

/** Like `take`, parsed as a finite number. */
export function takeNumber(name, fallback) {
  const raw = take(name);
  if (raw == null) return fallback;
  const value = Number(raw);
  if (!Number.isFinite(value)) throw new Error(`${name} must be a number`);
  return value;
}

/** Whether a bare `--name` flag is present. */
export function hasFlag(name) {
  return args.includes(name);
}

/** If `--name <value>` is present, return `["--name", value]` for spreading; else `[]`. */
export function passArg(name) {
  const value = take(name);
  return value != null ? [name, value] : [];
}

/**
 * Run a command, throwing on a non-zero exit. By default output is captured and
 * returned (and only printed on failure); pass `{ inherit: true }` to stream it live.
 */
export function run(command, commandArgs, { inherit = false } = {}) {
  const result = spawnSync(
    command,
    commandArgs,
    inherit ? { stdio: "inherit" } : { stdio: "pipe", encoding: "utf8" },
  );
  if (result.status !== 0) {
    if (!inherit) {
      process.stderr.write(result.stdout ?? "");
      process.stderr.write(result.stderr ?? "");
    }
    throw new Error(`${command} ${commandArgs.join(" ")} failed (status ${result.status})`);
  }
  return inherit ? "" : result.stdout;
}
