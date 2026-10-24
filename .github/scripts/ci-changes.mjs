import { appendFileSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const all = { sdk: true, modbus: true, slmp: true };

/** Fail closed: an unknown or shared path selects every area. */
export function classify(event, files) {
  if (event !== 'pull_request' || !files?.length) return { ...all, reason: 'full' };
  const selected = { sdk: false, modbus: false, slmp: false };
  let reason = 'paths';
  for (const file of files) {
    if (file.startsWith('sdk/') || file.startsWith('wit/')) {
      Object.assign(selected, all); // SDK/WIT can affect every consumer.
    } else if (file.startsWith('drivers/modbus/') || file.startsWith('comparison/')) {
      selected.modbus = true;
    } else if (file.startsWith('drivers/slmp/')) {
      selected.slmp = true;
    } else if (/^(docs\/.*\.md|[^/]+\.md)$/.test(file)) {
      // The scaffold check always validates the public documentation.
    } else {
      Object.assign(selected, all);
      reason = 'full';
    }
  }
  return { ...selected, reason };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const event = process.env.CI_EVENT ?? '';
  const files = process.env.CI_DIFF_OK === 'true'
    ? readFileSync(process.env.CI_CHANGED_FILES, 'utf8').split('\0').filter(Boolean)
    : null;
  const selected = classify(event, files);
  const lines = [
    `sdk=${selected.sdk}`,
    `modbus=${selected.modbus}`,
    `slmp=${selected.slmp}`,
    `reason=${selected.reason}`,
  ];
  console.log(`CI scope: ${lines.join(', ')}`);
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, lines.join('\n') + '\n');
  if (process.env.GITHUB_STEP_SUMMARY) {
    appendFileSync(process.env.GITHUB_STEP_SUMMARY, `### CI scope\n\n${lines.join('  \n')}\n`);
  }
}
