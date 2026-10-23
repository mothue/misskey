/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
export function distribution(values: number[]) {
 if (!values.length) throw new Error('empty distribution');
 const sorted = values.toSorted((a, b) => a - b);
 const mean = values.reduce((a, b) => a + b, 0) / values.length;
 return { median: sorted[Math.floor(sorted.length / 2)], min: sorted[0], max: sorted.at(-1)!, variance: values.reduce((sum, value) => sum + (value - mean) ** 2, 0) / values.length };
}

if (import.meta.main) {
 const root = new URL('../../../built/ts-to-rs/', import.meta.url);
 const data = await Bun.file(new URL('paired-results.json', root)).json();
 const output = [];
 for (const name of ['ports/ping', 'ports/test', 'ports/endpoint']) {
  const pairs = [1, 2, 3].map(pair => {
   const baseline = data.results.find((entry: { label: string; pair: number }) => entry.label === 'baseline' && entry.pair === pair);
   const candidate = data.results.find((entry: { label: string; pair: number }) => entry.label === 'candidate' && entry.pair === pair);
   const a = baseline.scenarios.find((scenario: { name: string }) => scenario.name === name);
   const b = candidate.scenarios.find((scenario: { name: string }) => scenario.name === name);
   return { pair, baseline: a, candidate: b, startupMs: { baseline: baseline.startupMs, candidate: candidate.startupMs }, percentage: Object.fromEntries(['p50', 'p95', 'p99'].map(metric => [metric, (b.measure.latencyMs[metric] / a.measure.latencyMs[metric] - 1) * 100]).concat([['throughput', (b.measure.throughput / a.measure.throughput - 1) * 100]])) };
  });
  const metrics = Object.fromEntries(['baseline', 'candidate'].map(label => [label, Object.fromEntries(['p50', 'p95', 'p99', 'throughput', 'tsKiB', 'rustKiB', 'startupMs'].map(metric => [metric, distribution(pairs.map(pair => {
   const scenario = pair[label as 'baseline' | 'candidate'];
   return metric === 'startupMs' ? pair.startupMs[label as 'baseline' | 'candidate'] : metric === 'throughput' ? scenario.measure.throughput : metric.endsWith('KiB') ? scenario.memory[metric] : scenario.measure.latencyMs[metric];
  }))]))]));
  output.push({ name, metrics, pairedPercentage: Object.fromEntries(['p50', 'p95', 'p99', 'throughput'].map(metric => [metric, distribution(pairs.map(pair => pair.percentage[metric]))])), pairs });
 }
 await Bun.write(new URL('paired-summary.json', root), JSON.stringify({ statistics: 'median, range and population variance across 3 runs; percentage=(candidate/baseline-1)*100', results: output }, null, 2));
}

export function completedOrder(results: { pair: number; label: string }[]) {
 const pairs = new Map<number, string>();
 for (const result of results) pairs.set(result.pair, (pairs.get(result.pair) ?? '') + (result.label === 'baseline' ? 'A' : 'B'));
 return [...pairs.values()];
}
