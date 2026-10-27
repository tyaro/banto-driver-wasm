import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';

const required = {
  'README.md': ['# banto-driver-wasm'],
  'AGENTS.md': ['docs/STATUS.md'],
  'docs/PLAN.md': ['# 段階計画'],
  'docs/CONTRACT.md': ['# API と WIT の照合'],
};

for (const [path, markers] of Object.entries(required)) {
  if (!existsSync(path)) throw new Error(`Required document missing: ${path}`);
  const body = readFileSync(path, 'utf8');
  for (const marker of markers) {
    if (!body.includes(marker)) throw new Error(`${path} missing: ${marker}`);
  }
}

// Unknown files need a deliberate CI rule. In this bootstrap repository every
// tracked project file has an explicit place; future source/config must update CI.
const allowed = new Set([
  ...Object.keys(required),
  // Local, untracked hand-off note (.gitignore); allowed when present.
  'docs/STATUS.md',
  'docs/SLMP_DISCOVERY_CONTRACT.md',
  '.github/scripts/ci-changes.mjs',
  '.github/scripts/ci-changes.test.mjs',
  '.github/scripts/check-scaffold.mjs',
  '.github/workflows/ci.yml',
  '.gitignore',
  'Cargo.toml',
  'Cargo.lock',
  'rust-toolchain.toml',
  'drivers/modbus/Cargo.toml',
  'drivers/modbus/src/lib.rs',
  'drivers/slmp/Cargo.toml',
  'drivers/slmp/Cargo.lock',
  'drivers/slmp/src/lib.rs',
  'drivers/slmp/src/q_zr.rs',
  'drivers/slmp/src/q_extension.rs',
  'drivers/slmp/src/sd_basic.rs',
  'drivers/slmp/src/type_name.rs',
  'drivers/slmp/tests/q_zr.rs',
  'drivers/slmp/tests/q_extension.rs',
  'drivers/slmp/tests/read_frame.rs',
  'drivers/slmp/tests/sd_basic.rs',
  'drivers/slmp/tests/type_name.rs',
  'drivers/slmp/tests/write_frame.rs',
  'slmp-component/Cargo.toml',
  'slmp-component/Cargo.lock',
  'slmp-component/src/lib.rs',
  'slmp-component/src/diagnostics.rs',
  'slmp-component/src/write_policy.rs',
  'slmp-runtime/Cargo.toml',
  'slmp-runtime/Cargo.lock',
  'slmp-runtime/src/lib.rs',
  'slmp-runtime/tests/diagnostics.rs',
  'slmp-runtime/tests/range_matrix.rs',
  'comparison/Cargo.toml',
  'comparison/Cargo.lock',
  'comparison/src/lib.rs',
  'component/Cargo.toml',
  'component/Cargo.lock',
  'component/src/lib.rs',
  'component/manifest.json',
  'runtime-comparison/Cargo.toml',
  'runtime-comparison/Cargo.lock',
  'runtime-comparison/src/lib.rs',
  'wit/driver.wit',
  'wit/write-v1.wit',
  'wit/diagnostics-v1.wit',
]);
function checkTree(dir) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (dir === '.' && (entry.name === '.git' || entry.name === 'target' || entry.name === 'dist')) continue;
    const path = join(dir, entry.name).replaceAll('\\', '/');
    if (path === 'comparison/target' || path === 'component/target' || path === 'runtime-comparison/target' || path === 'drivers/slmp/target'
      || path === 'slmp-component/target' || path === 'slmp-runtime/target') continue;
    if (entry.isDirectory()) checkTree(path);
    else if (!allowed.has(path) && !/^(sdk|wit|drivers\/modbus|drivers\/slmp)\/README\.md$/.test(path)) {
      throw new Error(`${path}: add the needed CI check before introducing this file`);
    }
  }
}
checkTree('.');

// Until a real build/test job replaces this guard, source files must not pass silently.
const area = process.argv[2];
const roots = { sdk: ['sdk'] };
if (area && !roots[area]) throw new Error(`Unknown area: ${area}`);
function visit(dir) {
  if (!existsSync(dir)) return;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) visit(path);
    else if (entry.name !== 'README.md' && entry.name !== '.gitkeep') {
      throw new Error(`${path}: product source found; replace the ${area} guard with real build and tests`);
    }
  }
}
for (const root of roots[area] ?? []) visit(root);
console.log(`Scaffold check passed${area ? ` (${area}: no product source)` : ''}`);
