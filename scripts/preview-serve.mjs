#!/usr/bin/env node
/**
 * 开发用静态服务器：把 `.preview/` 用 HTTP 提供出来。
 *
 * 为什么需要它：ES 模块在 `file://` 下会被 CORS 拦住
 * （`Access to script ... has been blocked by CORS policy`）。
 * 真实应用里不存在这个问题——Tauri 通过自己的协议提供资源。
 */
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize } from "node:path";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", ".preview");
const port = Number(process.env.PORT || 5199);

const types = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".png": "image/png",
  ".svg": "image/svg+xml",
};

const server = createServer(async (req, res) => {
  const url = decodeURIComponent((req.url || "/").split("?")[0]);
  const rel = normalize(url === "/" ? "/index.html" : url).replace(/^([/\\])+/, "");
  const file = join(root, rel);
  if (!file.startsWith(root)) {
    res.writeHead(403).end("forbidden");
    return;
  }
  try {
    const body = await readFile(file);
    res.writeHead(200, { "content-type": types[extname(file)] ?? "application/octet-stream" });
    res.end(body);
  } catch {
    res.writeHead(404).end("not found");
  }
});

server.listen(port, "127.0.0.1", () => {
  console.log(`预览服务已启动：http://127.0.0.1:${port}/`);
});
