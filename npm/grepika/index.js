#!/usr/bin/env node

const { execFileSync } = require("child_process");
const path = require("path");

const PLATFORMS = {
  "darwin-arm64": "@agentika/grepika-darwin-arm64",
  "darwin-x64": "@agentika/grepika-darwin-x64",
  "linux-x64": "@agentika/grepika-linux-x64",
  "linux-arm64": "@agentika/grepika-linux-arm64",
  "win32-x64": "@agentika/grepika-win32-x64",
};

const platformKey = `${process.platform}-${process.arch}`;
const pkg = PLATFORMS[platformKey];

if (!pkg) {
  console.error(
    `Unsupported platform: ${platformKey}. Supported: ${Object.keys(PLATFORMS).join(", ")}`
  );
  process.exit(1);
}

let binPath;
try {
  binPath = path.join(
    require.resolve(`${pkg}/package.json`),
    "..",
    "bin",
    `grepika${process.platform === "win32" ? ".exe" : ""}`
  );
} catch {
  console.error(
    `Could not find package ${pkg}. Make sure it was installed (npm should install it automatically via optionalDependencies).`
  );
  process.exit(1);
}

const args = process.argv.slice(2);
try {
  execFileSync(binPath, args, { stdio: "inherit" });
} catch (e) {
  process.exit(e.status ?? 1);
}
