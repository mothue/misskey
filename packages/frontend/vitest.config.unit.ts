import { defineConfig, mergeConfig } from 'vitest/config';
import { getConfig } from './vite.config.js';

export default mergeConfig(getConfig(), defineConfig({
	test: {
		include: ['./test/unit/**/*.test.ts'],
		environment: 'happy-dom',
		environmentOptions: {
			happyDOM: {
				settings: {
					// Unit tests inspect DOM output; embedded pages and assets
					// are exercised by browser e2e, not real network requests here.
					disableIframePageLoading: true,
					disableCSSFileLoading: true,
					disableJavaScriptFileLoading: true,
				},
			},
		},
		setupFiles: ['./test/setup.unit.ts'],
		deps: {
			optimizer: {
				web: {
					include: [
						// XXX: misskey-dev/browser-image-resizer has no "type": "module"
						'browser-image-resizer',
					],
				},
			},
		},
		includeSource: ['src/**/*.ts'],
	},
}));

