import { strict as assert } from 'node:assert';
import { test } from 'node:test';
import { classify } from './ci-changes.mjs';

const cases = [
  ['main push', 'push', ['docs/PLAN.md'], [true, true, true]],
  ['missing diff', 'pull_request', null, [true, true, true]],
  ['empty diff', 'pull_request', [], [true, true, true]],
  ['docs only', 'pull_request', ['README.md', 'docs/STATUS.md'], [false, false, false]],
  ['Modbus only', 'pull_request', ['drivers/modbus/src/lib.rs'], [false, true, false]],
  ['native comparison', 'pull_request', ['comparison/src/lib.rs'], [false, true, false]],
  ['SLMP only', 'pull_request', ['drivers/slmp/src/lib.rs'], [false, false, true]],
  ['SDK affects all', 'pull_request', ['sdk/src/lib.rs'], [true, true, true]],
  ['WIT affects all', 'pull_request', ['wit/driver.wit'], [true, true, true]],
  ['unknown path', 'pull_request', ['other/file.rs'], [true, true, true]],
  ['common config', 'pull_request', ['Cargo.lock'], [true, true, true]],
  ['workflow', 'pull_request', ['.github/workflows/ci.yml'], [true, true, true]],
  ['mixed', 'pull_request', ['docs/PLAN.md', 'drivers/modbus/src/lib.rs'], [false, true, false]],
];

for (const [name, event, files, expected] of cases) {
  test(name, () => {
    const { sdk, modbus, slmp } = classify(event, files);
    assert.deepEqual([sdk, modbus, slmp], expected);
  });
}
