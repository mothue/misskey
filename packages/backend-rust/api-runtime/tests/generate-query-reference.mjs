/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
// Run with Bun from any directory; this imports the installed Fastify parser.
import { createRequire } from 'node:module';
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
const require = createRequire(fileURLToPath(new URL('../../../backend/package.json', import.meta.url)));
const fastifyRequire = createRequire(require.resolve('fastify'));
const parserPath = fastifyRequire.resolve('fast-querystring');
const { parse } = fastifyRequire('fast-querystring');
const parserPackage = JSON.parse(readFileSync(new URL('../package.json', `file://${parserPath}`)));
const components = [
 '', '%', '%0', '%GG', '%0G', '%G0', '%41%ZZ', '%41%', '%41%4', '%41%FF',
 '%FF', '%80', '%BF', '%C0%AF', '%C1%BF', '%C2', '%C2%80', '%DF%BF',
 '%E0%80%AF', '%E0%A0%80', '%ED%A0%80', '%ED%BF%BF', '%EF%BF%BD',
 '%E2%82', '%E2%82%AC', '%F0%80%80%AF', '%F0%90%80%80', '%F4%8F%BF%BF',
 '%F4%90%80%80', '%F5%80%80%80', '%F8%80%80%80%80', '%FC%80%80%80%80%80',
 '%ED%A0%BD%ED%B8%80', '%F0%9F%90%88', '%E2x%82%AC', '%E2%82x%AC',
 '+', '%2B', 'a+b%2Bc', '%41+%ZZ', '%41%ZZ+%2B', 'é🐈', 'é%41🐈',
 '%25', '%252B', '%00', '%3D', '%26', '%2b%2B+',
 ...Array.from({ length: 256 }, (_, byte) => `%${byte.toString(16).padStart(2, '0')}`),
];
const inputs = [
 '', '&', '&&', '=', '==', '&&=&&', 'bare', 'a==b', 'a=1&a=2&a=3',
 'x=%41%ZZ&x=A%ZZ', '%41%ZZ=one&A%ZZ=two', 'a+b=one&a%20b=two',
 '__proto__=one&__proto__=two&constructor=x&constructor=y&toString=z',
 '2=b&10=c&1=a&x=first&x=second&01=leading',
];
for (const component of components) {
 inputs.push(`x=${component}`, `${component}=value`, `ok=%41&bad=${component}&last=%42`);
}
const cases = [...new Set(inputs)].map(input => ({ input, json: JSON.stringify(parse(input)) }));
writeFileSync(new URL('./fixtures/query-reference.json', import.meta.url), `${JSON.stringify({ parser: 'fast-querystring', version: parserPackage.version, cases }, null, 2)}\n`);
console.log(`Captured ${cases.length} fast-querystring ${parserPackage.version} cases`);
