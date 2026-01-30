<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';

  let selectedFilePath = '';
  let selectedFileName = '';
  let isDragOver = false;
  let dragDepth = 0;
  let dropZoneEl: HTMLElement | null = null;
  let lastMouseX = 0;
  let lastMouseY = 0;
  let unlistenMouseMove: (() => void) | null = null;
  let unlistenWorkbenchPath: (() => void) | null = null;
  let unlistenFileDrop: (() => void) | null = null;
  let unlistenWebviewDrop: (() => void) | null = null;

  type ValidationTab = 'file' | 'config' | 'results';
  let activeTab: ValidationTab = 'file';
  let assetType: 'Generic' | 'Building' = 'Generic';
  let saveLogPath = '';
  let saveLogFileName = '';
  let blenderPath = '';
  let configMessage = '';
  let checkAllMessage = '';
  let ebtAddonsDir = '';
  let workbenchPort = 5700;
  let showMqaPopup = false;
  let mqaReport: any = null;

  const tabs: { id: ValidationTab; label: string; icon: string }[] = [
    { id: 'file', label: 'Validate asset', icon: '📄' },
    { id: 'config', label: 'Config', icon: '⚙️' },
    { id: 'results', label: 'Results', icon: '✓' }
  ];

  const assetTypes = ['Generic', 'Building'] as const;

  async function loadAutosocketSettings() {
    try {
      const s = await invoke<any>('get_autosocket_settings');
      const bp = (s?.blenderPath ?? s?.blender_path) as any;
      if (bp && typeof bp === 'string') {
        blenderPath = bp;
      }
      const ebt = (s?.ebtAddonsDir ?? s?.ebt_addons_dir) as any;
      if (ebt && typeof ebt === 'string') {
        ebtAddonsDir = ebt;
      }
    } catch (err) {
      console.error('Failed to load autosocket settings', err);
    }
  }

  function loadWorkbenchPort() {
    try {
      const p = localStorage.getItem('workbench_port');
      if (p) {
        const n = parseInt(p);
        if (Number.isFinite(n)) workbenchPort = Math.max(1, Math.min(65535, n));
      }
    } catch {}
  }

  function saveWorkbenchPort() {
    try {
      localStorage.setItem('workbench_port', String(workbenchPort));
    } catch {}
  }

  async function handleCheckAll() {
    checkAllMessage = '';
    if (!selectedFilePath) {
      checkAllMessage = 'Please select .xob file first';
      return;
    }
    if (!selectedFilePath.toLowerCase().endsWith('.xob')) {
      checkAllMessage = 'Selected file must be .xob';
      return;
    }
    try {
      const res = await invoke<any>('mqa_report_from_xob', { xobPath: selectedFilePath, workbenchPort });
      mqaReport = res;
      showMqaPopup = true;
      checkAllMessage = 'MQA report generated';
    } catch (err: any) {
      checkAllMessage = String(err?.message || err || 'Failed to open Blender');
    }
  }

  function closeMqaPopup() {
    showMqaPopup = false;
  }

  function handleBackdropClick(e: MouseEvent) {
    if (e.target === e.currentTarget) {
      closeMqaPopup();
    }
  }

  function handleBackdropKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      closeMqaPopup();
    }
  }

  async function pickBlenderPath() {
    try {
      const selection = await open({
        multiple: false,
        filters: [{ name: 'Blender', extensions: ['exe'] }]
      });
      if (typeof selection === 'string' && selection) {
        blenderPath = selection;
        invoke('remember_blender_path', { path: blenderPath }).catch(() => {});
      }
    } catch (err) {
      console.error('Blender selection failed', err);
    }
  }

  function clearBlenderPath() {
    blenderPath = '';
    invoke('remember_blender_path', { path: null }).catch(() => {});
  }

  async function pickEbtAddonsDir() {
    try {
      const selection = await open({ directory: true, multiple: false });
      if (typeof selection === 'string' && selection) {
        ebtAddonsDir = selection;
        invoke('remember_ebt_addons_dir', { path: ebtAddonsDir }).catch(() => {});
      }
    } catch (err) {
      console.error('EBT addons dir selection failed', err);
    }
  }

  function clearEbtAddonsDir() {
    ebtAddonsDir = '';
    invoke('remember_ebt_addons_dir', { path: null }).catch(() => {});
  }

  async function openFbxInBlender() {
    configMessage = '';
    if (!selectedFilePath) {
      configMessage = 'Please select .xob file first';
      return;
    }
    if (!selectedFilePath.toLowerCase().endsWith('.xob')) {
      configMessage = 'Selected file must be .xob';
      return;
    }
    try {
      await invoke('open_fbx_in_blender', { xobPath: selectedFilePath, workbenchPort });
      configMessage = 'Opening FBX in Blender...';
    } catch (err: any) {
      configMessage = String(err?.message || err || 'Failed to open FBX');
    }
  }

  async function pickSaveLogDirectory() {
    try {
      const selection = await open({
        directory: true,
        multiple: false
      });
      if (typeof selection === 'string' && selection) {
        saveLogPath = selection;
        saveLogFileName = 'validation-log.txt';
      }
    } catch (err) {
      console.error('Directory selection failed', err);
    }
  }

  function clearSaveLogPath() {
    saveLogPath = '';
    saveLogFileName = '';
  }

  function switchTab(tab: ValidationTab) {
    activeTab = tab;
  }

  function chooseDropEffect(e: DragEvent) {
    const dt = e.dataTransfer;
    if (!dt) return;
    const types = Array.from((dt.types || []) as any);
    const hasFiles = types.some(t => {
      const lower = String(t).toLowerCase();
      return lower === 'files' || lower.includes('file');
    });
    if (hasFiles) {
      dt.dropEffect = 'copy';
      return;
    }
    if (types.includes('text/uri-list') || types.some(t => String(t).includes('uri'))) {
      dt.dropEffect = 'link';
      return;
    }
    const allowed = (dt.effectAllowed || '').toLowerCase();
    if (allowed.includes('copy') || allowed === 'all' || allowed === 'uninitialized' || allowed === '') {
      dt.dropEffect = 'copy';
    } else if (allowed.includes('link')) {
      dt.dropEffect = 'link';
    } else if (allowed.includes('move')) {
      dt.dropEffect = 'move';
    } else {
      dt.dropEffect = 'copy';
    }
  }

  function parseWorkbenchDrag(data: string): string | null {
    let s = String(data || '').trim();
    if (!s) return null;
    try {
      s = decodeURIComponent(s);
    } catch {}
    if (s.startsWith('{') && s.indexOf('}') > 0) {
      s = s.slice(s.indexOf('}') + 1);
    }
    const qIdx = s.indexOf('?');
    if (qIdx >= 0) {
      const query = s.slice(qIdx + 1);
      const pairs = query.split('&');
      for (const p of pairs) {
        const [k, v] = p.split('=');
        if (k === 'exactPath' && v) return v;
      }
      s = s.slice(0, qIdx);
    }
    if (s.startsWith('file:')) s = s.slice(5);
    return s || null;
  }

  function extractWorkbenchText(dt: DataTransfer): string {
    const candidates = ['text/uri-list', 'text/plain', 'text/x-moz-url', 'text/x-url', 'URL'];
    for (const t of candidates) {
      try {
        const v = dt.getData(t);
        if (v) return v;
      } catch {}
    }
    if (dt.types) {
      for (const t of Array.from(dt.types as any)) {
        try {
          const v = dt.getData(t as string);
          if (v) return v;
        } catch {}
      }
    }
    return '';
  }

  function applyDroppedPath(raw: string) {
    if (!raw) return;
    selectedFilePath = raw;
    selectedFileName = raw.split(/[\\/]/).pop() ?? raw;
  }

  async function pickFile() {
    try {
      const selection = await open({
        multiple: false,
        filters: [
          { name: 'All Files', extensions: ['*'] },
          { name: 'Text Files', extensions: ['txt', 'log'] },
          { name: 'JSON Files', extensions: ['json'] }
        ]
      });
      if (typeof selection === 'string' && selection) {
        applyDroppedPath(selection);
      }
    } catch (err) {
      console.error('File selection failed', err);
    }
  }

  function handleDragOver(e: DragEvent) {
    e.preventDefault();
    e.stopPropagation();
    chooseDropEffect(e);
    isDragOver = true;
  }

  function handleDragEnter(e: DragEvent) {
    e.preventDefault();
    e.stopPropagation();
    chooseDropEffect(e);
    dragDepth += 1;
    isDragOver = true;
    window.dispatchEvent(new CustomEvent('app:set-module-drag-active', { detail: true }));
  }

  function handleDragLeave(e: DragEvent) {
    e.preventDefault();
    e.stopPropagation();
    dragDepth = Math.max(0, dragDepth - 1);
    if (dragDepth === 0) {
      isDragOver = false;
      window.dispatchEvent(new CustomEvent('app:set-module-drag-active', { detail: false }));
    }
  }

  function handleDrop(e: DragEvent) {
    e.preventDefault();
    e.stopPropagation();
    isDragOver = false;
    dragDepth = 0;
    window.dispatchEvent(new CustomEvent('app:set-module-drag-active', { detail: false }));
    
    const dt = e.dataTransfer;
    if (!dt) return;
    
    if (dt.files && dt.files.length > 0) {
      const first = dt.files[0];
      const path = (first as any)?.path || first.name;
      if (typeof path === 'string') {
        applyDroppedPath(path);
      }
      return;
    }
    
    const raw = extractWorkbenchText(dt);
    if (!raw) return;
    let parsed = parseWorkbenchDrag(raw);
    if (!parsed && raw.includes('\n')) {
      parsed = parseWorkbenchDrag(raw.split('\n')[0]);
    }
    if (!parsed) return;
    applyDroppedPath(parsed);
  }

  function clearSelection() {
    selectedFilePath = '';
    selectedFileName = '';
  }

  onMount(() => {
    loadWorkbenchPort();
    const onMouseMove = (e: MouseEvent) => {
      lastMouseX = e.clientX;
      lastMouseY = e.clientY;
    };
    window.addEventListener('mousemove', onMouseMove, { capture: true } as any);
    unlistenMouseMove = () => window.removeEventListener('mousemove', onMouseMove, { capture: true } as any);

    try {
      window.dispatchEvent(new CustomEvent('app:set-allow-drop', { detail: true }));
    } catch {}

    const onWorkbench = (e: any) => {
      try {
        const detail = e?.detail;
        if (typeof detail === 'string' && detail) {
          applyDroppedPath(detail);
        }
      } catch {}
    };
    window.addEventListener('workbench-drop-path', onWorkbench as any);
    unlistenWorkbenchPath = () => window.removeEventListener('workbench-drop-path', onWorkbench as any);

    listen<string[] | string>('tauri://file-drop', (e) => {
      const payload = e.payload as any;
      const paths = Array.isArray(payload) ? payload : [payload];
      for (const p of paths) {
        if (typeof p === 'string' && p) {
          applyDroppedPath(p);
        }
      }
    }).then((u) => {
      unlistenFileDrop = u;
    });

    try {
      const webview = getCurrentWebviewWindow();
      webview.onDragDropEvent((ev: any) => {
        const t = ev?.payload?.type ?? ev?.type ?? ev?.event ?? ev?.kind;
        const updateHover = () => {
          const rawX =
            typeof ev?.payload?.position?.x === 'number'
              ? ev.payload.position.x
              : typeof ev?.payload?.x === 'number'
                ? ev.payload.x
                : typeof ev?.x === 'number'
                  ? ev.x
                  : lastMouseX;
          const rawY =
            typeof ev?.payload?.position?.y === 'number'
              ? ev.payload.position.y
              : typeof ev?.payload?.y === 'number'
                ? ev.payload.y
                : typeof ev?.y === 'number'
                  ? ev.y
                  : lastMouseY;

          let x = Number(rawX);
          let y = Number(rawY);
          if (x > window.innerWidth * 2 || y > window.innerHeight * 2) {
            x = x - window.screenX;
            y = y - window.screenY;
          }
          x = Math.max(0, Math.min(window.innerWidth - 1, x));
          y = Math.max(0, Math.min(window.innerHeight - 1, y));

          const dropRect = dropZoneEl?.getBoundingClientRect();
          const inRect = (r: DOMRect | undefined) => !!(r && x >= r.left && x <= r.right && y >= r.top && y <= r.bottom);
          isDragOver = inRect(dropRect);
          window.dispatchEvent(new CustomEvent('app:set-module-drag-active', { detail: isDragOver }));
        };

        if (t === 'hover' || t === 'over' || t === 'enter') {
          updateHover();
        }
        if (t === 'leave' || t === 'cancelled') {
          isDragOver = false;
          window.dispatchEvent(new CustomEvent('app:set-module-drag-active', { detail: false }));
        }
        if (t === 'drop') {
          isDragOver = false;
          dragDepth = 0;
          window.dispatchEvent(new CustomEvent('app:set-module-drag-active', { detail: false }));
          const pathsAny = ev?.payload?.paths ?? ev?.paths ?? ev?.payload;
          const paths = Array.isArray(pathsAny) ? pathsAny : [pathsAny];
          for (const p of paths) {
            if (typeof p === 'string' && p) {
              applyDroppedPath(p);
            }
          }
        }
      }).then((u) => {
        unlistenWebviewDrop = u;
      });
    } catch {}

    loadAutosocketSettings();
  });

  onDestroy(() => {
    try {
      window.dispatchEvent(new CustomEvent('app:set-allow-drop', { detail: false }));
    } catch {}
    unlistenMouseMove?.();
    unlistenMouseMove = null;
    unlistenWorkbenchPath?.();
    unlistenWorkbenchPath = null;
    unlistenFileDrop?.();
    unlistenFileDrop = null;
    unlistenWebviewDrop?.();
    unlistenWebviewDrop = null;
  });
</script>

<div class="validate-container">
  <div class="header">
    <h2>Validate</h2>
    <div class="tab-bar">
      {#each tabs as tab (tab.id)}
        <button
          class="tab-btn"
          class:active={activeTab === tab.id}
          on:click={() => switchTab(tab.id)}
        >
          <span class="tab-icon">{tab.icon}</span>
          <span class="tab-label">{tab.label}</span>
        </button>
      {/each}
    </div>
  </div>

  <div class="content">
    {#if activeTab === 'file'}
      <div class="tab-content">
        <div class="file-picker-section">
          <h3>Select File</h3>
          <div
            bind:this={dropZoneEl}
            class="drop-zone"
            class:drag-over={isDragOver}
            role="button"
            tabindex="0"
            on:dragover={handleDragOver}
            on:dragenter={handleDragEnter}
            on:dragleave={handleDragLeave}
            on:drop={handleDrop}
          >
            {#if selectedFilePath}
              <div class="file-info">
                <span class="file-icon">📄</span>
                <div class="file-details">
                  <div class="file-name">{selectedFileName}</div>
                  <div class="file-path">{selectedFilePath}</div>
                </div>
              </div>
            {:else}
              <div class="drop-hint">
                <span class="drop-icon">📁</span>
                <p>Drag and drop a file here</p>
                <p class="or-text">or</p>
              </div>
            {/if}
          </div>
          <div class="button-group">
            <button class="btn btn-primary" on:click={pickFile}>
              Browse File
            </button>
            {#if selectedFilePath}
              <button class="btn btn-secondary" on:click={clearSelection}>
                Clear
              </button>
            {/if}
          </div>

          <div class="asset-type-section">
            <label for="asset-type">Asset Type:</label>
            <select id="asset-type" bind:value={assetType} class="asset-type-select">
              {#each assetTypes as type}
                <option value={type}>{type}</option>
              {/each}
            </select>
          </div>

          <div class="save-log-section">
            <h3>Save Log As</h3>
            <div class="save-log-content">
              {#if saveLogPath}
                <div class="save-log-info">
                  <span class="save-log-icon">💾</span>
                  <div class="save-log-details">
                    <div class="save-log-name">{saveLogFileName}</div>
                    <div class="save-log-path">{saveLogPath}</div>
                  </div>
                </div>
              {:else}
                <div class="save-log-hint">
                  <span class="save-log-hint-icon">📁</span>
                  <p>Select a directory to save validation logs</p>
                </div>
              {/if}
            </div>
            <div class="save-log-buttons">
              <button class="btn btn-primary" on:click={pickSaveLogDirectory}>
                Choose Directory
              </button>
              {#if saveLogPath}
                <button class="btn btn-secondary" on:click={clearSaveLogPath}>
                  Clear
                </button>
              {/if}
            </div>
          </div>

          <button class="btn btn-check-all" on:click={handleCheckAll}>
            <span class="check-all-icon">✓</span>
            Check All
          </button>
          {#if checkAllMessage}
            <div class="check-all-message">{checkAllMessage}</div>
          {/if}
        </div>
      </div>
    {:else if activeTab === 'config'}
      <div class="tab-content">
        <div class="config-section">
          <h3>Configuration</h3>

          <div class="config-block">
            <div class="config-title">Blender Path</div>
            <div class="config-row">
              <input class="config-input" readonly value={blenderPath} placeholder="Select blender.exe" />
              <button class="btn btn-secondary" on:click={pickBlenderPath}>Browse</button>
              {#if blenderPath}
                <button class="btn btn-secondary" on:click={clearBlenderPath}>Clear</button>
              {/if}
            </div>
            <div class="config-title">EBT Addons Folder</div>
            <div class="config-row">
              <input class="config-input" readonly value={ebtAddonsDir} placeholder="Select .../Blender/scripts/addons" />
              <button class="btn btn-secondary" on:click={pickEbtAddonsDir}>Browse</button>
              {#if ebtAddonsDir}
                <button class="btn btn-secondary" on:click={clearEbtAddonsDir}>Clear</button>
              {/if}
            </div>
            <div class="config-title">Workbench Port</div>
            <div class="config-row">
              <input
                class="config-input"
                type="number"
                min="1"
                max="65535"
                bind:value={workbenchPort}
                on:change={saveWorkbenchPort}
              />
            </div>
            <div class="config-actions">
              <button class="btn btn-primary" on:click={openFbxInBlender}>Open FBX in Blender</button>
            </div>
            {#if configMessage}
              <div class="config-message">{configMessage}</div>
            {/if}
          </div>
        </div>
      </div>
    {:else if activeTab === 'results'}
      <div class="tab-content">
        <div class="placeholder-section">
          <div class="placeholder-icon">✓</div>
          <h3>Validation Results</h3>
          <p>Results will appear here after validation is complete.</p>
          <div class="placeholder-hint">Select a file and validate to see results</div>
        </div>
      </div>
    {/if}
  </div>
</div>

{#if showMqaPopup}
  <div
    class="modal-backdrop"
    role="dialog"
    aria-modal="true"
    tabindex="0"
    on:click={handleBackdropClick}
    on:keydown={handleBackdropKeydown}
  >
    <div class="modal" role="document" tabindex="-1">
      <div class="modal-header">
        <div class="modal-title">MQA Report</div>
        <button class="btn btn-secondary modal-close" on:click={closeMqaPopup}>Close</button>
      </div>
      <div class="modal-subtitle">
        <div class="modal-meta">
          <div class="modal-meta-row">FBX: {mqaReport?.fbx ?? ''}</div>
          <div class="modal-meta-row">Items: {mqaReport?.count ?? 0}</div>
        </div>
      </div>
      <div class="modal-body">
        {#if Array.isArray(mqaReport?.items) && mqaReport.items.length}
          <div class="mqa-list">
            {#each mqaReport.items as item, idx (idx)}
              <div class="mqa-item">
                <div class="mqa-line">
                  <span class="mqa-cat">[{item?.category ?? ''}]</span>
                  <span class="mqa-msg">{item?.message ?? ''}</span>
                  {#if (item?.count ?? 0) > 0}
                    <span class="mqa-count">[{item.count}]</span>
                  {/if}
                </div>
                {#if Array.isArray(item?.objects) && item.objects.length}
                  <div class="mqa-objects">
                    {#each item.objects as objName, j (j)}
                      <div class="mqa-obj">- {objName}</div>
                    {/each}
                  </div>
                {/if}
              </div>
            {/each}
          </div>
        {:else}
          {#if Array.isArray(mqaReport?.errors) && mqaReport.errors.length}
            <div class="modal-empty">No reports to display.</div>
            <div class="mqa-errors">
              {#each mqaReport.errors as errLine, ei (ei)}
                <div class="mqa-err">{errLine}</div>
              {/each}
            </div>
          {:else}
            <div class="modal-empty">No reports to display.</div>
          {/if}
          {#if mqaReport?.debug}
            <div class="mqa-debug">
              <div class="mqa-debug-title">Debug</div>
              <pre class="mqa-debug-pre">{JSON.stringify(mqaReport.debug, null, 2)}</pre>
            </div>
          {/if}
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .validate-container {
    display: flex;
    flex-direction: column;
    height: 100%;
    width: 100%;
  }

  .header {
    padding: 16px;
    border-bottom: 1px solid rgba(255, 255, 255, 0.1);
  }

  .header h2 {
    margin: 0 0 12px 0;
    font-size: 24px;
    font-weight: 600;
  }

  .tab-bar {
    display: flex;
    gap: 8px;
    border-top: 1px solid rgba(255, 255, 255, 0.1);
    padding-top: 12px;
  }

  .tab-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 16px;
    background: transparent;
    border: none;
    color: rgba(255, 255, 255, 0.6);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    border-bottom: 2px solid transparent;
    transition: all 120ms ease;
    white-space: nowrap;
  }

  .tab-btn:hover {
    color: rgba(255, 255, 255, 0.8);
  }

  .tab-btn.active {
    color: #1f73e6;
    border-bottom-color: #1f73e6;
  }

  .tab-icon {
    font-size: 16px;
  }

  .tab-label {
    display: inline;
  }

  :global(body.theme-light) .tab-btn {
    color: rgba(17, 17, 17, 0.6);
  }

  :global(body.theme-light) .tab-btn:hover {
    color: rgba(17, 17, 17, 0.8);
  }

  :global(body.theme-light) .tab-btn.active {
    color: #1f73e6;
    border-bottom-color: #1f73e6;
  }

  .content {
    flex: 1;
    padding: 16px;
    overflow-y: auto;
  }

  .file-picker-section {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .asset-type-section {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px;
    background: rgba(255, 255, 255, 0.05);
    border-radius: 6px;
  }

  .asset-type-section label {
    font-size: 13px;
    font-weight: 500;
    color: rgba(255, 255, 255, 0.8);
    white-space: nowrap;
  }

  .asset-type-select {
    flex: 1;
    padding: 6px 10px;
    background: rgba(255, 255, 255, 0.1);
    border: 1px solid rgba(255, 255, 255, 0.2);
    border-radius: 4px;
    color: rgba(255, 255, 255, 0.9);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: all 120ms ease;
  }

  .asset-type-select:hover {
    background: rgba(255, 255, 255, 0.15);
    border-color: rgba(255, 255, 255, 0.3);
  }

  .asset-type-select:focus {
    outline: none;
    background: rgba(255, 255, 255, 0.15);
    border-color: #1f73e6;
    box-shadow: 0 0 0 2px rgba(31, 115, 230, 0.2);
  }

  .asset-type-select option {
    background: #1e1e1e;
    color: rgba(255, 255, 255, 0.9);
  }

  :global(body.theme-light) .asset-type-section {
    background: rgba(17, 17, 17, 0.05);
  }

  :global(body.theme-light) .asset-type-section label {
    color: rgba(17, 17, 17, 0.8);
  }

  :global(body.theme-light) .asset-type-select {
    background: rgba(17, 17, 17, 0.1);
    border-color: rgba(17, 17, 17, 0.2);
    color: rgba(17, 17, 17, 0.9);
  }

  :global(body.theme-light) .asset-type-select:hover {
    background: rgba(17, 17, 17, 0.15);
    border-color: rgba(17, 17, 17, 0.3);
  }

  :global(body.theme-light) .asset-type-select:focus {
    background: rgba(17, 17, 17, 0.15);
    border-color: #1f73e6;
    box-shadow: 0 0 0 2px rgba(31, 115, 230, 0.2);
  }

  :global(body.theme-light) .asset-type-select option {
    background: #ffffff;
    color: rgba(17, 17, 17, 0.9);
  }

  .file-picker-section h3 {
    margin: 0;
    font-size: 16px;
    font-weight: 500;
  }

  .save-log-section {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px;
    background: rgba(255, 255, 255, 0.05);
    border-radius: 6px;
  }

  .save-log-section h3 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    color: rgba(255, 255, 255, 0.9);
  }

  .save-log-content {
    min-height: 48px;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 10px;
    background: rgba(255, 255, 255, 0.03);
    border-radius: 4px;
    border: 1px dashed rgba(255, 255, 255, 0.2);
  }

  .save-log-info {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
  }

  .save-log-icon {
    font-size: 24px;
    flex-shrink: 0;
  }

  .save-log-details {
    flex: 1;
    min-width: 0;
  }

  .save-log-name {
    font-size: 13px;
    font-weight: 500;
    color: rgba(255, 255, 255, 0.9);
    margin-bottom: 2px;
    word-break: break-word;
  }

  .save-log-path {
    font-size: 11px;
    color: rgba(255, 255, 255, 0.5);
    word-break: break-all;
  }

  .save-log-hint {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    text-align: center;
  }

  .save-log-hint-icon {
    font-size: 28px;
    opacity: 0.4;
  }

  .save-log-hint p {
    margin: 0;
    font-size: 12px;
    color: rgba(255, 255, 255, 0.5);
  }

  .save-log-buttons {
    display: flex;
    gap: 8px;
  }

  :global(body.theme-light) .save-log-section {
    background: rgba(17, 17, 17, 0.05);
  }

  :global(body.theme-light) .save-log-section h3 {
    color: rgba(17, 17, 17, 0.9);
  }

  :global(body.theme-light) .save-log-content {
    background: rgba(17, 17, 17, 0.03);
    border-color: rgba(17, 17, 17, 0.2);
  }

  :global(body.theme-light) .save-log-name {
    color: rgba(17, 17, 17, 0.9);
  }

  :global(body.theme-light) .save-log-path {
    color: rgba(17, 17, 17, 0.5);
  }

  :global(body.theme-light) .save-log-hint p {
    color: rgba(17, 17, 17, 0.5);
  }

  .drop-zone {
    border: 2px dashed rgba(255, 255, 255, 0.3);
    border-radius: 8px;
    padding: 20px;
    min-height: 140px;
    text-align: center;
    transition: all 200ms ease;
    cursor: pointer;
    background: rgba(255, 255, 255, 0.02);
  }

  .drop-zone:hover {
    border-color: rgba(255, 255, 255, 0.5);
    background: rgba(255, 255, 255, 0.05);
  }

  .drop-zone.drag-over {
    border-color: #1f73e6;
    background: rgba(31, 115, 230, 0.1);
  }

  :global(body.theme-light) .drop-zone {
    border-color: rgba(17, 17, 17, 0.3);
    background: rgba(17, 17, 17, 0.02);
  }

  :global(body.theme-light) .drop-zone:hover {
    border-color: rgba(17, 17, 17, 0.5);
    background: rgba(17, 17, 17, 0.05);
  }

  :global(body.theme-light) .drop-zone.drag-over {
    border-color: #1f73e6;
    background: rgba(31, 115, 230, 0.1);
  }

  .drop-hint {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
  }

  .drop-icon {
    font-size: 40px;
    opacity: 0.5;
  }

  .drop-hint p {
    margin: 0;
    font-size: 14px;
    color: rgba(255, 255, 255, 0.7);
  }

  :global(body.theme-light) .drop-hint p {
    color: rgba(17, 17, 17, 0.7);
  }

  .or-text {
    font-size: 12px;
    opacity: 0.5;
  }

  .file-info {
    display: flex;
    align-items: center;
    gap: 16px;
  }

  .file-icon {
    font-size: 32px;
  }

  .file-details {
    text-align: left;
    flex: 1;
    min-width: 0;
  }

  .file-name {
    font-size: 14px;
    font-weight: 500;
    margin-bottom: 4px;
    word-break: break-word;
  }

  .file-path {
    font-size: 12px;
    color: rgba(255, 255, 255, 0.5);
    word-break: break-all;
  }

  :global(body.theme-light) .file-path {
    color: rgba(17, 17, 17, 0.5);
  }

  .button-group {
    display: flex;
    gap: 8px;
  }

  .btn {
    padding: 8px 16px;
    border-radius: 4px;
    border: none;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: background 120ms ease;
  }

  .btn-primary {
    background: #1f73e6;
    color: #fff;
    flex: 1;
  }

  .btn-primary:hover {
    background: #1557b0;
  }

  .btn-primary:active {
    background: #0d47a1;
  }

  .btn-secondary {
    background: rgba(255, 255, 255, 0.1);
    color: inherit;
  }

  .btn-secondary:hover {
    background: rgba(255, 255, 255, 0.15);
  }

  :global(body.theme-light) .btn-secondary {
    background: rgba(17, 17, 17, 0.1);
  }

  :global(body.theme-light) .btn-secondary:hover {
    background: rgba(17, 17, 17, 0.15);
  }

  .btn-check-all {
    width: 100%;
    padding: 12px 20px;
    background: linear-gradient(135deg, #1f73e6 0%, #1557b0 100%);
    color: #fff;
    border: none;
    border-radius: 8px;
    font-size: 15px;
    font-weight: 600;
    cursor: pointer;
    transition: all 200ms ease;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    margin-top: 4px;
  }

  .btn-check-all:hover {
    background: linear-gradient(135deg, #1557b0 0%, #0d47a1 100%);
    transform: translateY(-2px);
    box-shadow: 0 4px 12px rgba(31, 115, 230, 0.3);
  }

  .btn-check-all:active {
    background: linear-gradient(135deg, #0d47a1 0%, #0a3a80 100%);
    transform: translateY(0);
  }

  .check-all-icon {
    font-size: 18px;
  }

  .check-all-message {
    margin-top: 8px;
    font-size: 12px;
    color: rgba(255, 255, 255, 0.65);
    word-break: break-word;
  }

  :global(body.theme-light) .check-all-message {
    color: rgba(17, 17, 17, 0.65);
  }

  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 16px;
    z-index: 1000;
  }

  .modal {
    width: min(920px, 100%);
    max-height: min(80vh, 760px);
    overflow: hidden;
    display: flex;
    flex-direction: column;
    border-radius: 10px;
    border: 1px solid rgba(255, 255, 255, 0.12);
    background: rgba(30, 30, 30, 0.98);
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.45);
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 14px;
    border-bottom: 1px solid rgba(255, 255, 255, 0.08);
  }

  .modal-title {
    font-size: 14px;
    font-weight: 700;
    color: rgba(255, 255, 255, 0.9);
  }

  .modal-close {
    flex: 0 0 auto;
  }

  .modal-subtitle {
    padding: 10px 14px;
    border-bottom: 1px solid rgba(255, 255, 255, 0.08);
  }

  .modal-meta {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: rgba(255, 255, 255, 0.65);
  }

  .modal-body {
    padding: 12px 14px;
    overflow: auto;
  }

  .modal-empty {
    font-size: 12px;
    color: rgba(255, 255, 255, 0.65);
  }

  .mqa-errors {
    margin-top: 10px;
    padding: 10px;
    border-radius: 8px;
    background: rgba(255, 70, 70, 0.08);
    border: 1px solid rgba(255, 70, 70, 0.22);
    color: rgba(255, 255, 255, 0.78);
    font-size: 12px;
    white-space: pre-wrap;
    word-break: break-word;
  }

  .mqa-err {
    padding: 2px 0;
  }

  .mqa-debug {
    margin-top: 10px;
    padding: 10px;
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.04);
    border: 1px solid rgba(255, 255, 255, 0.08);
  }

  .mqa-debug-title {
    font-size: 12px;
    font-weight: 700;
    color: rgba(255, 255, 255, 0.78);
    margin-bottom: 6px;
  }

  .mqa-debug-pre {
    margin: 0;
    font-size: 12px;
    line-height: 1.35;
    color: rgba(255, 255, 255, 0.65);
    white-space: pre-wrap;
    word-break: break-word;
  }

  .mqa-list {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .mqa-item {
    padding: 10px;
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.04);
    border: 1px solid rgba(255, 255, 255, 0.08);
  }

  .mqa-line {
    display: flex;
    gap: 8px;
    align-items: baseline;
    flex-wrap: wrap;
  }

  .mqa-cat {
    font-weight: 700;
    color: rgba(255, 255, 255, 0.85);
  }

  .mqa-msg {
    color: rgba(255, 255, 255, 0.75);
  }

  .mqa-count {
    color: rgba(255, 255, 255, 0.55);
    font-size: 12px;
  }

  .mqa-objects {
    margin-top: 8px;
    padding-left: 8px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: 12px;
    color: rgba(255, 255, 255, 0.6);
  }

  .mqa-obj {
    word-break: break-word;
  }

  :global(body.theme-light) .modal {
    background: rgba(255, 255, 255, 0.98);
    border-color: rgba(17, 17, 17, 0.12);
  }

  :global(body.theme-light) .modal-title {
    color: rgba(17, 17, 17, 0.9);
  }

  :global(body.theme-light) .modal-meta,
  :global(body.theme-light) .modal-empty,
  :global(body.theme-light) .mqa-msg,
  :global(body.theme-light) .mqa-count,
  :global(body.theme-light) .mqa-objects {
    color: rgba(17, 17, 17, 0.65);
  }

  :global(body.theme-light) .mqa-errors {
    color: rgba(17, 17, 17, 0.75);
    background: rgba(230, 40, 40, 0.06);
    border-color: rgba(230, 40, 40, 0.2);
  }

  :global(body.theme-light) .mqa-debug-title {
    color: rgba(17, 17, 17, 0.78);
  }

  :global(body.theme-light) .mqa-debug-pre {
    color: rgba(17, 17, 17, 0.65);
  }

  :global(body.theme-light) .mqa-cat {
    color: rgba(17, 17, 17, 0.85);
  }

  :global(body.theme-light) .mqa-item {
    background: rgba(17, 17, 17, 0.04);
    border-color: rgba(17, 17, 17, 0.08);
  }

  .tab-content {
    animation: fadeIn 150ms ease;
  }

  .config-section {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .config-section h3 {
    margin: 0;
    font-size: 16px;
    font-weight: 600;
  }

  .config-block {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px;
    background: rgba(255, 255, 255, 0.05);
    border-radius: 6px;
  }

  .config-title {
    font-size: 13px;
    font-weight: 600;
    color: rgba(255, 255, 255, 0.9);
  }

  .config-row {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .config-input {
    flex: 1;
    min-width: 0;
    padding: 8px 10px;
    background: rgba(255, 255, 255, 0.06);
    border: 1px solid rgba(255, 255, 255, 0.14);
    border-radius: 4px;
    color: rgba(255, 255, 255, 0.9);
    font-size: 12px;
  }

  .config-actions {
    display: flex;
    gap: 8px;
  }

  .config-message {
    font-size: 12px;
    color: rgba(255, 255, 255, 0.65);
    word-break: break-word;
  }

  :global(body.theme-light) .config-block {
    background: rgba(17, 17, 17, 0.05);
  }

  :global(body.theme-light) .config-title {
    color: rgba(17, 17, 17, 0.9);
  }

  :global(body.theme-light) .config-input {
    background: rgba(17, 17, 17, 0.06);
    border-color: rgba(17, 17, 17, 0.14);
    color: rgba(17, 17, 17, 0.9);
  }

  :global(body.theme-light) .config-message {
    color: rgba(17, 17, 17, 0.65);
  }

  @keyframes fadeIn {
    from {
      opacity: 0;
    }
    to {
      opacity: 1;
    }
  }

  .placeholder-section {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 16px;
    padding: 48px 24px;
    text-align: center;
    min-height: 300px;
  }

  .placeholder-icon {
    font-size: 64px;
    opacity: 0.3;
  }

  .placeholder-section h3 {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
    color: rgba(255, 255, 255, 0.9);
  }

  .placeholder-section p {
    margin: 0;
    font-size: 14px;
    color: rgba(255, 255, 255, 0.6);
    max-width: 300px;
  }

  .placeholder-hint {
    font-size: 12px;
    color: rgba(255, 255, 255, 0.4);
    font-style: italic;
  }

  :global(body.theme-light) .placeholder-section h3 {
    color: rgba(17, 17, 17, 0.9);
  }

  :global(body.theme-light) .placeholder-section p {
    color: rgba(17, 17, 17, 0.6);
  }

  :global(body.theme-light) .placeholder-hint {
    color: rgba(17, 17, 17, 0.4);
  }
</style>
