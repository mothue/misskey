/*
 * SPDX-FileCopyrightText: misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

// Volar normally patches fs.readFileSync while requiring tsc. Bun's module
// loader bypasses that hook, so compile Volar's transformed compiler explicitly.
const fs = require('node:fs');
const path = require('node:path');
const { createRequire, Module } = require('node:module');

const workspaceRequire = createRequire(path.join(process.cwd(), 'package.json'));
const vueRequire = createRequire(workspaceRequire.resolve('vue-tsc'));
const volarPath = vueRequire.resolve('@volar/typescript/lib/quickstart/runTsc');
const volar = vueRequire(volarPath);
const core = vueRequire('@vue/language-core');
const ts = vueRequire('typescript');
const cli = ts.parseCommandLine(process.argv.slice(2));
const configPath = cli.options.project
	? path.resolve(cli.options.project)
	: ts.findConfigFile(process.cwd(), ts.sys.fileExists);
if (!configPath) throw new Error('No tsconfig.json found');
const { vueOptions } = core.createParsedCommandLine(ts, ts.sys, configPath);

volar.getLanguagePlugins = (typescript, options) => ({
	languagePlugins: [core.createVueLanguagePlugin(typescript, options.options, vueOptions, id => id)],
});

const compilerPath = vueRequire.resolve('typescript/lib/_tsc.js');
const source = volar.transformTscContent(
	fs.readFileSync(compilerPath, 'utf8'),
	vueRequire.resolve('@volar/typescript/lib/node/proxyCreateProgram'),
	core.getAllExtensions(vueOptions),
	[],
	volarPath,
);
const compiler = new Module(compilerPath);
compiler.filename = compilerPath;
compiler.paths = Module._nodeModulePaths(path.dirname(compilerPath));
compiler._compile(source, compilerPath);
