export default {
  async fetch(request, env) {
    const url = new URL(request.url);

    // Own the legacy public hostname here so Cloudflare redirects it before
    // falling through to the static asset handler.
    if (url.hostname === "galacto.tre.systems") {
      url.protocol = "https:";
      url.hostname = "galacto.org";
      return Response.redirect(url.toString(), 301);
    }

    return env.ASSETS.fetch(request);
  },
};
