export const UPDATE_CHECK_INTERVAL_MS = 60 * 60 * 1000;
export const UPDATE_CHECK_COOLDOWN_MS = 60 * 1000;
export const UPDATE_RELOAD_FALLBACK_MS = 4000;

export function shouldCheckForUpdate(now, lastCheckAt, cooldownMs = UPDATE_CHECK_COOLDOWN_MS) {
  return lastCheckAt === 0 || now - lastCheckAt >= cooldownMs;
}

export async function checkForServiceWorkerUpdate(registration, swUrl, fetcher = fetch) {
  const response = await fetcher(swUrl, {
    cache: "no-store",
    headers: { "cache-control": "no-cache" },
  });
  if (!response.ok) throw new Error("Service worker is unavailable");
  await registration.update();
}

export function installUpdateCheckTriggers(check, options = {}) {
  const windowTarget = options.windowTarget || window;
  const documentTarget = options.documentTarget || document;
  const isVisible = options.isVisible || (() => document.visibilityState === "visible");
  const isOnline = options.isOnline || (() => navigator.onLine);
  const setIntervalFn = options.setIntervalFn || window.setInterval.bind(window);
  const clearIntervalFn = options.clearIntervalFn || window.clearInterval.bind(window);
  const intervalMs = options.intervalMs || UPDATE_CHECK_INTERVAL_MS;
  const checkWhenAvailable = () => {
    if (isVisible() && isOnline()) check();
  };
  const intervalId = setIntervalFn(checkWhenAvailable, intervalMs);

  documentTarget.addEventListener("visibilitychange", checkWhenAvailable);
  windowTarget.addEventListener("focus", checkWhenAvailable);
  windowTarget.addEventListener("online", checkWhenAvailable);
  windowTarget.addEventListener("pageshow", checkWhenAvailable);

  return () => {
    clearIntervalFn(intervalId);
    documentTarget.removeEventListener("visibilitychange", checkWhenAvailable);
    windowTarget.removeEventListener("focus", checkWhenAvailable);
    windowTarget.removeEventListener("online", checkWhenAvailable);
    windowTarget.removeEventListener("pageshow", checkWhenAvailable);
  };
}

export function activateWaitingServiceWorker(worker, options = {}) {
  const serviceWorkerContainer = options.serviceWorkerContainer || navigator.serviceWorker;
  const reload = options.reload || (() => window.location.reload());
  const setTimeoutFn = options.setTimeoutFn || window.setTimeout.bind(window);
  const clearTimeoutFn = options.clearTimeoutFn || window.clearTimeout.bind(window);
  let finished = false;

  const finish = () => {
    if (finished) return;
    finished = true;
    clearTimeoutFn(fallback);
    serviceWorkerContainer.removeEventListener("controllerchange", finish);
    worker.removeEventListener("statechange", onStateChange);
    reload();
  };
  const onStateChange = () => {
    if (worker.state === "activated") finish();
  };
  const fallback = setTimeoutFn(finish, UPDATE_RELOAD_FALLBACK_MS);

  serviceWorkerContainer.addEventListener("controllerchange", finish);
  worker.addEventListener("statechange", onStateChange);
  worker.postMessage({ type: "SKIP_WAITING" });
}
