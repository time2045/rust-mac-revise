import http from "node:http";
import path from "node:path";
import { open, realpath } from "node:fs/promises";
import { pipeline } from "node:stream/promises";
import { fileURLToPath } from "node:url";

const HOST = "127.0.0.1";
const PORT = 1420;
const UI_ROOT = fileURLToPath(new URL("../ui/", import.meta.url));
const MIME = new Map([
  [".html", "text/html; charset=utf-8"],
  [".css", "text/css; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".mjs", "text/javascript; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".svg", "image/svg+xml"],
  [".png", "image/png"],
  [".jpg", "image/jpeg"],
  [".jpeg", "image/jpeg"],
  [".webp", "image/webp"],
  [".ico", "image/x-icon"],
  [".woff2", "font/woff2"],
]);
const CSP = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self'; font-src 'self'; connect-src 'self' ipc: http://ipc.localhost https://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'; frame-ancestors 'none'";

function httpError(status, message) {
  return Object.assign(new Error(message), { status });
}
function isWithin(root, target) {
  const relative = path.relative(root, target);
  return relative === "" || (!path.isAbsolute(relative) && relative !== ".." && !relative.startsWith(`..${path.sep}`));
}

// Inspect the raw path before URL normalization, which would erase traversal segments.
export function resolveRequestPath(rawTarget, root = UI_ROOT) {
  if (typeof rawTarget !== "string" || !rawTarget.startsWith("/") || rawTarget.startsWith("//")) throw httpError(400, "Invalid request target");
  let pathname;
  try { pathname = decodeURIComponent(rawTarget.split(/[?#]/, 1)[0]); }
  catch { throw httpError(400, "Invalid URL encoding"); }
  if (/[\u0000-\u001f\u007f\\:<>"|?*]/.test(pathname)) throw httpError(400, "Invalid path");
  const segments = pathname.split("/").filter(Boolean);
  if (segments.some((part) => part === "." || part === ".." || part.startsWith(".") || /[. ]$/.test(part) || /^(?:CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(?:\.|$)/i.test(part))) throw httpError(403, "Forbidden path");
  const target = path.resolve(root, ...(segments.length ? segments : ["index.html"]));
  if (!isWithin(root, target)) throw httpError(403, "Forbidden path");
  return target;
}

export async function startServer() {
  const root = await realpath(UI_ROOT);
  const server = http.createServer(async (request, response) => {
    response.setHeader("X-Content-Type-Options", "nosniff");
    response.setHeader("Cache-Control", "no-store");
    response.setHeader("Content-Security-Policy", CSP);
    response.setHeader("Referrer-Policy", "no-referrer");
    let file;
    try {
      if (![`${HOST}:${PORT}`, `localhost:${PORT}`].includes(request.headers.host)) throw httpError(403, "Use http://127.0.0.1:1420");
      if (request.method !== "GET" && request.method !== "HEAD") {
        response.setHeader("Allow", "GET, HEAD");
        throw httpError(405, "Method not allowed");
      }
      const candidate = resolveRequestPath(request.url, root);
      if (path.basename(candidate) === "favicon.ico" && path.dirname(candidate) === root) {
        response.writeHead(204);
        response.end();
        return;
      }
      // Canonicalize the existing file too: symlinks/junctions must not escape ui/.
      const resolved = await realpath(candidate);
      if (!isWithin(root, resolved)) throw httpError(403, "Forbidden path");
      file = await open(resolved, "r");
      const info = await file.stat();
      if (!info.isFile()) throw httpError(404, "Not found");
      response.setHeader("Content-Type", MIME.get(path.extname(resolved).toLowerCase()) || "application/octet-stream");
      response.setHeader("Content-Length", info.size);
      response.writeHead(200);
      if (request.method === "HEAD") response.end();
      else await pipeline(file.createReadStream({ autoClose: false }), response);
    } catch (error) {
      if (!response.headersSent && !response.destroyed) {
        const status = error.status || (["ENOENT", "ENOTDIR", "EISDIR"].includes(error.code) ? 404 : error.code === "EACCES" || error.code === "EPERM" ? 403 : 500);
        const message = status === 500 ? "Internal server error" : error.status ? error.message : status === 403 ? "Forbidden" : "Not found";
        response.removeHeader("Content-Length");
        response.writeHead(status, { "Content-Type": "text/plain; charset=utf-8" });
        response.end(request.method === "HEAD" ? undefined : `${message}\n`);
      } else if (!response.destroyed) response.destroy();
      if (!error.status && !["ENOENT", "ENOTDIR", "EISDIR", "EACCES", "EPERM", "ECONNRESET", "ERR_STREAM_PREMATURE_CLOSE"].includes(error.code)) console.error("Dev server request error:", error.message);
    } finally {
      if (file) await file.close().catch((error) => console.error("File close error:", error.message));
    }
  });
  server.requestTimeout = 15000;
  server.headersTimeout = 10000;
  server.keepAliveTimeout = 5000;
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(PORT, HOST, resolve);
  });
  server.on("error", (error) => console.error("Dev server error:", error.message));
  console.log(`MAC Manager UI: http://${HOST}:${PORT} (browser demo, no system changes)`);
  return server;
}

// Importing path helpers for tests does not bind a port or start the server.
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  startServer().catch((error) => {
    console.error(error.code === "EADDRINUSE" ? `Port ${PORT} is already in use. Stop the existing process and retry.` : `Cannot start dev server: ${error.message}`);
    process.exitCode = 1;
  });
}
