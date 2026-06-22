(() => {
  const config = window.TRE_STATIC_SENTRY_CONFIG;
  if (!config?.dsn) return;

  const script = document.createElement("script");
  // Self-hosted SDK bundle (tracing + the user-feedback widget; replay is included
  // but stays off via the 0 sample rates below). Served from our own origin rather
  // than Sentry's CDN so the feedback button and error monitoring don't depend on
  // that CDN being reachable — `?v=` is the vendored bundle's version, bumped only
  // when sentry-sdk.js is replaced.
  script.src = "/sentry-sdk.js?v=10.57.0";
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
    const integrations = [];
    if (typeof window.Sentry.browserTracingIntegration === "function") {
      integrations.push(
        window.Sentry.browserTracingIntegration({
          enableInp: true,
          instrumentNavigation: true,
          instrumentPageLoad: true,
        }),
      );
    }
    // User-feedback widget, attached to the control panel's own button rather than
    // auto-injecting a floating one (autoInject: false).
    if (typeof window.Sentry.feedbackIntegration === "function") {
      integrations.push(
        window.Sentry.feedbackIntegration({
          autoInject: false,
          colorScheme: "dark",
          showBranding: false,
          submitButtonLabel: "Send feedback",
          formTitle: "Send feedback",
          messagePlaceholder: "What's working, what's broken, or what you'd love to see?",
        }),
      );
    }

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

    // Reveal the control panel's feedback button and make it open the form. The
    // button stays hidden until this runs, so it never appears as a dead control
    // when Sentry (or the feedback widget) is unavailable.
    const feedback =
      typeof window.Sentry.getFeedback === "function" ? window.Sentry.getFeedback() : null;
    if (feedback) {
      const attach = () => {
        const btn = document.getElementById("feedback-btn");
        if (!btn) return;
        try {
          feedback.attachTo(btn);
          btn.hidden = false;
        } catch {
          /* keep the button hidden if attach fails */
        }
      };
      if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", attach, { once: true });
      } else {
        attach();
      }
    }
  };
  document.head.appendChild(script);
})();
