<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import type { Game } from '@/types/types';
import { steamIds } from '@/services/game-catalog';
import { lookupSteam, type SteamMetadata, type SteamPlan } from '@/services/steam-metadata';

const props = defineProps<{ game: Game }>();
const ids = steamIds(props.game);
const steamId = ref(ids[0] || '');
const storageKey = `manual-steam-plan:${props.game.id}`;
const metadata = ref<SteamMetadata>();
const launchIndex = ref(0);
const manualPlan = ref<SteamPlan>();
try {
    const saved = JSON.parse(localStorage.getItem(storageKey) || 'null');
    if (saved && ['steam_id', 'name', 'folder', 'executable'].every(key => typeof saved[key] === 'string')) manualPlan.value = saved;
} catch { /* Ignore malformed saved entries; they can be entered again. */ }
const manualMode = ref(true);
const draft = reactive<SteamPlan>(manualPlan.value ? { ...manualPlan.value } : {
    steam_id: steamId.value, name: props.game.name, folder: '', executable: '',
});
const touched = ref(false);
const plan = computed(() => manualMode.value ? manualPlan.value : metadata.value?.launches[launchIndex.value]?.plan);
const library = ref(localStorage.getItem('steam-fallback-library') || '');
const installed = ref(false);
const running = ref(false);
const busy = ref(false);
const loading = ref(false);
const message = ref('');
const error = ref('');
const lookupError = ref('');
const fieldsLocked = computed(() => busy.value || running.value || installed.value);
let poll: ReturnType<typeof setInterval> | undefined;
let disposed = false;
let request = 0;
interface Status { installed: boolean; running: boolean; message: string }

async function discover(retry = false) {
    if (!props.game.steam_catalog_checked || !steamId.value || busy.value || running.value || installed.value) return;
    const current = ++request;
    loading.value = true;
    lookupError.value = '';
    try {
        const result = await lookupSteam(steamId.value, retry);
        if (disposed || current !== request) return;
        metadata.value = result;
        launchIndex.value = 0;
        if (!manualPlan.value && !touched.value) {
            Object.assign(draft, { steam_id: result.steam_id, name: result.name, folder: result.folder, executable: result.launches[0]?.plan.executable || '' });
            manualMode.value = result.launches.length === 0;
        }
    } catch (reason) {
        if (!disposed && current === request) lookupError.value = String(reason);
    } finally {
        if (!disposed && current === request) { loading.value = false; await refresh(); }
    }
}
async function refresh() {
    if (!library.value || !plan.value || busy.value) return;
    const selectedPlan = plan.value;
    const selectedLibrary = library.value;
    try {
        const status = await invoke<Status>('steam_fallback_action', { plan: selectedPlan, library: selectedLibrary, action: 'status' });
        if (disposed || selectedPlan !== plan.value || selectedLibrary !== library.value || busy.value) return;
        installed.value = status.installed;
        running.value = status.running;
    } catch (reason) {
        if (!disposed && selectedPlan === plan.value && selectedLibrary === library.value && !busy.value) {
            installed.value = false;
            error.value = String(reason);
        }
    }
}
async function saveManual() {
    if (fieldsLocked.value) return;
    busy.value = true;
    error.value = '';
    try {
        const validated = await invoke<SteamPlan>('validate_manual_steam_plan', { plan: { ...draft } });
        if (disposed) return;
        localStorage.setItem(storageKey, JSON.stringify(validated));
        manualPlan.value = validated;
        Object.assign(draft, validated);
        manualMode.value = true;
        touched.value = false;
        message.value = 'Manual details saved for this game.';
    } catch (reason) { error.value = String(reason); }
    finally { busy.value = false; if (!disposed) await refresh(); }
}
async function chooseLibrary() {
    error.value = '';
    try {
        const selected = await open({ directory: true, multiple: false, title: 'Select Steam library (contains steamapps)' });
        if (typeof selected !== 'string' || disposed) return;
        library.value = selected;
        installed.value = false;
        message.value = '';
        localStorage.setItem('steam-fallback-library', selected);
        await refresh();
    } catch (reason) { error.value = String(reason); }
}
async function act(action: string) {
    if (!plan.value) return;
    busy.value = true;
    error.value = '';
    message.value = '';
    try {
        const status = await invoke<Status>('steam_fallback_action', { plan: plan.value, library: library.value, action });
        installed.value = status.installed;
        running.value = status.running;
        message.value = status.message;
    } catch (reason) { error.value = String(reason); }
    finally { busy.value = false; if (!disposed) await refresh(); }
}
function changeMode() { installed.value = false; error.value = ''; refresh(); }
onMounted(() => { refresh(); discover(); poll = setInterval(refresh, 3000); });
onUnmounted(() => { disposed = true; ++request; if (poll) clearInterval(poll); });
</script>

<template>
    <section class="mt-5 border-t border-gray-300 dark:border-gray-600 pt-4 text-sm text-gray-600 dark:text-gray-300">
        <h3 class="font-medium text-gray-900 dark:text-white">Steam fallback · Manual details</h3>
        <p class="mt-2">No Windows executable was detected in the game catalog. Enter the details below, or use Steam launch metadata when available.</p>
        <p v-if="!game.steam_catalog_checked" class="mt-2">Automatic lookup needs both live catalogs. Manual entry is still available.</p>
        <p v-else-if="!ids.length" class="mt-2">No Steam app ID was supplied by Discord. Enter it manually below.</p>
        <template v-else>
            <label v-if="ids.length > 1" class="block mt-3">Steam application to look up
                <select v-model="steamId" @change="discover()" :disabled="fieldsLocked || loading" class="block w-full p-2 dark:bg-gray-700 rounded">
                    <option v-for="id in ids" :key="id" :value="id">{{ id }}</option>
                </select>
            </label>
            <button @click="discover(true)" :disabled="fieldsLocked || loading"
                class="mt-3 px-3 py-2 rounded bg-gray-200 dark:bg-gray-600 disabled:opacity-50">Refresh Steam metadata</button>
        </template>
        <p v-if="loading" role="status" class="mt-2">Reading Steam launch metadata…</p>
        <p v-if="lookupError" class="mt-2 text-amber-600 dark:text-amber-400">{{ lookupError }} You can enter the details manually.</p>
        <p v-if="metadata && !metadata.launches.length" class="mt-2">Steam supplies a launcher URL, not an EXE. The game name and folder have been filled in where available; enter the EXE path manually.</p>
        <label v-if="metadata?.launches.length" class="flex gap-2 items-center mt-3">
            <input type="checkbox" v-model="manualMode" @change="changeMode" :disabled="fieldsLocked">
            Use manual details
        </label>
        <form v-if="manualMode" @submit.prevent="saveManual" class="space-y-3 mt-3">
            <fieldset :disabled="fieldsLocked" class="space-y-3 disabled:opacity-60">
                <label class="block">Steam app ID
                    <input v-model="draft.steam_id" @input="touched = true" required inputmode="numeric" placeholder="Numeric Steam app ID"
                        class="block w-full p-2 mt-1 rounded border border-gray-300 dark:border-gray-600 dark:bg-gray-700">
                </label>
                <label class="block">Game name
                    <input v-model="draft.name" @input="touched = true" required maxlength="512"
                        class="block w-full p-2 mt-1 rounded border border-gray-300 dark:border-gray-600 dark:bg-gray-700">
                </label>
                <label class="block">Installation folder name
                    <input v-model="draft.folder" @input="touched = true" required placeholder="Folder inside steamapps/common"
                        class="block w-full p-2 mt-1 rounded border border-gray-300 dark:border-gray-600 dark:bg-gray-700">
                </label>
                <label class="block">EXE path inside that folder
                    <input v-model="draft.executable" @input="touched = true" required placeholder="game.exe or Binaries/Win64/game.exe"
                        class="block w-full p-2 mt-1 rounded border border-gray-300 dark:border-gray-600 dark:bg-gray-700">
                </label>
                <p class="text-xs">Use a relative EXE path, not a full drive path or launcher URL. Details are saved separately for each game.</p>
                <button type="submit" class="px-3 py-2 rounded bg-indigo-600 text-white disabled:opacity-50">Save manual details</button>
            </fieldset>
            <p v-if="installed" class="text-xs">Remove this fallback before changing its saved details.</p>
            <p v-if="touched && manualPlan" class="text-xs">Unsaved changes. The preview below still uses your saved details.</p>
        </form>
        <label v-else-if="metadata && metadata.launches.length > 1" class="block mt-3">Windows launch executable
            <select v-model="launchIndex" @change="changeMode" :disabled="fieldsLocked" class="block w-full p-2 dark:bg-gray-700 rounded">
                <option v-for="(launch, index) in metadata.launches" :key="index" :value="index">{{ launch.plan.executable }}</option>
            </select>
        </label>
        <template v-if="plan">
            <p class="mt-3 text-xs break-all">Runner: steamapps/common/{{ plan.folder }}/{{ plan.executable }}<br>Manifest: steamapps/appmanifest_{{ plan.steam_id }}.acf</p>
            <ol class="list-decimal list-inside mt-2 space-y-1">
                <li>Exit Steam from its tray menu.</li>
                <li>Select your Steam library and set up the fallback.</li>
                <li>Restart Steam and Discord, then launch the runner below.</li>
            </ol>
            <button @click="chooseLibrary" :disabled="busy || running" class="mt-3 px-3 py-2 rounded bg-gray-200 dark:bg-gray-600 disabled:opacity-50">Select Steam library</button>
            <p v-if="library" class="mt-2 text-xs break-all">{{ library }}</p>
            <div class="flex flex-wrap gap-2 mt-3">
                <button v-if="!installed" @click="act('setup')" :disabled="!library || busy || running || (manualMode && touched)" class="px-3 py-2 rounded bg-indigo-600 text-white disabled:opacity-50">Set up Steam fallback</button>
                <button v-if="installed && !running" @click="act('launch')" :disabled="busy" class="px-3 py-2 rounded bg-blue-600 text-white disabled:opacity-50">Launch Steam runner</button>
                <button v-if="running" @click="act('stop')" :disabled="busy" class="px-3 py-2 rounded bg-red-600 text-white disabled:opacity-50">Stop Steam runner</button>
                <button v-if="library" @click="act('remove')" :disabled="busy || running" class="px-3 py-2 rounded border border-gray-400 disabled:opacity-50">Remove Steam fallback</button>
            </div>
        </template>
        <p v-if="busy" class="mt-2" role="status">Working…</p>
        <p v-if="message" class="mt-2 text-green-700 dark:text-green-400" role="status">{{ message }}</p>
        <p v-if="error" class="mt-2 text-red-600 dark:text-red-400" role="alert">{{ error }}</p>
        <p class="mt-2 text-xs">Existing game files are never overwritten. This creates only a dummy runner; Discord recognition is unverified.</p>
    </section>
</template>
