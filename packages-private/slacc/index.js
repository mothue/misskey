import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const binding = require('./built/slacc.node');

export const { AhoCorasick, RsaKeyPair, ZipReader, init } = binding;
export default binding;
