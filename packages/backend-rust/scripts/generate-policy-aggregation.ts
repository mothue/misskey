/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { readFileSync, writeFileSync } from 'node:fs';
import ts from 'typescript';

type Aggregation = 'Any' | 'Max' | 'MaxFileSize' | 'Chat' | 'FileTypes';
export type PolicyAggregation = Readonly<{ name: string; aggregation: Aggregation }>;

const roleSourcePath = new URL('../../backend/src/core/RoleService.ts', import.meta.url);
const descriptorPath = new URL('../api-auth/src/policy_aggregation.rs', import.meta.url);
const generateCommand = 'bun packages/backend-rust/scripts/generate-policy-aggregation.ts';
const printer = ts.createPrinter({ removeComments: true });

// These are supported operations, not a second policy-name table. Reject a
// changed body until its Rust implementation has been reviewed alongside it.
const aggregators: ReadonlyArray<readonly [Exclude<Aggregation, 'Chat'>, string]> = [
	['Any', 'vs => vs.some(v => v === true)'],
	['Max', 'vs => Math.max(...vs)'],
	['MaxFileSize', 'vs => Math.min(serverMaxFileSizeMb, Math.max(...vs))'],
	['FileTypes', [
		'vs => {',
		' const set = new Set<string>();',
		' for (const v of vs) {',
		'  for (const type of v) {',
		"   if (type.trim() === '') continue;",
		'   set.add(type.trim());',
		'  }',
		' }',
		' return [...set];',
		'}',
	].join('\n')],
];

const policyPrelude = [
	'const basePolicies = { ...DEFAULT_POLICIES, ...this.meta.policies };',
	'if (userId == null) return basePolicies;',
	'const roles = await this.getUserRoles(userId);',
	'function calc<T extends keyof RolePolicies>(name: T, aggregate: (values: RolePolicies[T][]) => RolePolicies[T]) {',
	' if (roles.length === 0) return aggregate([basePolicies[name]]);',
	' const policies = roles.map(role => role.policies[name] ?? { priority: 0, useDefault: true });',
	' const p2 = policies.filter(policy => policy.priority === 2);',
	' if (p2.length > 0) return aggregate(p2.map(policy => policy.useDefault ? basePolicies[name] : policy.value));',
	' const p1 = policies.filter(policy => policy.priority === 1);',
	' if (p1.length > 0) return aggregate(p1.map(policy => policy.useDefault ? basePolicies[name] : policy.value));',
	' return aggregate(policies.map(policy => policy.useDefault ? basePolicies[name] : policy.value));',
	'}',
	"function aggregateChatAvailability(vs: RolePolicies['chatAvailability'][]) {",
	" if (vs.some(v => v === 'available')) return 'available';",
	" if (vs.some(v => v === 'readonly')) return 'readonly';",
	" return 'unavailable';",
	'}',
	'const serverMaxFileSizeMb = Math.floor(this.config.maxFileSize / (1024 * 1024));',
].join('\n');

function unsupported(detail: string): never {
	throw new Error('Unsupported RoleService policy source: ' + detail);
}

function parse(source: string): ts.SourceFile {
	const diagnostics = ts.transpileModule(source, {
		fileName: 'RoleService.ts',
		reportDiagnostics: true,
		compilerOptions: { target: ts.ScriptTarget.ESNext, experimentalDecorators: true },
	}).diagnostics ?? [];
	if (diagnostics.some(diagnostic => diagnostic.category === ts.DiagnosticCategory.Error)) {
		unsupported('invalid TypeScript syntax');
	}
	return ts.createSourceFile('RoleService.ts', source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
}

// Printing selected AST nodes removes formatting and comments. Token values
// normalize quote styles while preserving operators, identifiers and literals.
function fingerprint(node: ts.Node, file: ts.SourceFile): string {
	const scanner = ts.createScanner(ts.ScriptTarget.Latest, true, ts.LanguageVariant.Standard,
		printer.printNode(ts.EmitHint.Unspecified, node, file));
	const tokens: Array<readonly [ts.SyntaxKind, string]> = [];
	for (let token = scanner.scan(); token !== ts.SyntaxKind.EndOfFileToken; token = scanner.scan()) {
		const value = token === ts.SyntaxKind.StringLiteral || token === ts.SyntaxKind.NumericLiteral
			? scanner.getTokenValue()
			: scanner.getTokenText();
		tokens.push([token, value]);
	}
	return JSON.stringify(tokens);
}

function expressionFingerprint(source: string): string {
	const file = ts.createSourceFile('pattern.ts', 'const pattern = ' + source + ';', ts.ScriptTarget.Latest, true);
	const statement = file.statements[0] as ts.VariableStatement;
	return fingerprint(statement.declarationList.declarations[0].initializer!, file);
}

const aggregationPatterns = aggregators.map(([kind, source]) => [kind, expressionFingerprint(source)] as const);
const expectedPrelude = ts.createSourceFile('prelude.ts', policyPrelude, ts.ScriptTarget.Latest, true);

function one<T>(items: readonly T[], description: string): T {
	if (items.length !== 1) unsupported('expected exactly one ' + description);
	return items[0];
}

function propertyName(node: ts.PropertyName | undefined): string {
	if (!node || (!ts.isIdentifier(node) && !ts.isStringLiteral(node))) {
		unsupported('computed or invalid policy property');
	}
	if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(node.text)) unsupported('unsupported policy name');
	return node.text;
}

function uniqueNames(names: readonly string[], description: string): Set<string> {
	const result = new Set(names);
	if (result.size !== names.length) unsupported('duplicate ' + description);
	return result;
}

function requireSameNames(expected: Set<string>, actual: Set<string>, description: string): void {
	const missing = [...expected].filter(name => !actual.has(name));
	const extra = [...actual].filter(name => !expected.has(name));
	if (missing.length || extra.length) {
		unsupported(description + ' keys differ (missing: ' + missing.join(', ') + '; extra: ' + extra.join(', ') + ')');
	}
}

function allowedAggregations(type: ts.TypeNode): readonly Aggregation[] {
	if (type.kind === ts.SyntaxKind.BooleanKeyword) return ['Any'];
	if (type.kind === ts.SyntaxKind.NumberKeyword) return ['Max', 'MaxFileSize'];
	if (ts.isArrayTypeNode(type) && type.elementType.kind === ts.SyntaxKind.StringKeyword) return ['FileTypes'];
	if (ts.isUnionTypeNode(type) && type.types.length === 3) {
		const values = type.types.map(member => ts.isLiteralTypeNode(member) && ts.isStringLiteral(member.literal) ? member.literal.text : undefined);
		if (values.slice().sort().join(',') === 'available,readonly,unavailable') return ['Chat'];
	}
	unsupported('unsupported RolePolicies value type');
}

export function extractPolicyAggregations(source: string): readonly PolicyAggregation[] {
	const file = parse(source);
	const type = one(file.statements.filter(ts.isTypeAliasDeclaration).filter(node => node.name.text === 'RolePolicies'), 'RolePolicies type');
	if (!ts.isTypeLiteralNode(type.type)) unsupported('RolePolicies must be a type literal');
	const declaredPolicies = type.type.members.map(member => {
		if (!ts.isPropertySignature(member) || member.questionToken || !member.type) unsupported('invalid RolePolicies member');
		return { name: propertyName(member.name), aggregations: allowedAggregations(member.type) };
	});
	const declaredNames = uniqueNames(declaredPolicies.map(policy => policy.name), 'RolePolicies key');
	const defaults = one(file.statements.filter(ts.isVariableStatement)
		.flatMap(statement => [...statement.declarationList.declarations])
		.filter(node => ts.isIdentifier(node.name) && node.name.text === 'DEFAULT_POLICIES'), 'DEFAULT_POLICIES declaration');
	if (!defaults.initializer || !ts.isObjectLiteralExpression(defaults.initializer)) unsupported('invalid DEFAULT_POLICIES object');
	const defaultNames = uniqueNames(defaults.initializer.properties.map(property => {
		if (!ts.isPropertyAssignment(property)) unsupported('invalid DEFAULT_POLICIES property');
		return propertyName(property.name);
	}), 'DEFAULT_POLICIES key');
	requireSameNames(declaredNames, defaultNames, 'DEFAULT_POLICIES');

	const roleClass = one(file.statements.filter(ts.isClassDeclaration).filter(node => node.name?.text === 'RoleService'), 'RoleService class');
	const method = one(roleClass.members.filter(ts.isMethodDeclaration)
		.filter(node => ts.isIdentifier(node.name) && node.name.text === 'getUserPolicies'), 'getUserPolicies method');
	if (!method.body || method.asteriskToken || !method.modifiers?.some(modifier => modifier.kind === ts.SyntaxKind.AsyncKeyword)
		|| method.parameters.length !== 1 || !ts.isIdentifier(method.parameters[0].name)
		|| method.parameters[0].name.text !== 'userId' || method.parameters[0].initializer
		|| method.parameters[0].questionToken || method.parameters[0].dotDotDotToken) unsupported('invalid getUserPolicies method');
	const statements = method.body.statements;
	const prelude = statements.slice(0, -1);
	if (prelude.length !== expectedPrelude.statements.length
		|| prelude.some((statement, index) => fingerprint(statement, file) !== fingerprint(expectedPrelude.statements[index], expectedPrelude))) {
		unsupported('getUserPolicies setup, calc, chat helper or file-size cap changed');
	}
	const returned = statements.at(-1);
	if (!returned || !ts.isReturnStatement(returned) || !returned.expression || !ts.isObjectLiteralExpression(returned.expression)) {
		unsupported('getUserPolicies must return a policy object');
	}
	const result = returned.expression.properties.map(property => {
		if (!ts.isPropertyAssignment(property)) unsupported('invalid returned policy property');
		const name = propertyName(property.name);
		const call = property.initializer;
		if (!ts.isCallExpression(call) || !ts.isIdentifier(call.expression) || call.expression.text !== 'calc'
			|| call.arguments.length !== 2 || call.typeArguments?.length) unsupported('invalid calc call for ' + name);
		const [argument, aggregate] = call.arguments;
		if (!ts.isStringLiteral(argument) || argument.text !== name) unsupported('calc key differs from property ' + name);
		const aggregation: Aggregation | undefined = ts.isIdentifier(aggregate) && aggregate.text === 'aggregateChatAvailability'
			? 'Chat'
			: aggregationPatterns.find(([, pattern]) => fingerprint(aggregate, file) === pattern)?.[0];
		if (!aggregation) unsupported('aggregation body for ' + name);
		const declaration = declaredPolicies.find(policy => policy.name === name);
		if (declaration && !declaration.aggregations.includes(aggregation)) unsupported('aggregation differs from RolePolicies type for ' + name);
		return { name, aggregation };
	});
	requireSameNames(declaredNames, uniqueNames(result.map(policy => policy.name), 'returned policy key'), 'getUserPolicies');
	return result;
}

export function generatePolicyAggregation(source: string): string {
	const policies = extractPolicyAggregations(source);
	return [
		'// Generated from packages/backend/src/core/RoleService.ts. Do not edit.',
		'// Regenerate: ' + generateCommand,
		'',
		'#[derive(Clone, Copy)]',
		'pub(super) enum Aggregation {',
		'    Any,',
		'    Max,',
		'    MaxFileSize,',
		'    Chat,',
		'    FileTypes,',
		'}',
		'',
		'pub(super) fn policy_aggregation(name: &str) -> Option<Aggregation> {',
		'    Some(match name {',
		...policies.map(policy => '        ' + JSON.stringify(policy.name) + ' => Aggregation::' + policy.aggregation + ','),
		'        _ => return None,',
		'    })',
		'}',
		'',
	].join('\n');
}

export function checkPolicyAggregation(source: string, generated: string): void {
	if (generated !== generatePolicyAggregation(source)) {
		throw new Error('Generated Rust policy aggregation is stale; run ' + generateCommand);
	}
}

if (import.meta.main) {
	const args = process.argv.slice(2);
	if (args.length > 1 || (args.length === 1 && args[0] !== '--check')) {
		throw new Error('Usage: ' + generateCommand + ' [--check]');
	}
	const source = readFileSync(roleSourcePath, 'utf8');
	if (args[0] === '--check') {
		checkPolicyAggregation(source, readFileSync(descriptorPath, 'utf8'));
		console.log('Rust policy aggregation: current');
	} else {
		writeFileSync(descriptorPath, generatePolicyAggregation(source));
		console.log('Rust policy aggregation: generated');
	}
}
