/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
// Run with Bun from any cwd. Extracts the actual baseline methods, not a second
// hand-written implementation, and records deterministic differential fixtures.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../../../', import.meta.url));
const baseline = process.argv[2] ?? '48783a1dedb2a97f8bb1323324c0b74e64cd8d0b';
const source = (path: string) => execFileSync('git', ['show', `${baseline}:${path}`], { cwd: root, encoding: 'utf8' });
const roleSource = source('packages/backend/src/core/RoleService.ts');
const extract = (text: string, start: string, end: string) => text.slice(text.indexOf(start), text.indexOf(end, text.indexOf(start)));
const conditions = extract(roleSource, '\tprivate evalCond(', '\n\t@bindThis\n\tpublic async getRoles()');
const policies = extract(roleSource, '\tpublic async getUserPolicies(', '\n\t@bindThis\n\tpublic async isModerator(');
const defaults = extract(roleSource, 'export const DEFAULT_POLICIES:', '\n@Injectable()').replace('export const ', 'const ');
const parsers = [
 ['aid', 'parseAid'], ['aidx', 'parseAidx'], ['meid', 'parseMeid'],
 ['meidg', 'parseMeidg'], ['object-id', 'parseObjectId'], ['ulid', 'parseUlid'],
].map(([file, name]) => {
 const text = source(`packages/backend/src/misc/id/${file}.ts`);
 const method = extract(text, `export function ${name}(`, `\nexport function ${name}Full(`).replace('export function ', 'function ');
 const helper = file === 'ulid' ? extract(text, 'function parseBase32(', `\nexport function ${name}(`) : '';
 return `const ${name}Ref = (() => { const TIME2000 = 946684800000; const TIME_LENGTH = 8; const CHARS = '0123456789ABCDEFGHJKMNPQRSTVWXYZ'; ${helper} ${method}; return ${name}; })();`;
}).join('\n');
const program = new Bun.Transpiler({ loader: 'ts' }).transformSync(`${defaults}\n${parsers}\nclass Reference { ${conditions}\n${policies}\n }\nreturn {Reference, DEFAULT_POLICIES, parsers:{aid:parseAidRef,aidx:parseAidxRef,meid:parseMeidRef,meidg:parseMeidgRef,objectid:parseObjectIdRef,ulid:parseUlidRef}};`);
const { Reference, DEFAULT_POLICIES, parsers: idParsers } = new Function(program)();
const reference = Object.create(Reference.prototype);
reference.userEntityService = { isLocalUser:(user:any)=> user.host == null, isRemoteUser:(user:any)=>user.host != null };
const cases: unknown[] = [];
const now = 1780000000000;
const originalNow = Date.now;
Date.now = () => now;
const ids = {
 aid:'0000000100',aidx:'0000000100000000',meid:'800000000001000000000000',
 meidg:'g00000000001000000000000',objectid:'000000010000000000000000',ulid:'00000000010000000000000000',
};
for (const [method, good] of Object.entries(ids)) {
 for (const id of [good, '', '!', '1z!', '0x1234', '+1234567', ' 1234567', '-1234567', '🦀12345678']) {
  const date = idParsers[method](id).date.getTime();
  cases.push({ kind:'date',method,id,expected:Number.isNaN(date)?null:date });
 }
}
let seed = 0xdecafbad;
const rand = (n:number) => { seed = (Math.imul(seed,1664525)+1013904223)>>>0; return seed%n; };
const flags = ['isSuspended','isLocked','isBot','isCat','isExplorable'];
for (let i = 0; i < 80; i++) {
 const method = Object.keys(ids)[i%6];
 reference.idService = { parse:(id:string) => idParsers[method](id) };
 const user:any = { id:ids[method],host:rand(2)?null:'example.test',followersCount:rand(7),followingCount:rand(7),notesCount:rand(7) };
 for (const flag of flags) user[flag] = !!rand(2);
 const roles = [{id:'a'},{id:'b'}];
 const atoms = [
  {type:'isLocal'},{type:'isRemote'},...flags.map(type=>({type})),
  {type:'roleAssignedTo',roleId:rand(2)?'a':'missing'},
  ...['followers','following','notes'].flatMap(name=>['LessThanOrEq','MoreThanOrEq'].map(suffix=>({type:name+suffix,value:rand(7)}))),
  {type:'createdLessThan',sec:rand(2)?1:999999999999},{type:'createdMoreThan',sec:rand(2)?1:999999999999},
  {type:'unknown'},
 ];
 const atom = atoms[i%atoms.length];
 const formula = i%3===0 ? {type:'and',values:[atom,{type:'not',value:{type:'isBot'}}]} : i%3===1 ? {type:'or',values:[atom,{type:'roleAssignedTo',roleId:'missing'}]} : atom;
 cases.push({kind:'condition',method,now,user,roles,formula,expected:reference.evalCond(user,roles,formula)});
}
for (let i=0;i<60;i++) {
 const meta = { policies: i%2 ? {rateLimitFactor:0.5,canInvite:true} : {} };
 const roles:any[] = [];
 for (let j=0;j<i%5;j++) {
  const policies:any = {};
  for (const [key,base] of Object.entries(DEFAULT_POLICIES)) {
   if (rand(3)===0) continue;
   let value:any = typeof base==='boolean' ? !!rand(2) : typeof base==='number' ? [0,0.5,1,2,20,200][rand(6)] : key==='chatAvailability' ? ['available','readonly','unavailable'][rand(3)] : [' image/png ','text/plain','image/png','\ufeff',''];
   policies[key] = {priority:rand(3),useDefault:!!rand(2),value};
  }
  roles.push({id:String(j),policies});
 }
 reference.meta = meta;
 reference.config = {maxFileSize:(17*1024*1024)+123};
 reference.getUserRoles = async () => roles;
 const expected = await reference.getUserPolicies('user');
 cases.push({kind:'policies',base:{...DEFAULT_POLICIES,...meta.policies},roles,maxFileSize:reference.config.maxFileSize,expected});
}
Date.now = originalNow;
const fixture = { baseline, sourceSha256:createHash('sha256').update(roleSource).digest('hex'), cases };
writeFileSync(new URL('./fixtures/role-reference.json',import.meta.url),JSON.stringify(fixture,null,2)+'\n');
console.log(JSON.stringify({baseline,cases:cases.length,sourceSha256:fixture.sourceSha256}));
