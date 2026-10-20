import { invoke } from '@tauri-apps/api/core';
export interface SteamPlan { steam_id: string; name: string; folder: string; executable: string }
export interface SteamMetadata {
    steam_id: string;
    name: string;
    folder: string;
    source: string;
    launcher_urls: string[];
    launches: { description: string; plan: SteamPlan }[];
}
const cache = new Map<string, Promise<SteamMetadata>>();
export function lookupSteam(id: string, refresh = false): Promise<SteamMetadata> {
    if (refresh) cache.delete(id);
    let request = cache.get(id);
    if (!request) {
        request = invoke<SteamMetadata>('fetch_steam_metadata', { steamId: id });
        cache.set(id, request);
        const active = request;
        request.catch(() => { if (cache.get(id) === active) cache.delete(id); });
    }
    return request;
}
