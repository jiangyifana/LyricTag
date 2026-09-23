#!/usr/bin/env node
/**
 * 开发用预览：把真实的 dist/ 界面套上一层「假的 Tauri 后端」跑起来，
 * 用来在没有编译整个桌面应用的情况下检查界面渲染与交互。
 *
 * **不参与发布**：产物落在 `.preview/`，只供本地看一眼。
 *
 * 用法：
 *   node scripts/preview.mjs
 *   # 然后用浏览器打开 .preview/index.html，或用无头 Chrome 截图
 */
import { cpSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const dist = join(root, "dist");
const out = join(root, ".preview");

rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
cpSync(dist, out, { recursive: true });

// ── 假后端 ────────────────────────────────────────────────────────────────
const stub = readFileSync(join(root, "scripts", "preview-stub.js"), "utf8");
writeFileSync(join(out, "preview-stub.js"), stub);

const html = readFileSync(join(dist, "index.html"), "utf8").replace(
  '<script type="module" src="./main.js"></script>',
  '<script src="./preview-stub.js"></script>\n<script type="module" src="./main.js"></script>',
);
writeFileSync(join(out, "index.html"), html);

console.log(`预览已生成：${join(out, "index.html")}`);
