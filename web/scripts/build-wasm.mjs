import { spawnSync } from "node:child_process";

const mode = process.argv[2];
if (!['dev', 'release'].includes(mode)) {
  console.error("usage: node scripts/build-wasm.mjs <dev|release>");
  process.exit(2);
}

const arguments_ = [
  "build",
  "../crates/rvah-web",
  "--target",
  "web",
  mode === "release" ? "--release" : "--dev",
  "--out-dir",
  "../../web/pkg",
];

// A developer's global Cargo config may contain target-specific nightly flags.
// wasm-pack invokes host cargo metadata before the wasm build, so isolate this
// reproducible frontend build unless the developer explicitly opts back in.
const environment = {
  ...process.env,
  RUSTFLAGS: process.env.RVAH_RUSTFLAGS || "",
};
const result = spawnSync("wasm-pack", arguments_, {
  cwd: new URL("..", import.meta.url),
  env: environment,
  stdio: "inherit",
  shell: process.platform === "win32",
});
if (result.error) throw result.error;
process.exit(result.status ?? 1);
