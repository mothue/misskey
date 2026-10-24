/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

export type Hold = { endpoint: string; kind: 'performance' | 'compatibility'; reason: string; evidence: string };
export type StatusRow = { endpoint: string; status: 'Native' | 'TS' | 'Held (TS)'; hold?: Hold };
export type Inventory = { rows: StatusRow[]; counts: { total: number; native: number; ts: number; held: number } };

const sourcePaths = {
	registry: 'packages/backend/src/server/api/endpoint-list.ts',
	core: 'packages/backend-rust/api-runtime/src/lib.rs',
	business: 'packages/backend-rust/api-runtime/src/business/mod.rs',
	guard: 'packages/backend/src/server/api/ApiServerService.ts',
	holds: 'scripts/ts-to-rs/native-holds.json',
};
const outputPath = 'docs/ts-to-rs/native-status.md';

function sourceFile(text: string): ts.SourceFile {
	const source = ts.createSourceFile('registry.ts', text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
	const diagnostics = ts.transpileModule(text, { reportDiagnostics: true, compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ESNext } }).diagnostics ?? [];
	if (diagnostics.some(diagnostic => diagnostic.category === ts.DiagnosticCategory.Error)) throw new Error('Invalid TypeScript status source.');
	return source;
}

export function registryNames(text: string): string[] {
	const names: string[] = [];
	for (const statement of sourceFile(text).statements) {
		if (!ts.isExportDeclaration(statement)) continue;
		if (!statement.exportClause || !ts.isNamespaceExport(statement.exportClause)
			|| !statement.moduleSpecifier || !ts.isStringLiteral(statement.moduleSpecifier)) {
			throw new Error('Unsupported API registry export; update the status generator.');
		}
		names.push(statement.exportClause.name.text);
	}
	return unique(names, 'registry');
}

export function guardNames(text: string): string[] {
	const candidates: string[][] = [];
	function visit(node: ts.Node) {
		if (ts.isMethodDeclaration(node) && node.name.getText() === 'executeLegacy' && node.body) {
			function findGuard(child: ts.Node) {
				if (ts.isIfStatement(child) && ts.isCallExpression(child.expression)) {
					const call = child.expression;
					if (ts.isPropertyAccessExpression(call.expression) && call.expression.name.text === 'includes'
						&& ts.isArrayLiteralExpression(call.expression.expression)
						&& call.arguments.length === 1 && call.arguments[0]?.getText() === 'invocation.endpoint'
						&& child.thenStatement.getText().includes('Native API cannot execute through TS')) {
						candidates.push(call.expression.expression.elements.map(element => {
							if (!ts.isStringLiteral(element)) throw new Error('Callback guard must contain endpoint string literals.');
							return element.text;
						}));
					}
				}
				ts.forEachChild(child, findGuard);
			}
			findGuard(node.body);
		}
		ts.forEachChild(node, visit);
	}
	visit(sourceFile(text));
	if (candidates.length !== 1) throw new Error('Expected exactly one native callback rejection guard.');
	return unique(candidates[0]!, 'callback guard');
}

type RustToken = { kind: 'string' | 'other'; text: string };

// This deliberately recognizes the small source forms used by dispatch and its
// literal allowlist. Unsupported forms fail rather than silently miscounting.
function rustTokens(text: string): RustToken[] {
	const tokens: RustToken[] = [];
	for (let at = 0; at < text.length;) {
		const char = text[at]!;
		if (/\s/.test(char)) { at++; continue; }
		if (text.startsWith('//', at)) {
			const end = text.indexOf('\n', at + 2);
			at = end < 0 ? text.length : end + 1;
			continue;
		}
		if (text.startsWith('/*', at)) {
			at += 2;
			let depth = 1;
			while (at < text.length && depth) {
				if (text.startsWith('/*', at)) { depth++; at += 2; }
				else if (text.startsWith('*/', at)) { depth--; at += 2; }
				else at++;
			}
			if (depth) throw new Error('Unclosed Rust block comment.');
			continue;
		}
		if (char === '"') {
			const start = at++;
			while (at < text.length && text[at] !== '"') {
				if (text[at] === '\\') at++;
				at++;
			}
			if (at === text.length) throw new Error('Unclosed Rust string.');
			const value: unknown = JSON.parse(text.slice(start, ++at));
			if (typeof value !== 'string') throw new Error('Expected a Rust endpoint string.');
			tokens.push({ kind: 'string', text: value });
			continue;
		}
		const operator = text.startsWith('=>', at) ? '=>' : char;
		tokens.push({ kind: 'other', text: operator });
		at += operator.length;
	}
	return tokens;
}

function afterUniqueMarker(text: string, marker: string): string {
	const at = text.indexOf(marker);
	if (at < 0 || text.indexOf(marker, at + marker.length) >= 0) {
		throw new Error(`Expected one source marker: ${marker}`);
	}
	return text.slice(at + marker.length);
}

export function coreNames(text: string): string[] {
	const functionTail = afterUniqueMarker(text, 'async fn dispatch_endpoint(');
	const matchTail = afterUniqueMarker(functionTail, 'match endpoint.name.as_str()');
	const nextMethod = matchTail.indexOf('\n    async fn ');
	const tokens = rustTokens(nextMethod < 0 ? matchTail : matchTail.slice(0, nextMethod));
	if (tokens[0]?.text !== '{') throw new Error('Native dispatch must start with a match block.');
	const names: string[] = [];
	let depth = 0;
	let foundFallback = false;
	for (let i = 0; i < tokens.length; i++) {
		const token = tokens[i]!;
		if (token.kind === 'other' && token.text === '{') depth++;
		else if (token.kind === 'other' && token.text === '}') {
			if (--depth === 0) {
				if (!foundFallback) throw new Error('Native dispatch fallback not found.');
				return unique(names, 'native core dispatch');
			}
		} else if (depth === 1 && token.kind === 'other' && token.text === '|') {
			throw new Error('Unsupported native dispatch alternatives; update the status generator.');
		} else if (depth === 1 && tokens[i + 1]?.text === '=>') {
			if (token.kind === 'string') names.push(token.text);
			else if (token.text === '_' && !foundFallback) foundFallback = true;
			else throw new Error('Unsupported native dispatch pattern; update the status generator.');
		}
	}
	throw new Error('Native dispatch match block is unclosed.');
}

export function businessNames(text: string): string[] {
	const tail = afterUniqueMarker(text, 'const ACTIVE_READ_ENDPOINTS:');
	const end = tail.indexOf(';');
	if (end < 0) throw new Error('Unclosed native business allowlist.');
	const expression = tail.slice(0, end);
	const assignment = expression.indexOf('=');
	if (assignment < 0) throw new Error('Native business allowlist assignment not found.');
	const tokens = rustTokens(expression.slice(assignment + 1));
	if (tokens[0]?.text !== '&' || tokens[1]?.text !== '[' || tokens.at(-1)?.text !== ']') {
		throw new Error('Unsupported native business allowlist expression.');
	}
	const names: string[] = [];
	let expectName = true;
	for (const token of tokens.slice(2, -1)) {
		if (expectName && token.kind === 'string') { names.push(token.text); expectName = false; }
		else if (!expectName && token.kind === 'other' && token.text === ',') expectName = true;
		else throw new Error('Native business allowlist must contain only endpoint string literals.');
	}
	return unique(names, 'native business allowlist');
}

function unique(names: string[], label: string): string[] {
	if (new Set(names).size !== names.length) throw new Error(`Duplicate endpoint in ${label}.`);
	if (names.some(name => !/^[a-z0-9][a-z0-9/_-]*$/.test(name))) throw new Error(`Invalid endpoint name in ${label}.`);
	return names;
}

export function decodeHolds(value: unknown): Hold[] {
	if (!value || typeof value !== 'object' || !('version' in value) || value.version !== 1
		|| !('holds' in value) || !Array.isArray(value.holds)) throw new Error('Invalid native hold manifest.');
	const holds = value.holds.map((entry: unknown): Hold => {
		if (!entry || typeof entry !== 'object' || !('endpoint' in entry) || typeof entry.endpoint !== 'string'
			|| !('kind' in entry) || (entry.kind !== 'performance' && entry.kind !== 'compatibility')
			|| !('reason' in entry) || typeof entry.reason !== 'string' || !entry.reason.trim()
			|| !('evidence' in entry) || typeof entry.evidence !== 'string' || !entry.evidence.trim()) {
			throw new Error('Invalid native hold entry.');
		}
		return { endpoint: entry.endpoint, kind: entry.kind, reason: entry.reason, evidence: entry.evidence };
	});
	unique(holds.map(hold => hold.endpoint), 'hold manifest');
	return holds;
}

export function classify(registry: string[], core: string[], business: string[], guard: string[], holds: Hold[]): Inventory {
	const known = new Set(unique(registry, 'registry'));
	const active = new Set(unique([...core, ...business], 'combined native selection'));
	const rejected = new Set(unique(guard, 'callback guard'));
	const held = new Map(decodeHolds({ version: 1, holds }).map(hold => [hold.endpoint, hold]));
	for (const endpoint of [...active, ...rejected, ...held.keys()]) {
		if (!known.has(endpoint)) throw new Error(`Unknown classified endpoint: ${endpoint}`);
	}
	for (const endpoint of new Set([...active, ...rejected])) {
		if (active.has(endpoint) !== rejected.has(endpoint)) throw new Error(`Native/callback guard mismatch: ${endpoint}`);
	}
	for (const endpoint of held.keys()) {
		if (active.has(endpoint)) throw new Error(`Held endpoint is active: ${endpoint}`);
	}
	const rows = registry.map((endpoint): StatusRow => {
		if (active.has(endpoint)) return { endpoint, status: 'Native' };
		const hold = held.get(endpoint);
		return hold ? { endpoint, status: 'Held (TS)', hold } : { endpoint, status: 'TS' };
	});
	return { rows, counts: { total: rows.length, native: active.size, held: held.size, ts: rows.length - active.size - held.size } };
}

const markdown = (text: string) => text.replaceAll('|', '\\|').replaceAll('\n', ' ');

export async function generate(root: string): Promise<{ markdown: string; inventory: Inventory }> {
	const files = await Promise.all(Object.values(sourcePaths).map(async path => ({ path, text: await readFile(resolve(root, path), 'utf8') })));
	const [registry, core, business, guard, holds] = files;
	if (!registry || !core || !business || !guard || !holds) throw new Error('Missing status sources.');
	const manifest: unknown = JSON.parse(holds.text);
	const inventory = classify(registryNames(registry.text), coreNames(core.text), businessNames(business.text), guardNames(guard.text), decodeHolds(manifest));
	const { counts } = inventory;
	const lines = [
		'# Generated native API status', '',
		'Generated by `bun scripts/ts-to-rs/native-status.ts`. Verify with `bun scripts/ts-to-rs/native-status.ts --check`. Do not edit this table manually.', '',
		'Native means the complete business handler is selected in Rust; the public TS listener remains. Held endpoints still execute their TS body. Unported endpoints are marked TS. This records source selection, not test or CI acceptance.', '',
		'| Total | Native | TS | Held (TS) |', '| ---: | ---: | ---: | ---: |', `| ${counts.total} | ${counts.native} | ${counts.ts} | ${counts.held} |`, '',
		'Names and enumeration order come from the API registry. Native core match arms and the business allowlist are read from Rust source and must match the TS callback rejection guard. Hold reasons come from the hold manifest.', '',
		'| Endpoint | Status | Hold reason |', '| --- | --- | --- |',
		...inventory.rows.map(row => `| \`${row.endpoint}\` | ${row.status} | ${row.hold ? `${row.hold.kind}: ${markdown(row.hold.reason)} ([evidence](${row.hold.evidence}))` : ''} |`), '',
		'## Source fingerprints', '', '| Source | SHA-256 |', '| --- | --- |',
		...files.map(file => `| \`${file.path}\` | \`${createHash('sha256').update(file.text).digest('hex')}\` |`), '',
	];
	return { markdown: lines.join('\n'), inventory };
}

if (import.meta.main) {
	try {
		if (process.argv.slice(2).some(arg => arg !== '--check')) throw new Error('Usage: bun scripts/ts-to-rs/native-status.ts [--check]');
		const root = fileURLToPath(new URL('../../', import.meta.url));
		const generated = await generate(root);
		const check = process.argv.includes('--check');
		if (check) {
			if (await readFile(resolve(root, outputPath), 'utf8') !== generated.markdown) throw new Error('Native status table is stale; regenerate it.');
		} else await writeFile(resolve(root, outputPath), generated.markdown);
		console.log(JSON.stringify({ status: 'PASS', mode: check ? 'check' : 'generate', output: outputPath, counts: generated.inventory.counts }));
	} catch (error) {
		console.error(error instanceof Error ? error.message : 'Native status generation failed.');
		process.exitCode = 1;
	}
}
