/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { createRequire } from 'node:module';
import { writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
const require = createRequire(fileURLToPath(new URL('../../../backend/package.json', import.meta.url)));
const Redis = require('ioredis');
const Limiter = require('ratelimiter');
const microtime = require('ratelimiter/microtime');
const address = process.env.MISSKEY_RUST_REDIS_TEST_URL;
if (!address) throw new Error('Dedicated Redis test URL is required');
const redis = new Redis(address, { lazyConnect: true });
await redis.connect();
let now = 1700000000000000;
microtime.now = () => now;
const scenarios = [
 {name:'integer',duration:1000,max:2,offsets:[0,20,40,1000041]},
 {name:'fractional-max',duration:1000,max:2.5,offsets:[0,20,40,60]},
 {name:'fractional-duration',duration:0.5,max:1,offsets:[0,200,600]},
 {name:'zero-defaults',duration:0,max:0,offsets:[0,20]},
 {name:'negative-duration',duration:-1,max:2,offsets:[0,20]},
 {name:'small-fractional-max',duration:1000,max:0.5,offsets:[0,20,40]},
 {name:'duplicate-microtime',duration:1000,max:1,offsets:[0,0,0]},
 {name:'infinite-max',duration:1000,max:'Infinity',offsets:[0,20]},
 {name:'wrong-type',duration:1000,max:2,wrongType:true,offsets:[0,20]},
];
try {
 for (const scenario of scenarios) {
  const id = `rust-native-reference:${process.pid}:${scenario.name}`;
  const key = `limit:${id}`;
  scenario.steps = [];
  await redis.del(key);
  if (scenario.wrongType) await redis.set(key, 'wrong-type');
  try {
   for (const offset of scenario.offsets) {
    now = 1700000000000000 + offset;
    const info = await new Promise((resolve, reject) => new Limiter({ id, db:redis, duration:scenario.duration, max:Number(scenario.max) }).get((error, info) => error ? reject(error) : resolve(info)));
    const members = await redis.zrange(key,0,-1,'WITHSCORES').catch(() => ['WRONGTYPE']);
    const ttl = await redis.pttl(key);
    scenario.steps.push({now,info,members,ttlKind:ttl===-1?'persistent':ttl===-2?'missing':'expiring'});
   }
  } finally { await redis.del(key); }
 }
 writeFileSync(new URL('./fixtures/redis-reference.json', import.meta.url), JSON.stringify({module:'ratelimiter@3.4.1',scenarios},null,2)+'\n');
 console.log(JSON.stringify({scenarios:scenarios.length,requests:scenarios.reduce((n,s)=>n+s.steps.length,0)}));
} finally { redis.disconnect(); }
