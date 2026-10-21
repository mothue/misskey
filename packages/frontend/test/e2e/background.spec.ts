/*
	* SPDX-FileCopyrightText: syuilo and misskey-project
	* SPDX-License-Identifier: AGPL-3.0-only
	*/
import { deflateSync } from 'node:zlib';
import type { Page, APIRequestContext } from 'playwright';
import { test, expect } from './fixtures.js';
import { api, closeUserSetupDialog } from './shared.js';
import { BASE_URL, registerUser, resetState } from './utils.js';

test.use({ actionTimeout: 10000 });

// Small generated PNGs keep this test independent of private files and CDNs.
function png(red: number, blue: number) {
	function chunk(name: string, data: Buffer) {
		const body = Buffer.concat([Buffer.from(name), data]);
		let crc = 0xffffffff;
		for (const value of body) {
			crc ^= value;
			for (let bit = 0; bit < 8; bit++)
				crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
		}
		const header = Buffer.alloc(4);
		header.writeUInt32BE(data.length);
		const tail = Buffer.alloc(4);
		tail.writeUInt32BE((crc ^ 0xffffffff) >>> 0);
		return Buffer.concat([header, body, tail]);
	}
	const header = Buffer.alloc(13);
	header.writeUInt32BE(1, 0);
	header.writeUInt32BE(1, 4);
	header[8] = 8;
	header[9] = 6;
	return Buffer.concat([
		Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
		chunk('IHDR', header),
		chunk('IDAT', deflateSync(Buffer.from([0, red, 0, blue, 255]))),
		chunk('IEND', Buffer.alloc(0)),
	]);
}
async function selectSingle(page: Page, name: string) {
	await page.getByTestId('background-select').click();
	await page.getByText('From Drive', { exact: true }).click();
	await page.getByTitle(name).first().click();
	await page.getByRole('button', { name: /Done/ }).last().click();
	await expect(page.getByTestId('background-current')).toBeVisible();
}
async function seed(page: Page, request: APIRequestContext) {
	await resetState();
	await registerUser('admin', 'testpass', true);
	const user = await registerUser('backgrounduser', 'testpass');
	const folder = await api(BASE_URL, 'drive/folders/create', {
		i: user.token,
		name: 'Background slides',
	});
	const names = [
		'red-background.png',
		'blue-background.png',
		'purple-background.png',
	];
	for (const [index, name] of names.entries()) {
		const uploaded = await request.post(`${BASE_URL}/api/drive/files/create`, {
			multipart: {
				i: user.token,
				file: {
					name,
					mimeType: 'image/png',
					buffer: png(
						index === 0 ? 255 : index === 1 ? 0 : 128,
						index === 0 ? 0 : index === 1 ? 255 : 128,
					),
				},
			},
		});
		expect(uploaded.ok()).toBe(true);
		const file = await uploaded.json();
		// Keep the first image at the drive root for the single-image picker.
		if (index)
			await api(BASE_URL, 'drive/files/update', {
				i: user.token,
				fileId: file.id,
				folderId: folder.id,
			});
	}
	const me = await api(BASE_URL, 'i', { i: user.token });
	await page.addInitScript(
		(account) => {
			localStorage.setItem('account', JSON.stringify(account));
			localStorage.setItem('lang', 'en-US');
		},
		{ ...me, token: user.token },
	);
	await page.goto('/settings/background');
	await closeUserSetupDialog(page);
	return { user, folder, name: names[0] };
}

test('background settings reuse saved image bytes after reload and keep windows opaque', async ({
	page,
	request,
}, testInfo) => {
	const { name } = await seed(page, request);
	let imageRequests = 0;
	await page.route('**/files/**', async (route) => {
		imageRequests++;
		await route.fulfill({
			body: png(255, 0),
			contentType: 'image/png',
			headers: {
				'Cache-Control': 'no-store',
				'Access-Control-Allow-Origin': '*',
			},
		});
	});
	await selectSingle(page, name);
	const firstRequests = imageRequests;
	expect(firstRequests).toBeGreaterThan(0);
	await page.reload();
	await expect(page.getByTestId('background-current')).toBeVisible();
	expect(imageRequests).toBe(firstRequests);
	// Open the same settings page in a real floating window via its context menu.
	await page.getByRole('link', { name: /Timeline/ }).first().click({ button: 'right' });
	await page.getByText('Open in window', { exact: true }).click();

	await page.getByRole('link', { name: /Timeline/ }).first().click({ button: 'right' });
	await page.getByText('Open in window', { exact: true }).click();
	await expect(page.locator('._opaqueWindow')).toHaveCount(2);
	const windowRoot = page.locator('._opaqueWindow').last();
	await expect(windowRoot).toBeVisible();
	const alpha = await windowRoot.evaluate((el) => {
		const canvas = window.document.createElement('canvas');
		canvas.width = canvas.height = 1;
		const context = canvas.getContext('2d')!;
		context.fillStyle = getComputedStyle(el).backgroundColor;
		context.fillRect(0, 0, 1, 1);
		return context.getImageData(0, 0, 1, 1).data[3];
	});
	expect(alpha).toBe(255);
	expect(
		parseFloat(
			await windowRoot.evaluate(
				(el) => getComputedStyle(el).borderTopLeftRadius,
			),
		),
	).toBeGreaterThan(0);

	await windowRoot.locator('button:has(.ti-rectangle)').click();
	await expect(windowRoot).toHaveCSS('border-top-left-radius', '0px');
	await windowRoot.locator('button:has(.ti-picture-in-picture)').click();
	expect(parseFloat(await windowRoot.evaluate(el => getComputedStyle(el).borderTopLeftRadius))).toBeGreaterThan(0);
	// Synthetic theme colors exercise live variable inheritance, including alpha.
	const themed = await page.evaluate(() => {
		const root = window.document.documentElement;
		root.style.setProperty('--MI_THEME-bg', 'rgba(10, 20, 30, 0.5)');
		root.style.setProperty('--MI_THEME-panel', 'rgb(40, 50, 60)');
		root.style.setProperty('--MI_THEME-windowHeader', 'rgba(60, 70, 80, 0.4)');
		const windowEl = window.document.querySelector('._opaqueWindow')!;
		const content = window.document.querySelector(
			'[data-testid="background-select"]',
		)!;
		const sample = (color: string) => {
			const canvas = window.document.createElement('canvas');
			canvas.width = canvas.height = 1;
			const ctx = canvas.getContext('2d')!;
			ctx.fillStyle = color;
			ctx.fillRect(0, 0, 1, 1);
			return Array.from(ctx.getImageData(0, 0, 1, 1).data);
		};
		return {
			underlay: sample(getComputedStyle(windowEl).backgroundColor),
			panel: sample(
				getComputedStyle(windowEl).getPropertyValue('--MI_THEME-panel'),
			),
			main: sample(
				getComputedStyle(content).getPropertyValue('--MI_THEME-panel'),
			),
		};
	});
	expect(themed.underlay).toEqual([10, 20, 30, 255]);
	expect(themed.panel).toEqual([40, 50, 60, 255]);
	expect(themed.main[3]).toBeLessThan(255);

	await expect(page.getByTestId('background-current')).toBeVisible();
	await page.screenshot({ path: testInfo.outputPath('windows.png') });
});

test('selects a folder, shows its image, reports an empty folder and returns to single mode', async ({
	page,
	request,
}) => {
	await page.clock.install();
	await page.emulateMedia({ reducedMotion: 'reduce' });
	const { folder } = await seed(page, request);
	await page.route('**/files/**', (route) =>
		route.fulfill({
			body: png(0, 255),
			contentType: 'image/png',
			headers: { 'Access-Control-Allow-Origin': '*' },
		}),
	);
	await page
		.getByTestId('background-mode')
		.locator('[tabindex="0"]')
		.first()
		.click();
	await page.getByText('Slideshow', { exact: true }).last().click();
	await page.getByTestId('background-folder').click();
	const folderItem = page.getByTitle(folder.name, { exact: true }).last();
	await folderItem.click();
	await expect(page.getByTitle('blue-background.png').first()).toBeVisible();
	await page.getByRole('button', { name: /Select a folder/ }).click();
	await page.getByRole('button', { name: /Done/ }).last().click();
	await expect(page.getByTestId('background-current')).toBeVisible();

	const initialBackground = await page
		.getByTestId('background-current')
		.evaluate((el) => (el as HTMLElement).style.backgroundImage);
	await page.clock.fastForward(60000);
	await expect(page.getByTestId('background-current')).not.toHaveCSS(
		'background-image',
		initialBackground,
	);
	await expect(page.getByTestId('background-previous')).toHaveCount(0);
	await page.route('**/api/drive/files', async (route) => {
		const body = route.request().postDataJSON();
		if (body.folderId === folder.id) await route.fulfill({ json: [] });
		else await route.continue();
	});
	await page.getByTestId('background-reload').click();
	await expect(page.getByTestId('background-empty')).toBeVisible();
	await page
		.getByTestId('background-mode')
		.locator('[tabindex="0"]')
		.first()
		.click();
	await page.getByText('Single image', { exact: true }).last().click();
	await expect(page.getByTestId('background-folder')).toHaveCount(0);
});

test.describe('mobile background windows', () => {
	test.use({
		viewport: { width: 390, height: 844 },
		isMobile: true,
		hasTouch: true,
		userAgent:
			'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 Mobile/15E148 Safari/604.1',
	});
	test('keeps the drawer opaque when selecting a background again', async ({
		page,
		request,
	}) => {
		const { name } = await seed(page, request);
		await page.route('**/files/**', (route) =>
			route.fulfill({
				body: png(255, 0),
				contentType: 'image/png',
				headers: { 'Access-Control-Allow-Origin': '*' },
			}),
		);
		await selectSingle(page, name);
		await page.getByTestId('background-select').click();
		await page.getByText('From Drive', { exact: true }).click();
		const drawer = page.locator('._opaqueWindow').last();
		await expect(drawer).toBeVisible();
		expect(
			await drawer.evaluate((el) => getComputedStyle(el).backgroundColor),
		).not.toBe('rgba(0, 0, 0, 0)');
	});
});
