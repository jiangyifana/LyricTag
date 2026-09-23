// 把静态资源拷进 dist/。
//
// 本项目**不使用打包器**：tsc 只负责 .ts → .js，HTML/CSS 原样复制。
// 这样前端产物就是纯静态文件，直接内嵌进二进制，没有额外的构建链路要维护。
import { cpSync, mkdirSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));
const src = join(root, "..", "src");
const dist = join(root, "..", "dist");

mkdirSync(dist, { recursive: true });

for (const file of ["index.html", "styles.css"]) {
  const from = join(src, file);
  if (!existsSync(from)) {
    console.error(`缺少静态资源：${from}`);
    process.exit(1);
  }
  cpSync(from, join(dist, file));
  console.log(`copied ${file}`);
}
