(() => {
  const config = window.TRE_STATIC_SENTRY_CONFIG;
  if (!config?.dsn) return;

  const script = document.createElement("script");
  script.src = "https://browser.sentry-cdn.com/10.57.0/bundle.tracing.min.js";
  script.crossOrigin = "anonymous";
  script.onload = () => {
    if (!window.Sentry) return;
    const configuredTracesSampleRate = Number(config.tracesSampleRate);
    const tracesSampleRate =
      Number.isFinite(configuredTracesSampleRate) &&
      configuredTracesSampleRate >= 0 &&
      configuredTracesSampleRate <= 1
        ? configuredTracesSampleRate
        : config.environment === "production"
          ? 0.05
          : 0;
    const integrations =
      typeof window.Sentry.browserTracingIntegration === "function"
        ? [
            window.Sentry.browserTracingIntegration({
              enableInp: true,
              instrumentNavigation: true,
              instrumentPageLoad: true,
            }),
          ]
        : [];

    window.Sentry.init({
      dsn: config.dsn,
      environment: config.environment || "production",
      release: config.release,
      sendDefaultPii: false,
      tracesSampleRate,
      integrations,
      replaysSessionSampleRate: 0,
      replaysOnErrorSampleRate: 0,
      beforeSend(event) {
        if (event.request) {
          delete event.request.cookies;
          delete event.request.data;
          if (event.request.headers) {
            for (const key of Object.keys(event.request.headers)) {
              const lowerKey = key.toLowerCase();
              if (lowerKey.includes("authorization") || lowerKey.includes("cookie")) {
                event.request.headers[key] = "[Filtered]";
              }
            }
          }
        }
        event.tags = { ...event.tags, app: config.app || "galacto" };
        return event;
      },
    });
  };
  document.head.appendChild(script);
})();
