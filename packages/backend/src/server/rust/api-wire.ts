/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

/** JSON-compatible framing for JavaScript values, including UTF-16 and undefined. */
export type ApiWireValue =
	| { t: 'undefined' | 'null' }
	| { t: 'boolean'; v: boolean }
	| { t: 'number'; v: string }
	| { t: 'string'; v: number[] }
	| { t: 'array'; v: ApiWireValue[] }
	| { t: 'object'; v: [number[], ApiWireValue][] };

export function apiEncodeString(value: string): number[] {
	return Array.from({ length: value.length }, (_, index) => value.charCodeAt(index));
}

/** Serialize internal JSON-only frames without the added tagged nesting consuming the call stack. */
export function apiStringifyFrame(value: unknown): string {
	type Task = { value: unknown } | { text: string } | { leave: object };
	const tasks: Task[] = [{ value }];
	const parts: string[] = [];
	const active = new Set<object>();
	while (tasks.length > 0) {
		const task = tasks.pop()!;
		if ('text' in task) { parts.push(task.text); continue; }
		if ('leave' in task) { active.delete(task.leave); continue; }
		const current = task.value;
		if (current === null || typeof current === 'string' || typeof current === 'number' || typeof current === 'boolean') {
			parts.push(JSON.stringify(current));
			continue;
		}
		if (typeof current !== 'object') throw new Error('Invalid API frame value');
		if (active.has(current)) throw new Error('Cyclic API frame');
		active.add(current);
		tasks.push({ leave: current });
		if (Array.isArray(current)) {
			parts.push('[');
			tasks.push({ text: ']' });
			for (let index = current.length - 1; index >= 0; index--) {
				tasks.push({ value: current[index] === undefined ? null : current[index] });
				if (index > 0) tasks.push({ text: ',' });
			}
		} else {
			parts.push('{');
			tasks.push({ text: '}' });
			const record = current as Record<string, unknown>;
			const keys = Object.keys(record).filter(key => record[key] !== undefined);
			for (let index = keys.length - 1; index >= 0; index--) {
				tasks.push({ value: record[keys[index]] });
				tasks.push({ text: `${JSON.stringify(keys[index])}:` });
				if (index > 0) tasks.push({ text: ',' });
			}
		}
	}
	return parts.join('');
}

export function apiDecodeString(value: unknown): string {
	if (!Array.isArray(value)) throw new Error('Invalid API UTF-16 string');
	let result = '';
	for (const unit of value) {
		if (!Number.isInteger(unit) || unit < 0 || unit > 0xffff) throw new Error('Invalid API UTF-16 unit');
		result += String.fromCharCode(unit);
	}
	return result;
}

/** Encode iteratively: nested public JSON must not exhaust the JS call stack. */
export function apiEncodeValue(value: unknown): ApiWireValue {
	let result: ApiWireValue = { t: 'undefined' };
	const active = new Set<object>();
	type Task = { value: unknown; put: (wire: ApiWireValue) => void } | { leave: object };
	const tasks: Task[] = [{ value, put: wire => { result = wire; } }];
	while (tasks.length > 0) {
		const task = tasks.pop()!;
		if ('leave' in task) { active.delete(task.leave); continue; }
		const current = task.value;
		if (current === undefined) { task.put({ t: 'undefined' }); continue; }
		if (current === null) { task.put({ t: 'null' }); continue; }
		if (typeof current === 'boolean') { task.put({ t: 'boolean', v: current }); continue; }
		if (typeof current === 'number') { task.put({ t: 'number', v: Object.is(current, -0) ? '-0' : String(current) }); continue; }
		if (typeof current === 'string') { task.put({ t: 'string', v: apiEncodeString(current) }); continue; }
		if (typeof current !== 'object') throw new Error('Unsupported API value');
		if (active.has(current)) throw new Error('Cyclic API value');
		active.add(current);
		tasks.push({ leave: current });
		if (Array.isArray(current)) {
			const items: ApiWireValue[] = new Array(current.length);
			task.put({ t: 'array', v: items });
			for (let index = current.length - 1; index >= 0; index--) {
				tasks.push({ value: current[index], put: wire => { items[index] = wire; } });
			}
		} else {
			const keys = Object.keys(current);
			const entries: [number[], ApiWireValue][] = keys.map(key => [apiEncodeString(key), { t: 'undefined' }]);
			task.put({ t: 'object', v: entries });
			for (let index = keys.length - 1; index >= 0; index--) {
				tasks.push({ value: (current as Record<string, unknown>)[keys[index]], put: wire => { entries[index][1] = wire; } });
			}
		}
	}
	return result;
}

function record(value: unknown): value is Record<string, unknown> {
	return value !== null && typeof value === 'object' && !Array.isArray(value);
}

/** Decode only the declared wire shape; no prototype setters or type assertions as validation. */
export function apiDecodeValue(value: unknown): unknown {
	let result: unknown;
	const tasks: { value: unknown; put: (decoded: unknown) => void }[] = [{ value, put: decoded => { result = decoded; } }];
	while (tasks.length > 0) {
		const task = tasks.pop()!;
		const wire = task.value;
		if (!record(wire) || typeof wire.t !== 'string' || Object.keys(wire).some(key => key !== 't' && key !== 'v')) throw new Error('Invalid API value envelope');
		if (wire.t === 'undefined' || wire.t === 'null') {
			if ('v' in wire) throw new Error('Invalid empty API value');
			task.put(wire.t === 'undefined' ? undefined : null);
		} else if (wire.t === 'boolean') {
			if (typeof wire.v !== 'boolean') throw new Error('Invalid API boolean');
			task.put(wire.v);
		} else if (wire.t === 'number') {
			if (typeof wire.v !== 'string' || !/^(?:NaN|[+-]?Infinity|-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?)$/.test(wire.v)) throw new Error('Invalid API number');
			task.put(Number(wire.v));
		} else if (wire.t === 'string') {
			task.put(apiDecodeString(wire.v));
		} else if (wire.t === 'array') {
			if (!Array.isArray(wire.v)) throw new Error('Invalid API array');
			const decoded: unknown[] = new Array(wire.v.length);
			task.put(decoded);
			for (let index = wire.v.length - 1; index >= 0; index--) tasks.push({ value: wire.v[index], put: item => { decoded[index] = item; } });
		} else if (wire.t === 'object') {
			if (!Array.isArray(wire.v)) throw new Error('Invalid API object');
			const decoded: Record<string, unknown> = {};
			const seen = new Set<string>();
			const entries = wire.v.map((entry: unknown) => {
				if (!Array.isArray(entry) || entry.length !== 2) throw new Error('Invalid API object entry');
				const key = apiDecodeString(entry[0]);
				if (seen.has(key)) throw new Error('Duplicate API object key');
				seen.add(key);
				// Reserve own properties in wire order before deferred nested values.
				Object.defineProperty(decoded, key, { value: undefined, enumerable: true, configurable: true, writable: true });
				return { key, value: entry[1] as unknown };
			});
			task.put(decoded);
			for (let index = entries.length - 1; index >= 0; index--) tasks.push({ value: entries[index].value, put: item => { decoded[entries[index].key] = item; } });
		} else {
			throw new Error('Unknown API value tag');
		}
	}
	return result;
}
