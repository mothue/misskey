/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { resolve } from 'node:path';
import { isDeepStrictEqual } from 'node:util';
import * as ts from 'typescript';

export type ResponseSample = {
	endpoint: string;
	status: number;
	body: unknown;
	/** Required for responses whose generated contract declares no content. */
	rawBody?: string | Uint8Array;
	method?: 'post' | 'get';
};

export type TypeDiagnostic = { path: string; code: string; expected: string };
export type ResponseValidation = {
	status: 'PASS' | 'FAIL';
	diagnostics: TypeDiagnostic[];
	/** Any/unknown are opaque in the SDK itself; they are never inferred schemas. */
	opaque: TypeDiagnostic[];
};

export type ResponseValidator = {
	readonly endpoints: readonly string[];
	validate(sample: ResponseSample): ResponseValidation;
};

type Options = { typesPath?: string; maxDepth?: number; maxNodes?: number };

/** Compile the generated declarations once; incoming bodies always stay unknown. */
export function createResponseValidator(options: Options = {}): ResponseValidator {
	const typesPath = resolve(options.typesPath ?? resolve(import.meta.dirname, '../../packages/misskey-js/src/autogen/types.ts'));
	const program = ts.createProgram([typesPath], {
		strict: true, exactOptionalPropertyTypes: true, noEmit: true,
		target: ts.ScriptTarget.ESNext, module: ts.ModuleKind.ESNext,
		moduleResolution: ts.ModuleResolutionKind.Bundler, types: [], skipLibCheck: true,
	});
	const loadedSource = program.getSourceFile(typesPath);
	if (!loadedSource) throw new Error('Generated response type source is unavailable');
	const source = loadedSource;
	const errors = [...program.getSyntacticDiagnostics(source), ...program.getSemanticDiagnostics(source)];
	if (errors.length) throw new Error(`Generated response declarations do not compile (${errors.map(error => error.code).join(',')})`);
	const checker = program.getTypeChecker();
	const module = checker.getSymbolAtLocation(source);
	const pathsSymbol = module && checker.getExportsOfModule(module).find(symbol => symbol.name === 'paths');
	if (!pathsSymbol) throw new Error('Generated response declarations do not export paths');
	const paths = checker.getDeclaredTypeOfSymbol(pathsSymbol);
	const maxDepth = options.maxDepth ?? 128;
	const maxNodes = options.maxNodes ?? 1_000_000;
	if (!Number.isInteger(maxDepth) || maxDepth < 1 || !Number.isInteger(maxNodes) || maxNodes < 1) {
		throw new Error('Response validation limits must be positive integers');
	}

	function property(type: ts.Type, name: string): ts.Type | undefined {
		const symbol = checker.getPropertyOfType(type, name);
		return symbol ? checker.getTypeOfSymbolAtLocation(symbol, symbol.valueDeclaration ?? symbol.declarations?.[0] ?? source) : undefined;
	}
	function expected(type: ts.Type): string {
		return checker.typeToString(type, source, ts.TypeFormatFlags.NoTruncation);
	}
	function issue(path: string, code: string, expectation: string): TypeDiagnostic {
		return { path, code, expected: expectation };
	}
	function child(path: string, key: string | number): string {
		return `${path}[${JSON.stringify(key)}]`;
	}
	const contracts = new Map<string, ts.Type | null | TypeDiagnostic>();
	function contract(sample: ResponseSample): ts.Type | null | TypeDiagnostic {
		const key = `${sample.endpoint}\0${sample.method ?? 'post'}\0${sample.status}`;
		const cached = contracts.get(key);
		if (cached !== undefined) return cached;
		const path = property(paths, `/${sample.endpoint.replace(/^\//, '')}`);
		const operation = path && property(path, sample.method ?? 'post');
		const responses = operation && property(operation, 'responses');
		const response = responses && (property(responses, String(sample.status)) ?? property(responses, 'default'));
		let result: ts.Type | null | TypeDiagnostic;
		if (!path || !operation || !responses || !response) {
			result = issue('$', 'undeclared-response', 'A generated endpoint/method/status response');
		} else {
			const content = property(response, 'content');
			if (!content) result = null;
			else result = property(content, 'application/json') ?? issue('$', 'unsupported-content', 'A generated application/json response');
		}
		contracts.set(key, result);
		return result;
	}

	function validate(sample: ResponseSample): ResponseValidation {
		const diagnostics: TypeDiagnostic[] = [];
		const opaque: TypeDiagnostic[] = [];
		const type = contract(sample);
		if (type === null) {
			if (sample.rawBody === undefined) diagnostics.push(issue('$', 'raw-body-required', 'Zero response bytes'));
			else if (sample.rawBody.length !== 0) diagnostics.push(issue('$', 'unexpected-body', 'Zero response bytes'));
			return { status: diagnostics.length ? 'FAIL' : 'PASS', diagnostics, opaque };
		}
		if ('code' in type) return { status: 'FAIL', diagnostics: [type], opaque };
		if (sample.rawBody !== undefined && sample.rawBody.length === 0) {
			return { status: 'FAIL', diagnostics: [issue('$', 'missing-body', expected(type))], opaque };
		}
		let visited = 0;
		const ancestors = new WeakSet<object>();
		function json(value: unknown, path: string, depth: number): boolean {
			if (++visited > maxNodes || depth > maxDepth) {
				diagnostics.push(issue(path, 'resource-limit', 'JSON within configured validation limits'));
				return false;
			}
			if (value === null || typeof value === 'string' || typeof value === 'boolean') return true;
			if (typeof value === 'number' && Number.isFinite(value)) return true;
			if (typeof value !== 'object' || value === null || (!Array.isArray(value) && ![Object.prototype, null].includes(Object.getPrototypeOf(value)))) {
				diagnostics.push(issue(path, 'non-json-value', 'A parsed JSON value'));
				return false;
			}
			if (ancestors.has(value)) {
				diagnostics.push(issue(path, 'cyclic-value', 'An acyclic parsed JSON value'));
				return false;
			}
			ancestors.add(value);
			const keys: Iterable<string | number> = Array.isArray(value) ? value.keys() : Object.keys(value);
			let valid = true;
			for (const key of keys) {
				if (!json(Reflect.get(value, key), child(path, key), depth + 1)) { valid = false; break; }
			}
			ancestors.delete(value);
			return valid;
		}
		if (!json(sample.body, '$', 0)) return { status: 'FAIL', diagnostics, opaque };
		visited = 0;

		function walk(type: ts.Type, value: unknown, path: string, depth: number, out: TypeDiagnostic[], untyped: TypeDiagnostic[]): void {
			if (++visited > maxNodes || depth > maxDepth) {
				out.push(issue(path, 'resource-limit', 'Value/type traversal within configured validation limits'));
				return;
			}
			const flags = type.flags;
			if (flags & (ts.TypeFlags.Any | ts.TypeFlags.Unknown)) {
				untyped.push(issue(path, 'opaque-sdk-type', expected(type)));
				return;
			}
			if (type.isUnion()) {
				let closest: TypeDiagnostic[] | undefined;
				for (const branch of type.types) {
					const errors: TypeDiagnostic[] = [], branchOpaque: TypeDiagnostic[] = [];
					walk(branch, value, path, depth + 1, errors, branchOpaque);
					if (!errors.length) { untyped.push(...branchOpaque); return; }
					if (!closest || errors.length < closest.length) closest = errors;
				}
				out.push(issue(path, 'union', expected(type)), ...(closest ?? []));
				return;
			}
			if (type.isIntersection()) {
				for (const branch of type.types) walk(branch, value, path, depth + 1, out, untyped);
				return;
			}
			if (flags & ts.TypeFlags.Never) { out.push(issue(path, 'never', 'never')); return; }
			if (flags & ts.TypeFlags.Null) { if (value !== null) out.push(issue(path, 'type', 'null')); return; }
			if (flags & (ts.TypeFlags.Undefined | ts.TypeFlags.Void)) { out.push(issue(path, 'non-json-type', expected(type))); return; }
			if (type.isStringLiteral()) { if (value !== type.value) out.push(issue(path, 'literal', expected(type))); return; }
			if (type.isNumberLiteral()) { if (value !== type.value) out.push(issue(path, 'literal', expected(type))); return; }
			if (flags & ts.TypeFlags.BooleanLiteral) { if (value !== (expected(type) === 'true')) out.push(issue(path, 'literal', expected(type))); return; }
			for (const [flag, kind] of [[ts.TypeFlags.String, 'string'], [ts.TypeFlags.Number, 'number'], [ts.TypeFlags.Boolean, 'boolean']] as const) {
				if (flags & flag) { if (typeof value !== kind) out.push(issue(path, 'type', kind)); return; }
			}
			if (flags & ts.TypeFlags.NonPrimitive) {
				if (typeof value !== 'object' || value === null) out.push(issue(path, 'type', 'object'));
				else untyped.push(issue(path, 'opaque-sdk-type', 'object'));
				return;
			}
			if (!(flags & ts.TypeFlags.Object)) { out.push(issue(path, 'unsupported-type', expected(type))); return; }
			if (checker.isTupleType(type)) {
				if (!Array.isArray(value)) { out.push(issue(path, 'type', expected(type))); return; }
				// These are compiler-owned type nodes, never casts of response data.
				const tuple = type as ts.TypeReference;
				const target = tuple.target as ts.TupleType;
				const elements = checker.getTypeArguments(tuple);
				const rest = target.elementFlags.findIndex(flag => Boolean(flag & (ts.ElementFlags.Rest | ts.ElementFlags.Variadic)));
				if (rest >= 0 && rest !== elements.length - 1) { out.push(issue(path, 'unsupported-tuple', expected(type))); return; }
				if (value.length < target.minLength || (rest < 0 && value.length > elements.length)) out.push(issue(path, 'tuple-length', expected(type)));
				for (let i = 0; i < value.length; i++) {
					const element = elements[Math.min(i, rest >= 0 ? rest : elements.length)];
					if (element) walk(element, value[i], child(path, i), depth + 1, out, untyped);
				}
				return;
			}
			if (checker.isArrayType(type)) {
				if (!Array.isArray(value)) { out.push(issue(path, 'type', expected(type))); return; }
				const element = checker.getIndexTypeOfType(type, ts.IndexKind.Number);
				if (!element) { out.push(issue(path, 'unsupported-array', expected(type))); return; }
				for (let i = 0; i < value.length; i++) walk(element, value[i], child(path, i), depth + 1, out, untyped);
				return;
			}
			if (type.getCallSignatures().length || type.getConstructSignatures().length || type.getSymbol()?.declarations?.some(declaration =>
				ts.isClassDeclaration(declaration) || ts.isClassExpression(declaration)
				|| (ts.isInterfaceDeclaration(declaration) && program.isSourceFileDefaultLibrary(declaration.getSourceFile())))) {
				out.push(issue(path, 'unsupported-type', expected(type)));
				return;
			}
			const unsupportedIndex = checker.getIndexInfosOfType(type).find(index => !(index.keyType.flags & (ts.TypeFlags.String | ts.TypeFlags.Number)));
			if (unsupportedIndex) {
				out.push(issue(path, 'unsupported-index', expected(unsupportedIndex.keyType)));
				return;
			}
			// The TypeScript empty object type means any non-nullish value. Do not
			// silently reinterpret it as a stronger JSON Schema object contract.
			if (!checker.getPropertiesOfType(type).length && !checker.getIndexInfosOfType(type).length) {
				if (value === null) out.push(issue(path, 'type', expected(type)));
				else untyped.push(issue(path, 'opaque-sdk-type', expected(type)));
				return;
			}
			if (typeof value !== 'object' || value === null || Array.isArray(value)) { out.push(issue(path, 'type', expected(type))); return; }
			const properties = new Set<string>();
			for (const symbol of checker.getPropertiesOfType(type)) {
				properties.add(symbol.name);
				if (!Object.hasOwn(value, symbol.name)) {
					if (!(symbol.flags & ts.SymbolFlags.Optional)) out.push(issue(child(path, symbol.name), 'missing-property', expected(type)));
					continue;
				}
				const field = property(type, symbol.name);
				if (!field) out.push(issue(child(path, symbol.name), 'unsupported-property', expected(type)));
				else walk(field, Reflect.get(value, symbol.name), child(path, symbol.name), depth + 1, out, untyped);
			}
			const stringIndex = checker.getIndexTypeOfType(type, ts.IndexKind.String);
			const numberIndex = checker.getIndexTypeOfType(type, ts.IndexKind.Number);
			for (const key of Object.keys(value)) {
				const index = numberIndex && Number.isFinite(Number(key)) && String(Number(key)) === key ? numberIndex : stringIndex;
				if (index && !properties.has(key)) walk(index, Reflect.get(value, key), child(path, key), depth + 1, out, untyped);
			}
		}
		walk(type, sample.body, '$', 0, diagnostics, opaque);
		return { status: diagnostics.length ? 'FAIL' : 'PASS', diagnostics, opaque };
	}
	return { endpoints: checker.getPropertiesOfType(paths).map(symbol => symbol.name.replace(/^\//, '')), validate };
}

/** SDK conformance is reported separately from exact TS/native parity. */
export function compareResponseConformance(validator: ResponseValidator, baseline: ResponseSample, candidate: ResponseSample) {
	const baselineResult = validator.validate(baseline), candidateResult = validator.validate(candidate);
	const bothEmpty = baseline.rawBody?.length === 0 && candidate.rawBody?.length === 0;
	const parity = baseline.endpoint === candidate.endpoint && baseline.status === candidate.status
		&& (baseline.method ?? 'post') === (candidate.method ?? 'post')
		&& (bothEmpty || isDeepStrictEqual(baseline.body, candidate.body));
	const status = !parity ? 'FAIL'
		: baselineResult.status === 'PASS' && candidateResult.status === 'PASS' ? 'PASS'
			: baselineResult.status === 'FAIL' && candidateResult.status === 'FAIL' && isDeepStrictEqual(baselineResult.diagnostics, candidateResult.diagnostics) ? 'BASELINE' : 'FAIL';
	return { status, parity: parity ? 'PASS' : 'FAIL', baseline: baselineResult, candidate: candidateResult } as const;
}
