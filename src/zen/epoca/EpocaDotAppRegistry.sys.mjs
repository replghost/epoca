// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// Parent-process registry of dot product bundles. A product is an
// in-memory map of absolute paths to asset bytes; the dot protocol
// handler (directly in the parent, via the EpocaDotApp process actor from
// content) resolves every dot://<name>.dot/<path> load against it.
// Registry keys are bare labels ("browse"); the URI host carries the
// canonical .dot suffix ("browse.dot").
//
// Products never fetch from the network: the CSP injected into every HTML
// asset locks all load directives to the dotapp scheme, and
// EpocaDotAppContentPolicy backstops that from outside the document.

const DEV_ROOTS_PREF = "epoca.dotapp.dev-roots";
const DOTNS_PREF = "epoca.dotapp.dotns.enabled";
const IMAGE_GATEWAYS_PREF = "epoca.dotapp.image-gateways";

/**
 * Origins products may load images from, in addition to their own bundle:
 * the content-addressed IPFS gateways (app icons reference bare CIDs there).
 * This is the single deliberate exception to the no-network rule, shared by
 * the CSP below and EpocaDotAppContentPolicy.
 *
 * @returns {string[]}
 */
export function imageGatewayOrigins() {
  try {
    const parsed = JSON.parse(
      Services.prefs.getStringPref(IMAGE_GATEWAYS_PREF, "[]")
    );
    return Array.isArray(parsed)
      ? parsed.filter(o => /^https:\/\/[a-z0-9.-]+$/i.test(o))
      : [];
  } catch {
    return [];
  }
}

const lazy = {};

ChromeUtils.defineESModuleGetters(lazy, {
  EpocaDotNs: "resource:///modules/EpocaDotNs.sys.mjs",
});

// Every fetch directive is pinned to the product's own scheme (same-origin
// enforcement on top of this comes from CORS: dotapp channels carry no
// Access-Control-Allow-Origin, so cross-product fetch fails). The bridge
// MessagePort is unaffected by CSP, so TrUAPI traffic still flows.
function productCsp() {
  const imgExtra = imageGatewayOrigins().join(" ");
  return (
    "default-src 'none'; " +
    "script-src dot: 'unsafe-inline' 'unsafe-eval' 'wasm-unsafe-eval'; " +
    "style-src dot: 'unsafe-inline'; " +
    `img-src dot: data: blob: ${imgExtra}; `.replace(/ +/g, " ") +
    "media-src dot: data: blob:; " +
    "font-src dot: data:; " +
    "connect-src dot:; " +
    "frame-src dot:; " +
    "worker-src dot: blob:; " +
    "object-src 'none'; " +
    "base-uri 'none'; " +
    "form-action 'none'"
  );
}

const CONTENT_TYPES = new Map([
  ["html", "text/html"],
  ["htm", "text/html"],
  ["js", "text/javascript"],
  ["mjs", "text/javascript"],
  ["css", "text/css"],
  ["json", "application/json"],
  ["map", "application/json"],
  ["wasm", "application/wasm"],
  ["svg", "image/svg+xml"],
  ["png", "image/png"],
  ["jpg", "image/jpeg"],
  ["jpeg", "image/jpeg"],
  ["gif", "image/gif"],
  ["webp", "image/webp"],
  ["ico", "image/x-icon"],
  ["woff", "font/woff"],
  ["woff2", "font/woff2"],
  ["ttf", "font/ttf"],
  ["otf", "font/otf"],
  ["txt", "text/plain"],
]);

function contentTypeFor(path) {
  const ext = /\.([^./]+)$/.exec(path)?.[1]?.toLowerCase();
  return CONTENT_TYPES.get(ext) || "application/octet-stream";
}

export const EpocaDotAppRegistry = {
  // productId -> Map(absolute path -> { bytes: Uint8Array, contentType })
  _products: new Map(),

  /**
   * Register (or replace) a product bundle.
   *
   * @param {string} productId - The dotapp host, e.g. "browse".
   * @param {object} assets - Plain object mapping paths ("/index.html") to
   *   Uint8Array bytes or { bytes, contentType }.
   */
  register(productId, assets) {
    const map = new Map();
    for (const [path, value] of Object.entries(assets)) {
      // ArrayBuffer.isView, unlike instanceof, also recognizes typed arrays
      // created in another global (e.g. a test scope).
      const asset = ArrayBuffer.isView(value) ? { bytes: value } : value;
      if (!ArrayBuffer.isView(asset?.bytes)) {
        throw new Error(`dotapp asset ${path} must provide Uint8Array bytes`);
      }
      map.set(path.startsWith("/") ? path : `/${path}`, {
        bytes: asset.bytes,
        contentType: asset.contentType || contentTypeFor(path),
      });
    }
    this._products.set(productId, map);
  },

  unregister(productId) {
    this._products.delete(productId);
  },

  /**
   * Resolve a dot URI to channel payload. Only the canonical host form
   * <label>.dot is served — anything else 404s, so a product cannot obtain
   * a second origin for the same bundle.
   *
   * @param {nsIURI|string} uri
   * @param {number} [browsingContextId] - The initiating browsing context;
   *   used to surface resolution progress in that window's status panel.
   * @returns {Promise<{inputStream, contentType}|null>} null when unknown.
   */
  async resolve(uri, browsingContextId) {
    if (typeof uri === "string") {
      uri = Services.io.newURI(uri);
    }
    if (uri.scheme !== "dot" || !uri.host.endsWith(".dot")) {
      return null;
    }
    const productId = uri.host.slice(0, -".dot".length);
    // filePath excludes query/ref and is dot-segment-normalized by the
    // standard URL parser.
    let path = uri.filePath;
    if (path.endsWith("/")) {
      path += "index.html";
    }

    let asset = await this._lookup(productId, path, browsingContextId);
    if (!asset && !/\.[^/]+$/.test(path)) {
      // Extensionless miss: treat as a client-side SPA route.
      asset = await this._lookup(productId, "/index.html", browsingContextId);
    }
    if (!asset) {
      return null;
    }

    let { bytes, contentType } = asset;
    if (contentType === "text/html") {
      bytes = injectCsp(bytes);
    }
    const stream = Cc[
      "@mozilla.org/io/arraybuffer-input-stream;1"
    ].createInstance(Ci.nsIArrayBufferInputStream);
    stream.setData(
      bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength),
      0,
      bytes.byteLength
    );
    return { inputStream: stream, contentType };
  },

  async _lookup(productId, path, browsingContextId) {
    const registered = this._products.get(productId)?.get(path);
    if (registered) {
      return registered;
    }
    const dev = await this._readDevRoot(productId, path);
    if (dev) {
      return dev;
    }
    return this._resolveViaDotNs(productId, path, browsingContextId);
  },

  // Mirror ordinary web loads: while the (suspended) channel waits on dotNS
  // and the bundle fetch, show progress in the initiating window's status
  // panel — the same component that shows "Looking up host…" for the web.
  _setLoadStatus(browsingContextId, message) {
    if (!browsingContextId) {
      return;
    }
    try {
      const win = BrowsingContext.get(browsingContextId)?.topChromeWindow;
      win?.XULBrowserWindow?.onStatusChange(null, null, 0, message);
    } catch {
      // Status text is best-effort chrome sugar; never fail a load over it.
    }
  },

  // Unknown product: resolve it through dotNS, register the fetched bundle,
  // and retry the lookup. This is what makes a first navigation to
  // dot://browse.dot/ work with nothing pre-registered.
  async _resolveViaDotNs(productId, path, browsingContextId) {
    if (
      !Services.prefs.getBoolPref(DOTNS_PREF, false) ||
      this._products.has(productId)
    ) {
      return null;
    }
    this._setLoadStatus(browsingContextId, `Looking up ${productId}.dot…`);
    try {
      const assets = await lazy.EpocaDotNs.resolve(productId);
      this.register(productId, assets);
    } catch (e) {
      console.warn(`dotapp: dotNS resolution failed for ${productId}`, e);
      return null;
    } finally {
      this._setLoadStatus(browsingContextId, "");
    }
    return this._products.get(productId)?.get(path) ?? null;
  },

  // Dev convenience: epoca.dotapp.dev-roots is a JSON object mapping product
  // ids to local directories holding an unpacked bundle, so a product like
  // browse.dot can be loaded before dotNS/CAR distribution exists.
  async _readDevRoot(productId, path) {
    let roots;
    try {
      roots = JSON.parse(Services.prefs.getStringPref(DEV_ROOTS_PREF, ""));
    } catch {
      return null;
    }
    const root = roots?.[productId];
    if (!root || path.includes("..")) {
      return null;
    }
    const file = PathUtils.join(root, ...path.split("/").filter(Boolean));
    try {
      const bytes = await IOUtils.read(file);
      return { bytes, contentType: contentTypeFor(path) };
    } catch {
      return null;
    }
  },
};

// The host, not the product, decides the CSP: inject it as the first thing
// in <head> so it applies before any product markup. Bundles are expected to
// be well-formed documents; without a <head> or <html> tag the meta is
// prepended after any doctype (the parser hoists leading metas into the
// implicit head).
function injectCsp(bytes) {
  const html = new TextDecoder().decode(bytes);
  // <base href="/"> is essential for SPA client routes: products reference
  // bundled assets relatively (e.g. Nuxt/Vite "./_nuxt/x.js"), and when we
  // serve index.html as the fallback for a route like /play/, the document
  // base would otherwise be dot://<id>/play/ so relative assets resolve to
  // /play/_nuxt/x.js (404). Pinning the base to the product root fixes them;
  // absolute-path products are unaffected. It must precede any asset ref.
  const meta =
    `<base href="/">` +
    `<meta http-equiv="Content-Security-Policy" content="${productCsp()}">`;
  let insertAt = null;
  const anchor = /<head[^>]*>|<html[^>]*>/i.exec(html);
  if (anchor) {
    insertAt = anchor.index + anchor[0].length;
  } else {
    const doctype = /^\s*<!doctype[^>]*>/i.exec(html);
    insertAt = doctype ? doctype.index + doctype[0].length : 0;
  }
  return new TextEncoder().encode(
    html.slice(0, insertAt) + meta + html.slice(insertAt)
  );
}
