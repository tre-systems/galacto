import { existsSync } from "node:fs";
import { createServer } from "node:net";
import { spawnSync } from "node:child_process";

export function findChrome() {
  const candidates = [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/usr/bin/google-chrome",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
    "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
  ].filter(Boolean);
  return candidates.find((path) => existsSync(path));
}

export function ensureExecutable(command, { args = ["--version"], label = command, installHint = "" } = {}) {
  if (!command) {
    throw new Error(`${label} is required${formatHint(installHint)}`);
  }
  const result = spawnSync(command, args, { stdio: "ignore" });
  if (result.error) {
    const detail =
      result.error.code === "ENOENT"
        ? "not found"
        : `could not start: ${result.error.message}`;
    throw new Error(`${label} is required (${detail})${formatHint(installHint)}`);
  }
  if (result.status !== 0) {
    throw new Error(
      `${label} is required, but "${command} ${args.join(" ")}" exited with status ${result.status}${formatHint(
        installHint,
      )}`,
    );
  }
}

export function ensureChrome(chromePath) {
  ensureExecutable(chromePath, {
    args: ["--version"],
    label: "Chrome/Chromium",
    installHint: "install Chrome/Chromium, set CHROME=/path/to/browser, or pass --chrome /path/to/browser",
  });
}

export async function getFreePort(host = "127.0.0.1") {
  const server = createServer();
  server.unref();
  await new Promise((resolveListen, rejectListen) => {
    server.once("error", rejectListen);
    server.listen(0, host, () => {
      server.off("error", rejectListen);
      resolveListen();
    });
  });
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : null;
  await new Promise((resolveClose, rejectClose) => {
    server.close((error) => (error ? rejectClose(error) : resolveClose()));
  });
  if (!port) throw new Error("could not allocate a free TCP port");
  return port;
}

function formatHint(installHint) {
  return installHint ? `. ${installHint}` : "";
}
