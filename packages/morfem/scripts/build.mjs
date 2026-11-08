// Builds the engine to wasm, generates JS glue, compiles TS. Needs: rust + wasm32 target, wasm-bindgen CLI 0.2.130.
import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, rmSync, readdirSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const pkg = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const root = resolve(pkg, "../..");
const run = (cmd, args, opts = {}) => execFileSync(cmd, args, { stdio: "inherit", cwd: root, ...opts });

function findBindgen() {
  try {
    execFileSync("wasm-bindgen", ["--version"], { stdio: "pipe" });
    return "wasm-bindgen";
  } catch {}
  // worker-build caches a matching binary.
  const cache = join(homedir(), "Library/Caches/worker-build");
  const alt = [cache, join(homedir(), ".cache/worker-build")];
  for (const dir of alt) {
    if (!existsSync(dir)) continue;
    for (const d of readdirSync(dir)) {
      const p = join(dir, d, "wasm-bindgen");
      if (d.startsWith("wasm-bindgen-") && d.endsWith("0.2.130") && existsSync(p)) return p;
    }
  }
  throw new Error("wasm-bindgen CLI not found. Run: cargo install wasm-bindgen-cli --version 0.2.130");
}

run("cargo", ["build", "--release", "--target", "wasm32-unknown-unknown", "-p", "morfem-wasm"]);
const gen = join(pkg, "src/gen");
rmSync(gen, { recursive: true, force: true });
mkdirSync(gen, { recursive: true });
run(findBindgen(), [
  "--target", "web", "--out-dir", gen, "--out-name", "morfem_wasm",
  join(root, "target/wasm32-unknown-unknown/release/morfem_wasm.wasm"),
]);
rmSync(join(pkg, "dist"), { recursive: true, force: true });
run(join(pkg, "node_modules/.bin/tsc"), ["-p", pkg], { cwd: pkg });
cpSync(gen, join(pkg, "dist/gen"), { recursive: true });
