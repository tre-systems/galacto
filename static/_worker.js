// The known page routes (extensionless, clean URLs). Anything else that looks
// like a page request gets a real 404 instead of falling back to the app shell.
const PAGES = new Set(["/", "/physics", "/engineering", "/audio"]);

export default {
  async fetch(request, env) {
    const url = new URL(request.url);

    // Canonical host: redirect the legacy and www hostnames to the apex before
    // falling through to the static asset handler.
    if (url.hostname === "galacto.tre.systems" || url.hostname === "www.galacto.org") {
      url.protocol = "https:";
      url.hostname = "galacto.org";
      return Response.redirect(url.toString(), 301);
    }

    // Real 404 for unknown page routes. Asset requests (anything with a file
    // extension, e.g. /og-card.png, /robots.txt) and the known pages are served
    // normally; only an extensionless unknown path gets the 404 page, so crawlers
    // see a true 404 rather than a 200 duplicate of the home page.
    const accept = request.headers.get("accept") || "";
    if (
      (request.method === "GET" || request.method === "HEAD") &&
      accept.includes("text/html")
    ) {
      const path = url.pathname.replace(/\/+$/, "") || "/";
      const isAsset = path.split("/").pop().includes(".");
      if (!isAsset && !PAGES.has(path)) {
        const res = await env.ASSETS.fetch(new URL("/404.html", url));
        return new Response(res.body, { status: 404, headers: res.headers });
      }
    }

    return env.ASSETS.fetch(request);
  },
};
