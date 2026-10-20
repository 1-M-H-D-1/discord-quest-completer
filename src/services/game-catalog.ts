import type { Game } from '../types/types';

export function validCatalog(data: unknown): data is Game[] {
    return Array.isArray(data) && data.length > 0 && data.every(game =>
        typeof game?.id === 'string' && typeof game.name === 'string' &&
        Array.isArray(game.executables) && game.executables.every((exe: any) =>
            typeof exe?.name === 'string' && typeof exe.os === 'string') &&
        (game.third_party_skus == null || (Array.isArray(game.third_party_skus) &&
            game.third_party_skus.every((sku: any) => sku && (sku.id === null || typeof sku.id === 'string') && typeof sku.distributor === 'string'))));
}

export function hasWindowsExecutable(game: Game): boolean {
    return game.executables.some(exe => exe.os === 'win32' && exe.name.trim().length > 0);
}

export function steamIds(game: Game): string[] {
    return [...new Set((game.third_party_skus || [])
        .flatMap(sku => sku.distributor === 'steam' && typeof sku.id === 'string' && /^[1-9]\d*$/.test(sku.id) ? [sku.id] : []))];
}

// A live executable from either source wins over Steam. The bundled catalog is
// used only when neither network source is usable, never to override fresh data.
export function mergeCatalogs(discord: unknown, mirror: unknown, bundled: Game[]): Game[] {
    const directOK = validCatalog(discord);
    const mirrorOK = validCatalog(mirror);
    const result = new Map<string, Game>();
    const sources = directOK || mirrorOK ? [mirrorOK ? mirror : [], directOK ? discord : []] : [bundled];
    for (const source of sources) for (const game of source) {
        const previous = result.get(game.id);
        const executables = [...(previous?.executables || []), ...game.executables];
        const skus = [...(previous?.third_party_skus || []), ...(game.third_party_skus || [])];
        result.set(game.id, {
            ...previous, ...game,
            executables: executables.filter((exe, i) => executables.findIndex(other =>
                other.os === exe.os && other.name.toLowerCase() === exe.name.toLowerCase()) === i),
            third_party_skus: skus.filter((sku, i) => skus.findIndex(other =>
                other.distributor === sku.distributor && other.id === sku.id) === i),
            steam_catalog_checked: directOK && mirrorOK,
        });
    }
    return [...result.values()];
}
