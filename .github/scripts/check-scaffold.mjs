import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';

const required = {
  'README.md': ['# banto-driver-wasm', 'docs/STATUS.md'],
  'AGENTS.md': ['docs/STATUS.md'],
  'docs/STATUS.md': ['## 完了', '## 検証', '## 未検証', '## 残件', '## 次作業', '## 判断'],
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
]);
function checkTree(dir) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (dir === '.' && (entry.name === '.git' || entry.name === 'target')) continue;
    const path = join(dir, entry.name).replaceAll('\\', '/');
    if (entry.isDirectory()) checkTree(path);
    else if (!allowed.has(path) && !/^(sdk|wit|drivers\/modbus|drivers\/slmp)\/README\.md$/.test(path)) {
      throw new Error(`${path}: add the needed CI check before introducing this file`);
    }
  }
}
checkTree('.');

// Until a real build/test job replaces this guard, source files must not pass silently.
const area = process.argv[2];
const roots = { sdk: ['sdk', 'wit'], slmp: ['drivers/slmp'] };
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
