// Version checks CI runs before building:
// - package.json and src-tauri/Cargo.toml carry the same app version
// - on a tag build, the tag matches that version
// - each @tauri-apps npm package is on the same major.minor as its Rust crate,
//   which `tauri build` requires. Dependabot bumps cargo and npm separately.
// Run from the repo root after `npm ci`.
const fs = require("fs");

const problems = [];
const minor = (v) => v.split(".").slice(0, 2).join(".");

const appVersion = require("../package.json").version;
const cargoToml = fs.readFileSync("src-tauri/Cargo.toml", "utf8");
const cargoVersion = cargoToml.match(/^version = "([^"]+)"/m)?.[1];
if (appVersion !== cargoVersion) {
  problems.push(`package.json is ${appVersion} but Cargo.toml is ${cargoVersion}`);
}

if (process.env.GITHUB_REF_TYPE === "tag") {
  const tag = process.env.GITHUB_REF_NAME;
  if (tag !== `v${cargoVersion}`) {
    problems.push(`tag ${tag} doesn't match the app version ${cargoVersion}`);
  }
}

const lock = fs.readFileSync("src-tauri/Cargo.lock", "utf8");
const crateVersion = (name) =>
  lock.match(new RegExp(`name = "${name}"\\nversion = "([^"]+)"`))?.[1];
const pkg = require("../package.json");
for (const name of Object.keys({ ...pkg.dependencies, ...pkg.devDependencies })) {
  let crate = null;
  if (name === "@tauri-apps/api") crate = "tauri";
  else if (name.startsWith("@tauri-apps/plugin-")) {
    crate = `tauri-plugin-${name.slice("@tauri-apps/plugin-".length)}`;
  }
  if (!crate) continue;
  const npm = require(`../node_modules/${name}/package.json`).version;
  const rust = crateVersion(crate);
  if (rust && minor(npm) !== minor(rust)) {
    problems.push(`${name} ${npm} and the ${crate} crate ${rust} are on different minor versions`);
  }
}

if (problems.length) {
  for (const p of problems) console.error(p);
  process.exit(1);
}
console.log(`versions ok: app ${cargoVersion}`);
