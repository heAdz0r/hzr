// Exercise the public configuration flow using only executables shipped in the bundle.
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { pathToFileURL } from "node:url";

const bundle = resolve(process.argv[2]);
const root = realpathSync(mkdtempSync(join(tmpdir(), "hzr-bundled-delegation-")));
try {
  const home = join(root, "home");
  mkdirSync(home);
  const config = join(root, "config.toml");
  writeFileSync(config, `data_dir = ${JSON.stringify(join(root, "data"))}\n`, { mode: 0o600 });
  const env = { ...process.env, HOME: home, XDG_CONFIG_HOME: home,
    PATH: join(root, "no-external-tools"), HZR_NO_UPDATE_CHECK: "1" };
  delete env.HZR_NODE;
  delete env.HZR_CAVEMAN_CODE_DIR;
  const cli = (...args) => spawnSync(join(bundle, "bin/hzr"),
    ["--config", config, ...args], { env, encoding: "utf8", timeout: 15000 });
  let result = cli("settings", "--json");
  assert.equal(result.status, 0, result.stderr);
  assert.equal(JSON.parse(result.stdout).delegation.enabled, false);
  const source = join(root, "input.secret");
  const fixtureKey = "bundle-fixture-not-a-provider-key";
  writeFileSync(source, fixtureKey, { mode: 0o600 });
  result = cli("settings", "login", "--provider", "opencode-go", "--key-file", source, "--json");
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout.includes(fixtureKey), false);
  result = cli("settings", "delegation", "--provider", "opencode-go",
    "--model", "deepseek-v4.1-flash", "--enabled", "true", "--json");
  assert.equal(result.status, 0, result.stderr);
  assert.equal(JSON.parse(result.stdout).credential_configured, true);
  assert.equal(readFileSync(config, "utf8").includes(fixtureKey), false);
  // Import the shipped bridge and its production dependencies with no PATH runtime.
  const bridge = await import(pathToFileURL(join(bundle, "engines/caveman-code/bridge.mjs")));
  const options = await bridge.createWorkerOptions({
    provider: "opencode-go", model: "deepseek-v4.1-flash",
    credential_file: join(root, "data/credentials/opencode-go.secret"),
  });
  assert.equal(options.model.provider, "opencode-go");
  assert.equal(options.model.id, "deepseek-v4.1-flash");
  result = cli("settings", "delegation", "--enabled", "false");
  assert.equal(result.status, 0, result.stderr);
  result = cli("delegate", "Never call a provider");
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /delegation is disabled/);
  console.log("Bundled delegation: private login, persistence, selection and disabled guard passed with no external tools.");
} finally {
  rmSync(root, { recursive: true, force: true });
}
