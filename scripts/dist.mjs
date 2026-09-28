// 打包：跑 tauri build，把便携版 exe 和 NSIS 安装包收进 release/。
// 用法：npm run dist
import { execSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, statSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const fail = (m) => { console.error(`[dist] ✗ ${m}`); process.exit(1); };
const mb = (p) => (statSync(p).size / 1024 / 1024).toFixed(2);

// 三个地方版本号必须一致
const read = (p) => readFileSync(join(root, p), "utf8");
const cargo = read("src-tauri/Cargo.toml");
const versions = {
  "package.json": JSON.parse(read("package.json")).version,
  "tauri.conf.json": JSON.parse(read("src-tauri/tauri.conf.json")).version,
  "Cargo.toml": cargo.slice(0, cargo.indexOf("[lib]")).match(/^\s*version\s*=\s*"([^"]+)"/m)?.[1],
};
if (new Set(Object.values(versions)).size !== 1) {
  fail(`版本号不一致：${Object.entries(versions).map(([k, v]) => `${k}=${v}`).join("  ")}`);
}
const ver = versions["package.json"];
console.log(`[dist] ✓ 版本一致：v${ver}`);

console.log("[dist] tauri build …");
try {
  execSync("npm run tauri build", { cwd: root, stdio: "inherit" });
} catch {
  fail("构建失败。若报 os error 32 / 拒绝访问，多半是本程序还在运行锁住了 exe，退出后重试");
}

const rawExe = join(root, "src-tauri/target/release/zcode-pool.exe");
if (!existsSync(rawExe)) fail(`找不到裸 exe：${rawExe}`);

const outDir = join(root, "release");
mkdirSync(outDir, { recursive: true });
const portable = join(outDir, `zcode-pool_${ver}_portable.exe`);
copyFileSync(rawExe, portable);
console.log(`[dist] ✓ 便携版  ${basename(portable)}  (${mb(portable)} MB)`);

// NSIS 安装包（没配 nsis 目标时可能不存在，不算失败）
const nsisDir = join(root, "src-tauri/target/release/bundle/nsis");
if (existsSync(nsisDir)) {
  const setup = readdirSync(nsisDir)
    .filter((f) => f.endsWith("-setup.exe") && f.includes(`_${ver}_`))
    .sort((a, b) => statSync(join(nsisDir, b)).mtimeMs - statSync(join(nsisDir, a)).mtimeMs)[0];
  if (setup) {
    copyFileSync(join(nsisDir, setup), join(outDir, setup));
    console.log(`[dist] ✓ 安装包  ${setup}`);
  }
}
console.log(`[dist] 完成 → ${outDir}`);
