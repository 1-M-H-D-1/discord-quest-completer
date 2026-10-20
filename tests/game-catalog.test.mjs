import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
const source = readFileSync(new URL('../src/services/game-catalog.ts', import.meta.url), 'utf8');
const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020 } }).outputText;
const { mergeCatalogs, steamIds, hasWindowsExecutable } = await import(`data:text/javascript;base64,${Buffer.from(code).toString('base64')}`);
const game = (executables = [], extra = {}) => ({ id: '111', name: 'Example', executables, ...extra });
const exe = { name: 'game.exe', os: 'win32', is_launcher: false };
test('an executable from either live catalog prevents Steam fallback', () => {
    for (const sources of [[[game([exe])], [game()]], [[game()], [game([exe])]]]) {
        const [merged] = mergeCatalogs(...sources, []);
        assert.equal(hasWindowsExecutable(merged), true);
        assert.equal(merged.steam_catalog_checked, true);
    }
});
test('merges IDs and Steam SKUs; deduplicates executables', () => {
    const result = mergeCatalogs([game([exe], { third_party_skus: [{ distributor: 'steam', id: '123' }] })],
        [game([exe]), game([], { id: '222' })], []);
    assert.equal(result.length, 2);
    assert.equal(result[0].executables.length, 1);
    assert.deepEqual(steamIds(result[0]), ['123']);
});
test('missing or malformed source does not qualify as confirmed absence', () => {
    for (const bad of [null, [], {}, [{ id: '1' }]]) {
        const [merged] = mergeCatalogs([game()], bad, []);
        assert.equal(merged.steam_catalog_checked, false);
    }
});
test('bundled data cannot override fresh empty executable lists', () => {
    assert.equal(hasWindowsExecutable(mergeCatalogs([game()], [game()], [game([exe])])[0]), false);
    const [offline] = mergeCatalogs(null, null, [game([exe])]);
    assert.equal(hasWindowsExecutable(offline), true);
    assert.equal(offline.steam_catalog_checked, false);
});
test('non-Windows paths and invalid Steam IDs are not candidates', () => {
    const candidate = game([{ ...exe, os: 'linux' }], { third_party_skus: [
        { distributor: 'steam', id: '../12' }, { distributor: 'epic', id: '123' },
        { distributor: 'steam', id: '456' }, { distributor: 'steam', id: '456' },
    ] });
    assert.equal(hasWindowsExecutable(candidate), false);
    assert.deepEqual(steamIds(candidate), ['456']);
});
test('null store identifiers in real Discord catalogs do not invalidate the source', () => {
    const entry = game([], { third_party_skus: [{ distributor: 'battlenet', id: null }, { distributor: 'steam', id: '123' }] });
    const [merged] = mergeCatalogs([entry], [entry], []);
    assert.equal(merged.steam_catalog_checked, true);
    assert.deepEqual(steamIds(merged), ['123']);
});
