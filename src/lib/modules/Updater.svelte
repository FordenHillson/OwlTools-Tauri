<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';

  type ManifestVersion = {
    version: string;
    notes?: string;
    msi: {
      url: string;
      sha256: string;
      size?: number;
    };
  };

  type Manifest = {
    product?: string;
    platform?: string;
    latest?: string;
    versions: ManifestVersion[];
  };

  let manifestUrl = '';
  let manifest: Manifest | null = null;
  let errorText = '';
  let loading = false;

  let downloadStatus: { version: string; percent?: number; bytes?: number; total?: number; message?: string } | null = null;
  let installStatus: { message: string } | null = null;

  async function loadManifest() {
    errorText = '';
    installStatus = null;
    downloadStatus = null;
    manifest = null;
    const url = manifestUrl.trim();
    if (!url) {
      errorText = 'Please enter manifest URL';
      return;
    }
    loading = true;
    try {
      const res = await fetch(url, { cache: 'no-store' });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data = (await res.json()) as Manifest;
      if (!data || !Array.isArray(data.versions)) throw new Error('Invalid manifest');
      manifest = data;
    } catch (e: any) {
      errorText = String(e?.message || e || 'Failed to load manifest');
    } finally {
      loading = false;
    }
  }

  async function downloadMsi(v: ManifestVersion) {
    errorText = '';
    installStatus = null;
    downloadStatus = { version: v.version, message: 'Starting...' };
    try {
      const out = await invoke<{ local_path: string }>('updater_download_msi', {
        version: v.version,
        url: v.msi.url,
        sha256: v.msi.sha256
      });
      downloadStatus = { version: v.version, percent: 100, message: `Downloaded: ${out.local_path}` };
    } catch (e: any) {
      errorText = String(e?.message || e || 'Download failed');
      downloadStatus = null;
    }
  }

  async function installMsi(v: ManifestVersion) {
    errorText = '';
    installStatus = { message: 'Launching installer...' };
    downloadStatus = null;
    try {
      await invoke('updater_install_msi', { version: v.version });
      installStatus = { message: 'Installer launched. OwlTools will close.' };
    } catch (e: any) {
      errorText = String(e?.message || e || 'Install failed');
      installStatus = null;
    }
  }

  let unlistenProgress: (() => void) | null = null;
  let unlistenInfo: (() => void) | null = null;

  onMount(async () => {
    unlistenProgress = await listen<any>('updater://download_progress', (ev) => {
      const p = ev.payload as any;
      if (!p || !p.version) return;
      downloadStatus = {
        version: p.version,
        percent: p.percent,
        bytes: p.bytes,
        total: p.total,
        message: p.message
      };
    });
    unlistenInfo = await listen<any>('updater://info', (ev) => {
      const p = ev.payload as any;
      if (!p || !p.message) return;
      installStatus = { message: String(p.message) };
    });
  });

  onDestroy(() => {
    unlistenProgress?.();
    unlistenInfo?.();
  });
</script>

<div class="updater">
  <h2>Updater</h2>

  <div class="row">
    <input class="text" type="text" bind:value={manifestUrl} placeholder="Manifest URL (JSON)" />
    <button class="btn" type="button" on:click={loadManifest} disabled={loading}>
      {loading ? 'Loading...' : 'Load'}
    </button>
  </div>

  {#if errorText}
    <div class="error">{errorText}</div>
  {/if}

  {#if downloadStatus}
    <div class="status">
      <div class="status-title">Downloading {downloadStatus.version}</div>
      {#if downloadStatus.percent != null}
        <div class="status-sub">{downloadStatus.percent.toFixed(0)}%</div>
      {/if}
      {#if downloadStatus.message}
        <div class="status-sub">{downloadStatus.message}</div>
      {/if}
    </div>
  {/if}

  {#if installStatus}
    <div class="status">
      <div class="status-title">Installer</div>
      <div class="status-sub">{installStatus.message}</div>
    </div>
  {/if}

  {#if manifest}
    <div class="meta">
      <div><strong>Product:</strong> {manifest.product || 'Unknown'}</div>
      <div><strong>Latest:</strong> {manifest.latest || 'Unknown'}</div>
    </div>

    <div class="list" aria-label="Versions">
      {#each manifest.versions as v (v.version)}
        <div class="item">
          <div class="left">
            <div class="ver">{v.version}</div>
            {#if v.notes}
              <div class="notes">{v.notes}</div>
            {/if}
          </div>
          <div class="right">
            <button class="btn" type="button" on:click={() => downloadMsi(v)}>Download</button>
            <button class="btn primary" type="button" on:click={() => installMsi(v)}>Install</button>
          </div>
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .updater {
    width: 100%;
    max-width: 900px;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  h2 { margin: 0 0 6px 0; }

  .row {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .text {
    flex: 1;
    min-width: 260px;
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid rgba(255, 255, 255, 0.18);
    background: rgba(0, 0, 0, 0.15);
    color: inherit;
  }

  .btn {
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid rgba(255, 255, 255, 0.18);
    background: rgba(255, 255, 255, 0.06);
    color: inherit;
    cursor: pointer;
  }

  .btn.primary {
    background: rgba(90, 160, 255, 0.18);
    border-color: rgba(90, 160, 255, 0.35);
  }

  .btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .error {
    padding: 10px 12px;
    border-radius: 10px;
    background: rgba(255, 60, 60, 0.12);
    border: 1px solid rgba(255, 60, 60, 0.25);
  }

  .status {
    padding: 10px 12px;
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.06);
    border: 1px solid rgba(255, 255, 255, 0.12);
  }

  .status-title { font-weight: 600; }
  .status-sub { opacity: 0.8; font-size: 12px; margin-top: 4px; }

  .meta {
    display: flex;
    gap: 18px;
    flex-wrap: wrap;
    opacity: 0.9;
  }

  .list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .item {
    display: flex;
    gap: 12px;
    align-items: center;
    justify-content: space-between;
    padding: 10px 12px;
    border-radius: 12px;
    border: 1px solid rgba(255, 255, 255, 0.12);
    background: rgba(0, 0, 0, 0.12);
  }

  .left { display: flex; flex-direction: column; gap: 2px; }
  .ver { font-weight: 600; }
  .notes { font-size: 12px; opacity: 0.8; }

  .right { display: flex; gap: 8px; }
</style>
