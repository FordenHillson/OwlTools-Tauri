<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { open, save } from '@tauri-apps/plugin-dialog';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
  type LogEntry = { id: string; type: 'info' | 'warn' | 'error'; message: string };
  let fillExpanded = true;
  let createExpanded = false;
  let logExpanded = true;
  let filterTerm = '';
  let logs: LogEntry[] = [];
  $: filteredLogs = logs.filter(log => {
    const term = filterTerm.trim().toLowerCase();
    if (!term) return true;
    return log.message.toLowerCase().includes(term) || log.type.toLowerCase().includes(term);
  });

  function toggle(section: 'fill' | 'create' | 'log') {
    if (section === 'fill') {
      fillExpanded = !fillExpanded;
    } else if (section === 'create') {
      createExpanded = !createExpanded;
    } else {
      logExpanded = !logExpanded;
    }

  }

  function chooseDropEffect(e: DragEvent) {
    const dt = e.dataTransfer;
    if (!dt) return;
    const types = Array.from((dt.types || []) as any);
    // Check for file types (case-insensitive and handle variations)
    const hasFiles = types.some(t => {
      const lower = String(t).toLowerCase();
      return lower === 'files' || lower.includes('file');
    });
    if (hasFiles) {
      dt.dropEffect = 'copy';
      return;
    }
    // Check for text/uri-list or similar
    if (types.includes('text/uri-list') || types.some(t => String(t).includes('uri'))) {
      dt.dropEffect = 'link';
      return;
    }
    // Fallback: always set a valid dropEffect
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
    // Strip leading {GUID} if present
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

  async function ensureArma4Root(): Promise<string | null> {
    if (arma4Root) return arma4Root;
    try {
      const dir = await open({ directory: true, multiple: false });
      if (typeof dir === 'string' && dir) {
        arma4Root = dir;
        try {
          localStorage.setItem('arma4Root', arma4Root);
        } catch {}
        return arma4Root;
      }
    } catch {}
    return null;
  }

  async function resolveArmaPath(s: string): Promise<string | null> {
    if (s.startsWith('$Arma4:')) {
      const root = await ensureArma4Root();
      if (!root) return null;
      const sub = s.slice('$Arma4:'.length).replaceAll('/', '\\');
      const base = root.replace(/[\\/]+$/, '');
      return base + '\\' + sub;
    }
    return s;
  }

  function normalizeXobPath(p: string): string {
    const s = String(p || '').trim();
    const lower = s.toLowerCase();
    if (lower.endsWith('.xob.meta')) return s.slice(0, -5);
    return s;
  }

  async function applyDroppedPath(raw: string) {
    if (!raw) return;
    const resolved = await resolveArmaPath(raw);
    if (!resolved) return;
    const normalized = normalizeXobPath(resolved);
    if (!normalized.toLowerCase().endsWith('.xob')) return;
    const prev = fillSelectedFilePath;
    fillSelectedFilePath = normalized;
    fillSelectedFileName = normalized.split(/[\\/]/).pop() ?? normalized;
    if (prev !== normalized) {
      const parent = normalized.split(/[\\/]/).slice(0, -1).join('\\');
      if (parent) {
        saveLocation = parent;
        invoke('remember_save_dir', { path: saveLocation }).catch(() => {});
      }
    }
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
    isDragOver = true;
  }
  function handleDragLeave(e: DragEvent) {
    e.preventDefault();
    e.stopPropagation();
    // Keep it simple: hide highlight on leave
    isDragOver = false;
    fillIsDragOver = false;
    createIsDragOver = false;
    window.dispatchEvent(new CustomEvent('app:set-module-drag-active', { detail: false }));
  }
  function handleDrop(e: DragEvent) {
    e.preventDefault();
    e.stopPropagation();
    isDragOver = false;
    const dt = e.dataTransfer;
    if (!dt) return;
    // Try to get files from dataTransfer (fallback for when Tauri events don't fire)
    if (dt.files && dt.files.length > 0) {
      const first = dt.files[0];
      const path = (first as any)?.path || first.name;
      if (typeof path === 'string') applyDroppedPath(path);
      return;
    }
    const raw = extractWorkbenchText(dt);
    if (!raw) return;
    let parsed = parseWorkbenchDrag(raw);
    if (!parsed && raw.includes('\n')) parsed = parseWorkbenchDrag(raw.split('\n')[0]);
    if (!parsed) return;
    applyDroppedPath(parsed);
  }

  type DropSection = 'fill' | 'create';
  let activeDropSection: DropSection = 'fill';
  let hoverDropSection: DropSection | null = null;
  let fillDragDepth = 0;
  let createDragDepth = 0;

  function handleDragOverFill(e: DragEvent) {
    activeDropSection = 'fill';
    fillIsDragOver = true;
    handleDragOver(e);
  }
  function handleDragEnterFill(e: DragEvent) {
    activeDropSection = 'fill';
    fillDragDepth += 1;
    fillIsDragOver = true;
    window.dispatchEvent(new CustomEvent('app:set-module-drag-active', { detail: true }));
    handleDragEnter(e);
  }
  function handleDragLeaveFill(e: DragEvent) {
    e.preventDefault();
    e.stopPropagation();
    fillDragDepth = Math.max(0, fillDragDepth - 1);
    if (fillDragDepth === 0) {
      fillIsDragOver = false;
      window.dispatchEvent(new CustomEvent('app:set-module-drag-active', { detail: false }));
    }
  }
  function handleDropFill(e: DragEvent) {
    activeDropSection = 'fill';
    fillIsDragOver = false;
    fillDragDepth = 0;
    window.dispatchEvent(new CustomEvent('app:set-module-drag-active', { detail: false }));
    handleDrop(e);
  }

  function enqueueCreateXob(normalized: string) {
    const n = String(normalized || '').trim();
    if (!n) return;
    if (createPrefabQueue.includes(n)) return;
    createPrefabQueue = [...createPrefabQueue, n];
    createSelectedFilePath = n;
    createSelectedFileName = n.split(/[\\/]/).pop() ?? n;
  }

  async function applyDroppedPathCreate(raw: string) {
    if (!raw) return;
    const resolved = await resolveArmaPath(raw);
    if (!resolved) return;
    const normalized = normalizeXobPath(resolved);
    if (!normalized.toLowerCase().endsWith('.xob')) return;

    enqueueCreateXob(normalized);

    if (!saveLocation) {
      const parent = normalized.split(/[\\/]/).slice(0, -1).join('\\');
      if (parent) {
        saveLocation = parent;
        invoke('remember_save_dir', { path: saveLocation }).catch(() => {});
      }
    }
  }

  function handleDragOverCreate(e: DragEvent) {
    activeDropSection = 'create';
    createIsDragOver = true;
    handleDragOver(e);
  }
  function handleDragEnterCreate(e: DragEvent) {
    activeDropSection = 'create';
    createDragDepth += 1;
    createIsDragOver = true;
    window.dispatchEvent(new CustomEvent('app:set-module-drag-active', { detail: true }));
    handleDragEnter(e);
  }
  function handleDragLeaveCreate(e: DragEvent) {
    e.preventDefault();
    e.stopPropagation();
    createDragDepth = Math.max(0, createDragDepth - 1);
    if (createDragDepth === 0) {
      createIsDragOver = false;
      window.dispatchEvent(new CustomEvent('app:set-module-drag-active', { detail: false }));
    }
  }
  function handleDropCreate(e: DragEvent) {
    e.preventDefault();
    e.stopPropagation();
    activeDropSection = 'create';
    isDragOver = false;
    createIsDragOver = false;
    createDragDepth = 0;
    window.dispatchEvent(new CustomEvent('app:set-module-drag-active', { detail: false }));
    const dt = e.dataTransfer;
    if (!dt) return;
    // Try to get files from dataTransfer (fallback for when Tauri events don't fire)
    if (dt.files && dt.files.length > 0) {
      const first = dt.files[0];
      const path = (first as any)?.path || first.name;
      if (typeof path === 'string') applyDroppedPathCreate(path);
      return;
    }
    const raw = extractWorkbenchText(dt);
    if (!raw) return;
    let parsed = parseWorkbenchDrag(raw);
    if (!parsed && raw.includes('\n')) parsed = parseWorkbenchDrag(raw.split('\n')[0]);
    if (!parsed) return;
    applyDroppedPathCreate(parsed);
  }

  async function importFromClipboard() {
    try {
      const text = await navigator.clipboard.readText();
      if (!text) return;
      let parsed = parseWorkbenchDrag(text);
      if (!parsed && text.includes('\n')) parsed = parseWorkbenchDrag(text.split('\n')[0]);
      if (!parsed) return;
      if (activeDropSection === 'create') await applyDroppedPathCreate(parsed);
      else await applyDroppedPath(parsed);
    } catch (err) {
      console.error('Clipboard import failed', err);
    }
  }

  type SuggestFoldersResult = {
    sockets: number;
    matched: number;
    unmatched: number;
    suggested_extra_dirs?: string[];
  };

  async function refreshSuggestedFolders() {
    if (!fillSelectedFilePath) {
      suggestedPrefabFolders = [];
      suggestSummary = '';
      return;
    }
    suggestingFolders = true;
    suggestSummary = 'Detecting folders...';
    try {
      const res = await invoke<SuggestFoldersResult>('suggest_prefab_folders_from_xob', {
        xobPath: fillSelectedFilePath,
        svnRoot: svnFolderPath || null,
        extraDirs: customPrefabFolders
      });
      const arr = Array.isArray(res?.suggested_extra_dirs)
        ? res.suggested_extra_dirs.filter((x) => typeof x === 'string' && x)
        : [];
      suggestedPrefabFolders = arr;
      suggestSummary = `Matched ${res?.matched ?? 0}/${res?.sockets ?? 0} sockets`;
      if (autoAddDetectedFolders && arr.length) {
        suppressSuggestRefresh = true;
        for (const p of arr) {
          addSuggestedFolder(p);
        }
        // Update key after merging to avoid immediate re-trigger
        lastSuggestKey = `${fillSelectedFilePath}|${svnFolderPath}|${customPrefabFolders.join(';')}`;
        suppressSuggestRefresh = false;
      }
    } catch (err) {
      console.error('Suggest folders failed', err);
      suggestedPrefabFolders = [];
      suggestSummary = 'Detect failed';
    }
    suggestingFolders = false;
  }

  function addAllSuggestedFolders() {
    if (!suggestedPrefabFolders.length) return;
    for (const p of suggestedPrefabFolders) {
      addSuggestedFolder(p);
    }
  }

  function addSuggestedFolder(folder: string) {
    const f = String(folder || '').trim();
    if (!f) return;
    if (customPrefabFolders.includes(f)) return;
    const merged = [...customPrefabFolders, f];
    customPrefabFolders = merged;
    if (activeCustomFolderIdx == null && merged.length) activeCustomFolderIdx = merged.length - 1;
    invoke('remember_extra_dirs', { extraDirs: merged }).catch(() => {});
  }

  function clearLogs() {
    logs = [];
  }

  function formatLogs() {
    return logs.map(entry => `[${entry.type.toUpperCase()}] ${entry.message}`).join('\n');
  }

  async function copyLogs() {
    if (!logs.length) return;
    try {
      await navigator.clipboard.writeText(formatLogs());
    } catch (err) {
      console.error('Clipboard copy failed', err);
    }
  }

  function exportLogs() {
    if (!logs.length) return;
    exportLogsToFile();
  }

  async function exportLogsToFile() {
    if (!logs.length) return;
    try {
      const ts = new Date().toISOString().replace(/[:.]/g, '-');
      const path = await save({
        defaultPath: `socket-log-${ts}.txt`,
        filters: [{ name: 'Text', extensions: ['txt'] }]
      });
      if (typeof path === 'string' && path) {
        await invoke('write_text_file', { path, content: formatLogs() });
      }
    } catch (err) {
      console.error('Export failed', err);
    }
  }

  let logSeq = 0;
  function appendLog(type: 'info' | 'warn' | 'error', message: string) {
    logSeq += 1;
    const id = `${Date.now()}-${logSeq}`;
    const next = [...logs, { id, type, message }];
    logs = next.length > 600 ? next.slice(next.length - 600) : next;
  }
  const fillSteps = [
    { id: 'select', title: 'Select .xob file', subtitle: 'Choose or drag the model you want to fill sockets into.' },
    { id: 'svn', title: 'Set Folder SVN', subtitle: 'Set SVN folder for scan asset socket' },
    { id: 'review', title: 'Set Prefab', subtitle: 'Set save directory and add folder addon' },
    { id: 'process', title: 'Process', subtitle: 'waiting to process' },
    { id: 'done', title: 'Complete', subtitle: 'All set!' }
  ];
  let currentFillStep = 0;
  let fillSelectedFilePath = '';
  let fillSelectedFileName = '';
  let createSelectedFilePath = '';
  let createSelectedFileName = '';
  let fillDropEl: HTMLElement | null = null;
  let createDropEl: HTMLElement | null = null;
  let isDragOver = false;
  let fillIsDragOver = false;
  let createIsDragOver = false;
  let lastMouseX = 0;
  let lastMouseY = 0;
  let unlistenMouseMove: (() => void) | null = null;
  let arma4Root = '';
  try {
    arma4Root = localStorage.getItem('arma4Root') ?? '';
  } catch {}

  let unlistenWorkbenchPath: (() => void) | null = null;
  let unlistenFileDrop: (() => void) | null = null;
  let unlistenWebviewDrop: (() => void) | null = null;
  const totalFillSteps = fillSteps.length;
  $: progressPercent = totalFillSteps > 1 ? (currentFillStep / (totalFillSteps - 1)) * 100 : 0;
  let svnFolderPath = '';
  let autoDetecting = false;
  let autoDetectError = false;
  let scanStatus: 'idle' | 'scanning' | 'success' | 'error' = 'idle';
  let scanMessage = 'Waiting to scan';
  type ScanLogPayload = {
    level: string;
    message: string;
    current?: number | null;
    total?: number | null;
  };
  let scanPopupOpen = false;
  let scanPopupBusy = false;
  let scanPopupTitle = 'Scanning...';
  let scanPopupSub = '';
  let scanPopupCurrent: number | null = null;
  let scanPopupTotal: number | null = null;
  let scanPopupLogs: string[] = [];
  let scanOwlImgOk = true;
  let unlistenPrefabScan: (() => void) | null = null;
  let suppressAutoSkipStep2 = false;
  $: scanPopupPct =
    scanPopupTotal && scanPopupTotal > 0 && scanPopupCurrent != null
      ? Math.max(0, Math.min(100, Math.round((scanPopupCurrent * 100) / scanPopupTotal)))
      : null;
  type PrefabCacheStatus = {
    has_cache: boolean;
    cache_path?: string | null;
    svn_root?: string | null;
    generated?: string | null;
    prefab_count?: number;
  };
  type PrefabScanResult = {
    total_entries: number;
    cache_path: string;
  };
  let cacheStatus: PrefabCacheStatus | null = null;
  type AutoSocketSettings = {
    svnRoot?: string | null;
    saveDir?: string | null;
    extraDirs?: string[] | null;
    blenderPath?: string | null;
    svn_root?: string | null;
    save_dir?: string | null;
    extra_dirs?: string[] | null;
    blender_path?: string | null;
  };

  type AutoSocketPreset = {
    name: string;
    save_dir?: string | null;
    extra_dirs?: string[] | null;
    blender_path?: string | null;
  };
  type CreateEtResult = {
    et_path: string;
    meta_path?: string | null;
    sockets: number;
    matched: number;
    unmatched: number;
    suggested_extra_dirs?: string[];
  };
  let saveLocation = '';
  let blenderPath = '';
  let presets: AutoSocketPreset[] = [];
  let selectedPresetName = '';
  let newPresetName = '';
  let detectedPrefabFolders: string[] = [];
  let suggestedPrefabFolders: string[] = [];
  let suggestingFolders = false;
  let suggestSummary = '';
  let lastSuggestKey = '';
  let autoAddDetectedFolders = true;
  let suppressSuggestRefresh = false;
  let showDetectedFolders = false;
  let customPrefabFolders: string[] = [];
  let activeCustomFolderIdx: number | null = null;
  let createdEtPath = '';
  let createdEtStats: CreateEtResult | null = null;
  $: hasSelectedXob = !!fillSelectedFilePath;
  $: selectedXobName = fillSelectedFileName;
  $: createHasSelectedXob = !!(createPrefabQueue.length || createSelectedFilePath);
  $: hasPrefabCache = !!cacheStatus?.has_cache;
  $: nextDisabled =
    (currentFillStep === 0 && !hasSelectedXob) ||
    (currentFillStep === 1 && scanStatus !== 'success' && !hasPrefabCache);
  $: currentStepId = fillSteps[currentFillStep]?.id;
  $: primaryLabel = currentStepId === 'review' ? 'Process' : currentStepId === 'done' ? 'Finish' : 'Next';
  $: primaryDisabled =
    (currentStepId === 'process') ||
    (currentStepId !== 'process' && currentStepId !== 'done' && nextDisabled);

  $: if (currentFillStep === 1 && hasSelectedXob && hasPrefabCache && !suppressAutoSkipStep2) {
    currentFillStep = Math.min(fillSteps.length - 1, 2);
  }

  function applySvnFolder(path: string) {
    svnFolderPath = path;
    scanStatus = 'idle';
    scanMessage = 'Waiting to scan';
    autoDetectError = false;
    autoDetecting = false;
    try {
      localStorage.setItem('svnFolderPath', path);
    } catch {}
    invoke('remember_svn_root', { path }).catch(() => {});
    updateDetectedFolders(path);
  }

  function updateDetectedFolders(path?: string) {
    if (!path) {
      detectedPrefabFolders = [];
      return;
    }
    const normalized = path.replace(/[\\/]+$/, '');
    detectedPrefabFolders = [
      `${normalized}\\Prefabs`,
      `${normalized}\\Prefabs\\Custom`,
      `${normalized}\\Output`
    ];
  }

  async function pickXobViaDialog() {
    try {
      const selection = await open({
        multiple: false,
        filters: [{ name: 'XOB files', extensions: ['xob'] }]
      });
      if (typeof selection === 'string' && selection) {
        const prev = fillSelectedFilePath;
        fillSelectedFilePath = selection;
        fillSelectedFileName = selection.split(/[\\/]/).pop() ?? selection;
        if (prev !== selection) {
          const parent = selection.split(/[\\/]/).slice(0, -1).join('\\');
          if (parent) {
            saveLocation = parent;
            invoke('remember_save_dir', { path: saveLocation }).catch(() => {});
          }
        }
      }
    } catch (err) {
      console.error('XOB selection failed', err);
    }
  }

  async function pickXobViaDialogMulti(): Promise<string[]> {
    try {
      const selection = await open({
        multiple: true,
        filters: [{ name: 'XOB files', extensions: ['xob'] }]
      });
      if (Array.isArray(selection)) {
        return selection.filter((x) => typeof x === 'string' && x) as string[];
      }
      if (typeof selection === 'string' && selection) {
        return [selection];
      }
    } catch (err) {
      console.error('XOB selection failed', err);
    }
    return [];
  }

  async function autoDetectSvnFolder() {
    if (autoDetecting) return;
    autoDetecting = true;
    autoDetectError = false;
    scanMessage = 'Detecting SVN folder...';
    try {
      const detected = await invoke<string | null>('auto_detect_svn_root');
      if (detected) {
        applySvnFolder(detected);
        scanMessage = 'Detected SVN folder automatically';
      } else {
        autoDetectError = true;
        scanMessage = 'Unable to detect SVN folder, please select manually';
      }
    } catch (err) {
      console.error('Auto-detect failed', err);
      autoDetectError = true;
      scanMessage = 'Auto-detect failed, please choose folder manually';
    } finally {
      autoDetecting = false;
    }
  }

  async function manualPickSvnFolder() {
    try {
      const selection = await open({ directory: true, multiple: false });
      if (typeof selection === 'string' && selection) {
        applySvnFolder(selection);
        await refreshCacheStatus(true);
      }
    } catch (err) {
      console.error('Folder selection failed', err);
    }
  }

  function openScanPopup() {
    scanPopupOpen = true;
    scanPopupBusy = true;
    scanPopupTitle = 'Scanning SVN...';
    scanPopupSub = '';
    scanPopupCurrent = null;
    scanPopupTotal = null;
    scanPopupLogs = [];
    scanOwlImgOk = true;
  }

  function closeScanPopup() {
    if (scanPopupBusy) return;
    scanPopupOpen = false;
  }

  function pushScanLog(line: string) {
    const next = [...scanPopupLogs, line];
    scanPopupLogs = next.length > 600 ? next.slice(next.length - 600) : next;
  }

  async function triggerScan() {
    if (!svnFolderPath || scanStatus === 'scanning') return;
    openScanPopup();
    scanStatus = 'scanning';
    scanMessage = 'Scanning SVN for prefabs...';
    try {
      const result = await invoke<PrefabScanResult>('scan_prefab_index', { svnRoot: svnFolderPath, verbose: true });
      scanStatus = 'success';
      scanMessage = `Indexed ${result?.total_entries ?? 0} prefabs`;
      await refreshCacheStatus(false);
      scanPopupTitle = 'Scan complete';
      scanPopupSub = scanMessage;
      scanPopupBusy = false;
      setTimeout(() => {
        if (scanPopupOpen && scanStatus === 'success' && !scanPopupBusy) {
          scanPopupOpen = false;
        }
      }, 700);
      return;
    } catch (err) {
      console.error('Scan failed', err);
      scanStatus = 'error';
      let detail = '';
      if (typeof err === 'string') detail = err;
      else if (err instanceof Error) detail = err.message;
      else {
        try {
          detail = JSON.stringify(err);
        } catch {
          detail = String(err);
        }
      }
      scanMessage = detail || 'Scan failed. Please retry.';
      pushScanLog(`error: ${scanMessage}`);
      scanPopupTitle = 'Scan failed';
      scanPopupSub = scanMessage;
    }
    scanPopupBusy = false;
  }

  $: if (currentFillStep === 1 && !svnFolderPath && !autoDetecting && !autoDetectError) {
    autoDetectSvnFolder();
  }

  function skipToPrefab() {
    autoDetectError = false;
    currentFillStep = Math.min(fillSteps.length - 1, 2);
  }

  let processingProgress = 0;
  let processRunning = false;
  let processSub = '';
  let processCurrent: number | null = null;
  let processTotal: number | null = null;
  $: processPct =
    processTotal && processTotal > 0 && processCurrent != null
      ? Math.max(0, Math.min(100, Math.round((processCurrent * 100) / processTotal)))
      : null;

  function finishFlow() {
    processingProgress = 0;
    processPct = null;
    processSub = '';
    processCurrent = null;
    processTotal = null;
    currentFillStep = 0;
    suppressAutoSkipStep2 = false;
  }

  $: if (currentStepId === 'review' && fillSelectedFilePath && !suggestingFolders) {
    const key = `${fillSelectedFilePath}|${svnFolderPath}|${customPrefabFolders.join(';')}`;
    if (!suppressSuggestRefresh && key !== lastSuggestKey) {
      lastSuggestKey = key;
      refreshSuggestedFolders();
    }
  }

  async function refreshCacheStatus(fromManualSelection = false) {
    try {
      const status = await invoke<PrefabCacheStatus>('get_prefab_cache_status');
      cacheStatus = status;
      if (!fromManualSelection && status?.svn_root) {
        applySvnFolder(status.svn_root);
      }
      if (status?.has_cache) {
        scanStatus = 'success';
        const count = status.prefab_count ?? 0;
        const timeLabel = status.generated ? ` | ${status.generated}` : '';
        scanMessage = `Cached ${count} prefabs${timeLabel}`;
      } else if (!fromManualSelection) {
        scanStatus = 'idle';
        scanMessage = 'Please scan your SVN folder to build prefab index';
      }
    } catch (err) {
      console.error('Failed to load prefab cache status', err);
    }
  }

  onMount(() => {
    const onMouseMove = (e: MouseEvent) => {
      lastMouseX = e.clientX;
      lastMouseY = e.clientY;
    };
    window.addEventListener('mousemove', onMouseMove, { capture: true } as any);
    unlistenMouseMove = () => window.removeEventListener('mousemove', onMouseMove, { capture: true } as any);

    // Enable the global drop overlay while this module is mounted.
    try {
      window.dispatchEvent(new CustomEvent('app:set-allow-drop', { detail: true }));
    } catch {}

    // Accept resolved drops produced by +layout.svelte overlay
    const onWorkbench = (e: any) => {
      try {
        const detail = e?.detail;
        if (typeof detail === 'string' && detail) {
          if (activeDropSection === 'create') applyDroppedPathCreate(detail);
          else applyDroppedPath(detail);
        }
      } catch {}
    };
    window.addEventListener('workbench-drop-path', onWorkbench as any);
    unlistenWorkbenchPath = () => window.removeEventListener('workbench-drop-path', onWorkbench as any);

    // Progress/log stream used by scan + process UI
    listen<ScanLogPayload>('prefab_scan_log', (event) => {
      const p = event?.payload;
      if (!p) return;
      const lvl = String(p.level || 'info').toLowerCase();
      const msg = String(p.message || '');
      const line = `${lvl}: ${msg}`;

      const mapped: 'info' | 'warn' | 'error' =
        lvl === 'error' ? 'error' : lvl === 'warn' || lvl === 'warning' ? 'warn' : 'info';
      appendLog(mapped, `[Scan] ${line}`);

      if (scanPopupOpen) {
        pushScanLog(line);
        scanPopupCurrent = typeof p.current === 'number' ? p.current : null;
        scanPopupTotal = typeof p.total === 'number' ? p.total : null;
        scanPopupSub = msg;
      }

      if (processRunning) {
        processSub = msg;
        processCurrent = typeof p.current === 'number' ? p.current : null;
        processTotal = typeof p.total === 'number' ? p.total : null;
        if (processTotal && processTotal > 0 && processCurrent != null) {
          processingProgress = Math.max(0, Math.min(100, Math.round((processCurrent * 100) / processTotal)));
        }
      }
    }).then((u) => {
      unlistenPrefabScan = u;
    });

    // Also accept native file drops (Explorer -> app).
    listen<string[] | string>('tauri://file-drop', (e) => {
      const payload = e.payload as any;
      const paths = Array.isArray(payload) ? payload : [payload];
      const targetSection = hoverDropSection ?? activeDropSection;
      for (const p of paths) {
        if (typeof p === 'string' && p) {
          if (targetSection === 'create') applyDroppedPathCreate(p);
          else applyDroppedPath(p);
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
          // Some Tauri/Webview drag events report screen coords; map to client coords heuristically.
          if (x > window.innerWidth * 2 || y > window.innerHeight * 2) {
            x = x - window.screenX;
            y = y - window.screenY;
          }
          x = Math.max(0, Math.min(window.innerWidth - 1, x));
          y = Math.max(0, Math.min(window.innerHeight - 1, y));

          const fillRect = fillDropEl?.getBoundingClientRect();
          const createRect = createDropEl?.getBoundingClientRect();
          const inRect = (r: DOMRect | undefined) => !!(r && x >= r.left && x <= r.right && y >= r.top && y <= r.bottom);

          let overFill = inRect(fillRect);
          let overCreate = inRect(createRect);

          // If pointer is between/near zones but not strictly inside, pick the closest zone by vertical distance.
          if (!overFill && !overCreate && fillRect && createRect) {
            const fy = (fillRect.top + fillRect.bottom) / 2;
            const cy = (createRect.top + createRect.bottom) / 2;
            if (Math.abs(y - fy) <= Math.abs(y - cy)) overFill = true;
            else overCreate = true;
          }

          fillIsDragOver = overFill;
          createIsDragOver = overCreate;
          if (overFill && !overCreate) {
            hoverDropSection = 'fill';
            activeDropSection = 'fill';
          } else if (overCreate && !overFill) {
            hoverDropSection = 'create';
            activeDropSection = 'create';
          } else {
            hoverDropSection = null;
          }
          window.dispatchEvent(new CustomEvent('app:set-module-drag-active', { detail: overFill || overCreate }));
        };

        if (t === 'hover' || t === 'over' || t === 'enter') {
          updateHover();
        }
        if (t === 'leave' || t === 'cancelled') {
          fillIsDragOver = false;
          createIsDragOver = false;
          hoverDropSection = null;
          window.dispatchEvent(new CustomEvent('app:set-module-drag-active', { detail: false }));
        }
        if (t === 'drop') {
          fillIsDragOver = false;
          createIsDragOver = false;
          const targetSection = hoverDropSection ?? activeDropSection;
          hoverDropSection = null;
          window.dispatchEvent(new CustomEvent('app:set-module-drag-active', { detail: false }));
          const pathsAny = ev?.payload?.paths ?? ev?.paths ?? ev?.payload;
          const paths = Array.isArray(pathsAny) ? pathsAny : [pathsAny];
          for (const p of paths) {
            if (typeof p === 'string' && p) {
              if (targetSection === 'create') applyDroppedPathCreate(p);
              else applyDroppedPath(p);
            }
          }
        }
      }).then((u) => {
        unlistenWebviewDrop = u;
      });
    } catch {}

    refreshCacheStatus(false).then(() => {
      if (!svnFolderPath) {
        autoDetectSvnFolder();
      }
    });
    loadAutosocketSettings();
    refreshPresetList();
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
    if (unlistenPrefabScan) {
      try {
        unlistenPrefabScan();
      } catch {}
      unlistenPrefabScan = null;
    }
  });

  function handlePresetChange(event: Event) {
    const target = event.target as HTMLSelectElement;
    selectedPresetName = target.value;
    applyPresetByName(selectedPresetName);
  }

  async function refreshPresetList(selectName?: string) {
    try {
      const list = await invoke<AutoSocketPreset[]>('get_autosocket_presets');
      presets = Array.isArray(list) ? list.filter((p) => p && typeof p.name === 'string' && p.name.trim()) : [];
      if (selectName) {
        selectedPresetName = selectName;
      } else if (selectedPresetName && presets.some((p) => p.name === selectedPresetName)) {
        // keep
      } else {
        selectedPresetName = presets[0]?.name ?? '';
      }
    } catch (err) {
      console.error('Failed to load preset list', err);
      presets = [];
      selectedPresetName = '';
    }
  }

  async function persistCurrentPresetRelatedSettings() {
    invoke('remember_save_dir', { path: saveLocation || null }).catch(() => {});
    invoke('remember_blender_path', { path: blenderPath || null }).catch(() => {});
    invoke('remember_extra_dirs', { extraDirs: customPrefabFolders }).catch(() => {});
  }

  async function applyPreset(p: AutoSocketPreset) {
    saveLocation = (p?.save_dir ?? '') || '';
    blenderPath = (p?.blender_path ?? '') || '';
    const extra = p?.extra_dirs;
    if (Array.isArray(extra)) {
      customPrefabFolders = extra.filter((x) => typeof x === 'string' && x);
      activeCustomFolderIdx = customPrefabFolders.length ? 0 : null;
    } else {
      customPrefabFolders = [];
      activeCustomFolderIdx = null;
    }
    await persistCurrentPresetRelatedSettings();
  }

  async function applyPresetByName(name: string) {
    const n = String(name || '').trim();
    if (!n) return;
    const p = presets.find((x) => x.name === n);
    if (!p) return;
    await applyPreset(p);
  }

  async function handleImportPreset() {
    try {
      const selection = await open({ multiple: false, filters: [{ name: 'Preset', extensions: ['json'] }] });
      if (typeof selection !== 'string' || !selection) return;
      const text = await invoke<string>('read_text_file', { path: selection });
      const parsed = JSON.parse(text);
      const name = String(parsed?.name ?? parsed?.preset_name ?? '').trim();
      if (!name) return;
      const preset: AutoSocketPreset = {
        name,
        save_dir: typeof parsed?.saveLocation === 'string' ? parsed.saveLocation : parsed?.save_dir ?? null,
        blender_path: typeof parsed?.blenderPath === 'string' ? parsed.blenderPath : parsed?.blender_path ?? null,
        extra_dirs: Array.isArray(parsed?.customPrefabFolders) ? parsed.customPrefabFolders : parsed?.extra_dirs ?? null
      };
      const merged = [preset, ...presets.filter((p) => p.name !== preset.name)];
      await invoke('save_autosocket_presets', { presets: merged });
      await refreshPresetList(preset.name);
      await applyPreset(preset);
    } catch (err) {
      console.error('Preset import failed', err);
    }
  }

  async function handleExportPreset() {
    try {
      const p = presets.find((x) => x.name === selectedPresetName);
      if (!p) return;
      const selection = await save({
        defaultPath: `${p.name}.json`,
        filters: [{ name: 'Preset', extensions: ['json'] }]
      });
      if (typeof selection !== 'string' || !selection) return;
      const payload = {
        name: p.name,
        saveLocation: p.save_dir ?? '',
        blenderPath: p.blender_path ?? '',
        customPrefabFolders: p.extra_dirs ?? []
      };
      await invoke('write_text_file', { path: selection, content: JSON.stringify(payload, null, 2) });
    } catch (err) {
      console.error('Preset export failed', err);
    }
  }

  async function saveCurrentAsPreset() {
    const name = newPresetName.trim();
    if (!name) return;
    const preset: AutoSocketPreset = {
      name,
      save_dir: saveLocation || null,
      blender_path: blenderPath || null,
      extra_dirs: customPrefabFolders
    };
    try {
      const merged = [preset, ...presets.filter((p) => p.name !== preset.name)];
      await invoke('save_autosocket_presets', { presets: merged });
      newPresetName = '';
      await refreshPresetList(preset.name);
    } catch (err) {
      console.error('Save preset failed', err);
    }
  }

  async function pickSaveLocation() {
    try {
      const selection = await open({ directory: true, multiple: false });
      if (typeof selection === 'string' && selection) {
        saveLocation = selection;
        invoke('remember_save_dir', { path: saveLocation }).catch(() => {});
      }
    } catch (err) {
      console.error('Save selection failed', err);
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

  async function addCustomPrefabFolder() {
    try {
      const selection = await open({ directory: true, multiple: false });
      if (typeof selection === 'string' && selection) {
        if (!customPrefabFolders.includes(selection)) {
          const next = [...customPrefabFolders, selection];
          customPrefabFolders = next;
          activeCustomFolderIdx = next.length - 1;
          invoke('remember_extra_dirs', { extraDirs: customPrefabFolders }).catch(() => {});
        }
      }
    } catch (err) {
      console.error('Custom folder selection failed', err);
    }
  }

  function handleSelectCustomFolder(idx: number) {
    activeCustomFolderIdx = idx;
  }

  function removeSelectedPrefabFolder() {
    if (activeCustomFolderIdx === null) return;
    const next = customPrefabFolders.filter((_, i) => i !== activeCustomFolderIdx);
    customPrefabFolders = next;
    activeCustomFolderIdx = next.length ? Math.min(activeCustomFolderIdx, next.length - 1) : null;
    invoke('remember_extra_dirs', { extraDirs: customPrefabFolders }).catch(() => {});
  }

  async function loadAutosocketSettings() {
    try {
      const s = await invoke<AutoSocketSettings>('get_autosocket_settings');
      const saveDir = (s?.saveDir ?? s?.save_dir) as any;
      if (saveDir && typeof saveDir === 'string') {
        saveLocation = saveDir;
      }
      const extraDirs = (s?.extraDirs ?? s?.extra_dirs) as any;
      if (Array.isArray(extraDirs)) {
        customPrefabFolders = extraDirs.filter((x) => typeof x === 'string' && x);
        activeCustomFolderIdx = customPrefabFolders.length ? 0 : null;
      }
      const svnRoot = (s?.svnRoot ?? s?.svn_root) as any;
      if (svnRoot && typeof svnRoot === 'string' && !svnFolderPath) {
        applySvnFolder(svnRoot);
      }
      const bp = (s?.blenderPath ?? s?.blender_path) as any;
      if (bp && typeof bp === 'string') {
        blenderPath = bp;
      }
    } catch (err) {
      console.error('Failed to load autosocket settings', err);
    }
  }

  async function processCreateEt() {
    if (!fillSelectedFilePath) {
      appendLog('warn', '[Process] No .xob selected');
      return;
    }
    if (!saveLocation) {
      appendLog('warn', '[Process] Please select save folder');
      return;
    }
    const processIndex = fillSteps.findIndex((s) => s.id === 'process');
    if (processIndex >= 0) currentFillStep = processIndex;
    processRunning = true;
    processSub = 'Creating .et from sockets';
    processCurrent = null;
    processTotal = null;
    processingProgress = 0;
    try {
      const res = await invoke<CreateEtResult>('create_new_et_from_xob', {
        xobPath: fillSelectedFilePath,
        saveDir: saveLocation,
        svnRoot: svnFolderPath || null,
        extraDirs: customPrefabFolders
      });
      const suggested = Array.isArray(res?.suggested_extra_dirs) ? res.suggested_extra_dirs.filter((x) => typeof x === 'string' && x) : [];
      if (suggested.length) {
        const merged = [...customPrefabFolders];
        for (const p of suggested) {
          if (!merged.includes(p)) merged.push(p);
        }
        customPrefabFolders = merged;
        if (activeCustomFolderIdx == null && merged.length) activeCustomFolderIdx = 0;
        invoke('remember_extra_dirs', { extraDirs: merged }).catch(() => {});
        appendLog('info', `[Process] Auto-assigned prefab folders: ${suggested.length}`);
      }
      createdEtPath = res?.et_path || '';
      createdEtStats = res;
      appendLog('info', `[Process] Saved .et: ${createdEtPath}`);
      currentFillStep = fillSteps.findIndex((s) => s.id === 'done');
    } catch (err) {
      console.error('Process failed', err);
      let detail = '';
      if (typeof err === 'string') detail = err;
      else if (err instanceof Error) detail = err.message;
      else {
        try {
          detail = JSON.stringify(err);
        } catch {
          detail = String(err);
        }
      }
      const msg = detail || 'Process failed';
      appendLog('error', `[Process] ${msg}`);
    }
    processRunning = false;
  }

  let createdPrefabEtPath = '';
  let createdPrefabMetaPath = '';
  let createdPrefabStats: CreateEtResult | null = null;

  type CreatePrefabItemResult = {
    xob_path: string;
    et_path: string;
    meta_path?: string | null;
    sockets: number;
    matched: number;
    unmatched: number;
  };

  let createPrefabQueue: string[] = [];
  let createPrefabResults: CreatePrefabItemResult[] = [];

  async function chooseCreatePrefabFiles() {
    const files = await pickXobViaDialogMulti();
    if (!files.length) return;
    createPrefabQueue = files;
    createPrefabResults = [];

    // Keep a reasonable UX: show first file in Create section
    const first = files[0];
    if (first) {
      createSelectedFilePath = first;
      createSelectedFileName = first.split(/[\\/]/).pop() ?? first;
    }
  }

  async function processCreatePrefabEt() {
    const queue = createPrefabQueue.length ? [...createPrefabQueue] : (createSelectedFilePath ? [createSelectedFilePath] : []);
    if (!queue.length) {
      appendLog('warn', '[Create Prefab] No .xob selected');
      return;
    }
    if (!saveLocation) {
      appendLog('warn', '[Create Prefab] Please select save folder');
      return;
    }
    processRunning = true;
    processSub = 'Creating .et from sockets';
    processCurrent = 0;
    processTotal = queue.length;
    processingProgress = 0;
    createdPrefabEtPath = '';
    createdPrefabMetaPath = '';
    createdPrefabStats = null;
    createPrefabResults = [];
    try {
      for (let i = 0; i < queue.length; i++) {
        const xob = queue[i];
        processCurrent = i;
        processSub = `Creating ${i + 1}/${queue.length}: ${xob.split(/[\\/]/).pop() ?? xob}`;
        if (processTotal && processTotal > 0) {
          processingProgress = Math.max(0, Math.min(100, Math.round(((i + 1) * 100) / processTotal)));
        }

        // Keep Create-selected file in sync for UI context
        createSelectedFilePath = xob;
        createSelectedFileName = xob.split(/[\\/]/).pop() ?? xob;

        const res = await invoke<CreateEtResult>('create_new_et_with_meta_from_xob', {
          xobPath: xob,
          saveDir: saveLocation,
          svnRoot: svnFolderPath || null,
          extraDirs: customPrefabFolders
        });

        const suggested = Array.isArray(res?.suggested_extra_dirs)
          ? res.suggested_extra_dirs.filter((x) => typeof x === 'string' && x)
          : [];
        if (suggested.length) {
          const merged = [...customPrefabFolders];
          for (const p of suggested) {
            if (!merged.includes(p)) merged.push(p);
          }
          customPrefabFolders = merged;
          if (activeCustomFolderIdx == null && merged.length) activeCustomFolderIdx = 0;
          invoke('remember_extra_dirs', { extraDirs: merged }).catch(() => {});
          appendLog('info', `[Create Prefab] Auto-assigned prefab folders: ${suggested.length}`);
        }

        createdPrefabEtPath = res?.et_path || '';
        createdPrefabMetaPath = (res as any)?.meta_path || '';
        createdPrefabStats = res;

        const nextItem: CreatePrefabItemResult = {
          xob_path: xob,
          et_path: createdPrefabEtPath,
          meta_path: (res as any)?.meta_path ?? null,
          sockets: res?.sockets ?? 0,
          matched: res?.matched ?? 0,
          unmatched: res?.unmatched ?? 0
        };
        createPrefabResults = [...createPrefabResults, nextItem];

        if (createdPrefabEtPath) {
          appendLog('info', `[Create Prefab] Saved .et: ${createdPrefabEtPath}`);
        }
        if (createdPrefabMetaPath) {
          appendLog('info', `[Create Prefab] Saved .et.meta: ${createdPrefabMetaPath}`);
        }
      }

      processCurrent = queue.length;
    } catch (err) {
      console.error('Create prefab failed', err);
      let detail = '';
      if (typeof err === 'string') detail = err;
      else if (err instanceof Error) detail = err.message;
      else {
        try {
          detail = JSON.stringify(err);
        } catch {
          detail = String(err);
        }
      }
      const msg = detail || 'Create prefab failed';
      appendLog('error', `[Create Prefab] ${msg}`);
    }
    processRunning = false;
  }

  async function nextFillStep() {
    if (currentStepId === 'review') {
      await processCreateEt();
      return;
    }
    if (currentStepId === 'process') {
      return;
    }
    if (currentStepId === 'done') {
      finishFlow();
      return;
    }
    if (nextDisabled) return;
    if (currentFillStep === 1) {
      suppressAutoSkipStep2 = false;
    }
    currentFillStep = Math.min(fillSteps.length - 1, currentFillStep + 1);
  }

  function prevFillStep() {
    if (currentFillStep === 2) {
      suppressAutoSkipStep2 = true;
    }
    currentFillStep = Math.max(0, currentFillStep - 1);
  }
</script>

<div class="socket-manager">
  <h2>Socket Manager</h2>

  <section class="section">
    <button class="section-header" on:click={() => toggle('fill')}>
      <span>Fill socket to model</span>
      <span class="chevron" aria-hidden="true">{fillExpanded ? '▾' : '▸'}</span>
    </button>
    {#if fillExpanded}
      <div class="section-body">
        <div class="stepper-header">
          <div class="progress-bar">
            <div class="progress-fill" style={`width:${progressPercent}%`}></div>
          </div>
          <div class="bubble-row">
            {#each fillSteps as step, index}
              <div class="bubble-group">
                <div class={`bubble ${index < currentFillStep ? 'complete' : ''} ${index === currentFillStep ? 'active' : ''}`}>
                  {index < currentFillStep ? '✓' : index + 1}
                </div>
                <p class="bubble-label">{step.title}</p>
                {#if index === currentFillStep}
                  <p class="bubble-subtitle">{step.subtitle}</p>
                {/if}
              </div>
            {/each}
          </div>
        </div>

        <div class="step-panel">
          {#if currentStepId === 'select'}
            <div>
              <button
                type="button"
                class={`drop-zone ${hasSelectedXob ? 'selected' : ''} ${fillIsDragOver ? 'drag-active' : ''}`}
                aria-label="Select .xob file"
                on:click={pickXobViaDialog}
                on:dragover={handleDragOverFill}
                on:dragenter={handleDragEnterFill}
                on:dragleave={handleDragLeaveFill}
                on:drop={handleDropFill}
                bind:this={fillDropEl}
              >
                <div class="drop-label">
                  <strong>Select .xob file</strong>
                  <span>click to browse via system dialog</span>
                </div>
                {#if hasSelectedXob}
                  <p class="selected-file">Selected: {selectedXobName}</p>
                  {#if fillSelectedFilePath}
                    <p class="selected-path">{fillSelectedFilePath}</p>
                  {/if}
                {/if}
              </button>
              <div style="display:flex; justify-content:flex-end; margin-top: 10px;">
                <button
                  type="button"
                  class="ghost-btn small"
                  on:click={() => {
                    activeDropSection = 'fill';
                    importFromClipboard();
                  }}
                >
                  Import from Workbench (Clipboard)
                </button>
              </div>
            </div>
          {:else if currentStepId === 'svn'}
            <div class="svn-config">
              <div class="svn-row">
                <label for="svn-folder">Folder SVN</label>
                <div class="svn-field">
                  <input
                    id="svn-folder"
                    type="text"
                    placeholder="Auto-detected SVN path"
                    value={svnFolderPath}
                    readonly
                    title={svnFolderPath || 'No folder selected'}
                    class:error={autoDetectError}
                    class:auto-detecting={autoDetecting && !svnFolderPath && !autoDetectError}
                  />
                  <div class="tooltip">
                    <button class="icon-btn" class:attention={autoDetectError} on:click={manualPickSvnFolder} aria-label="Select folder">
                      📂
                    </button>
                    <span class="tooltip-text">Choose folder manually</span>
                  </div>
                </div>
                {#if autoDetectError}
                  <p class="auto-detect-error">can't find your SVN folder, please use Browse Folder Button</p>
                {/if}
              </div>

              <div class="scan-row">
                <button
                  class={`scan-btn ${scanStatus}`}
                  on:click={triggerScan}
                  disabled={!svnFolderPath || scanStatus === 'scanning'}
                >
                  <span>Scan</span>
                  <span class={`scan-dot ${scanStatus}`} aria-hidden="true"></span>
                </button>
                <p class="scan-message">
                  {scanMessage}
                  <button class="skip-btn" on:click={skipToPrefab}>Skip</button>
                </p>
              </div>
            </div>
          {:else if currentStepId === 'review'}
            <div class="prefab-config">
              <div class="file-summary">
                {#if fillSelectedFileName}
                  <p class="file-name">{fillSelectedFileName}</p>
                  <p class="file-path">{fillSelectedFilePath || 'Path unavailable in this environment'}</p>
                {:else}
                  <p class="file-placeholder">No .xob file selected yet</p>
                {/if}
              </div>

              <div class="form-row">
                <span class="row-title">Preset</span>
                <div class="preset-controls">
                  <select class="preset-select" bind:value={selectedPresetName} on:change={handlePresetChange}>
                    <option value="" disabled selected={!selectedPresetName}>Select preset</option>
                    {#each presets as p (p.name)}
                      <option value={p.name}>{p.name}</option>
                    {/each}
                  </select>
                  <input
                    class="preset-input"
                    type="text"
                    placeholder="New preset name"
                    bind:value={newPresetName}
                  />
                  <button type="button" class="ghost-btn small" on:click={saveCurrentAsPreset} disabled={!newPresetName.trim()}>
                    Save
                  </button>
                  <div class="preset-actions">
                    <button type="button" class="ghost-btn small" on:click={handleImportPreset}>Import</button>
                    <button type="button" class="ghost-btn small" on:click={handleExportPreset}>Export</button>
                  </div>
                </div>
              </div>

              <div class="form-row">
                <span class="row-title">Save as</span>
                <button class="save-picker" type="button" on:click={pickSaveLocation}>
                  {saveLocation || 'please click to select save location'}
                </button>
              </div>

              <div class="form-row">
                <span class="row-title">Blender Path</span>
                <button class="save-picker" type="button" on:click={pickBlenderPath}>
                  {blenderPath || 'please click to select blender.exe'}
                </button>
              </div>

              <div class="custom-prefab-section">
                <div class="section-header-row">
                  <div class="section-header-title">
                    <p class="section-title">Custom prefab folder</p>
                    <p class="section-subtitle">เพิ่มโฟลเดอร์ prefab เพิ่มเติมได้ที่นี่ หากตัว socket ไม่ได้อยู่ในโฟลเดอร์ SVN</p>
                  </div>
                </div>

                <div class="prefab-board">
                  <p class="column-title">Your folders</p>
                  <div class="custom-panel-body">
                    <div class="custom-list" role="listbox" aria-label="Custom prefab folders">
                      {#if customPrefabFolders.length}
                        {#each customPrefabFolders as folder, idx}
                          <button
                            type="button"
                            class={`custom-entry ${activeCustomFolderIdx === idx ? 'active' : ''}`}
                            on:click={() => handleSelectCustomFolder(idx)}
                          >
                            {folder}
                          </button>
                        {/each}
                      {:else}
                        <span class="placeholder small">Add folders you want to include</span>
                      {/if}
                    </div>
                    <div class="custom-actions">
                      <button class="circle-btn" type="button" on:click={addCustomPrefabFolder}>+</button>
                      <button class="circle-btn" type="button" on:click={removeSelectedPrefabFolder} disabled={activeCustomFolderIdx === null}>−</button>
                    </div>
                  </div>
                  <div style="display:flex; justify-content:flex-end; margin-top: 10px;">
                    <button type="button" class="ghost-btn small" on:click={() => (showDetectedFolders = !showDetectedFolders)}>
                      {showDetectedFolders ? 'Hide detected folders' : 'Show detected folders'}
                    </button>
                  </div>
                </div>

                {#if showDetectedFolders}
                  <div class="prefab-board" style="margin-top: 12px;">
                    <div class="section-header-row">
                      <p class="column-title">Detected folders</p>
                      <div class="preset-actions">
                        <label class="ghost-btn small" style="display:flex; align-items:center; gap:8px;">
                          <input type="checkbox" bind:checked={autoAddDetectedFolders} />
                          Auto add
                        </label>
                        <button type="button" class="ghost-btn small" on:click={refreshSuggestedFolders} disabled={suggestingFolders}>
                          Refresh
                        </button>
                        <button type="button" class="ghost-btn small" on:click={addAllSuggestedFolders} disabled={!suggestedPrefabFolders.length}>
                          Add all
                        </button>
                      </div>
                    </div>
                    {#if suggestSummary}
                      <p class="placeholder small" style="margin: 6px 0 10px;">{suggestSummary}</p>
                    {/if}
                    <div class="custom-list" role="list" aria-label="Detected prefab folders">
                      {#if suggestedPrefabFolders.length}
                        {#each suggestedPrefabFolders as folder}
                          <button
                            type="button"
                            class="custom-entry"
                            on:click={() => addSuggestedFolder(folder)}
                            title="Add to Your folders"
                          >
                            {folder}
                          </button>
                        {/each}
                      {:else}
                        <span class="placeholder small">No detected folders</span>
                      {/if}
                    </div>
                  </div>
                {/if}

              </div>
            </div>
          {:else if currentStepId === 'process'}
            <div class="processing-panel">
              <div class="process-card">
                <div class="process-main">
                  <div class="process-icon" aria-hidden="true">
                    <span>⬆</span>
                  </div>
                  <div class="process-text">
                    <p class="process-title">Processing prefabs…</p>
                    <p class="process-subtext">
                      {#if processPct != null}
                        <span class="accent">{processCurrent}</span>
                        <span class="divider">/</span>
                        <span>{processTotal}</span>
                      {:else}
                        <span>{processSub || 'Working...'}</span>
                      {/if}
                    </p>
                  </div>
                </div>
                <button type="button" class="process-action" disabled aria-label="Stop processing">
                  ✕
                </button>
              </div>

              <div class="process-track">
                <div
                  class="process-track-fill"
                  style={processPct != null ? `width:${processPct}%` : 'width:25%'}
                  class:indeterminate={processPct == null}
                ></div>
              </div>

              <div class="process-log">
                <p class="column-title">Log</p>
                {#if logs.length === 0}
                  <p class="placeholder small">No log entries yet</p>
                {:else}
                  <ul class="log-list compact">
                    {#each logs.slice(-6) as entry (entry.id)}
                      <li class={`log-entry ${entry.type}`}>
                        <span class="badge">{entry.type.toUpperCase()}</span>
                        <span>{entry.message}</span>
                      </li>
                    {/each}
                  </ul>
                {/if}
              </div>
            </div>
          {:else}
            <div class="completion-panel">
              <p class="completion-title">Finished fill socket</p>
              <p class="completion-text">please check file in your save directory</p>
              {#if createdEtPath}
                <p class="selected-path">{createdEtPath}</p>
              {/if}
              <button class="primary-btn slim" on:click={finishFlow}>Back to start</button>
            </div>
          {/if}
        </div>

        <div class="step-footer">
          <span class="step-count">Step {currentFillStep + 1} of {totalFillSteps}</span>
          <div class="step-actions">
            <button class="ghost-btn" on:click={prevFillStep} disabled={currentFillStep === 0}>Previous</button>
            <button class="primary-btn" on:click={nextFillStep} disabled={primaryDisabled}>
              {primaryLabel}
            </button>
          </div>
        </div>
      </div>
    {/if}
  </section>

  <section class="section">
    <button class="section-header" on:click={() => toggle('create')}>
      <span>Create socket</span>
      <span class="chevron" aria-hidden="true">{createExpanded ? '▾' : '▸'}</span>
    </button>
    {#if createExpanded}
      <div class="section-body">
        <div class="processing-panel">
          <div class="process-card">
            <div class="process-main">
              <div class="process-icon" aria-hidden="true">🧩</div>
              <div class="process-text">
                <p class="process-title">make xob to socket files</p>
                <p class="process-subtext">
                  <span>for testing assign custom socket</span>
                </p>
              </div>
            </div>
            <button class="primary-btn" on:click={processCreatePrefabEt} disabled={processRunning}>
              Create
            </button>
          </div>

          <div class="process-log">
            <div>
              <button
                type="button"
                class={`drop-zone ${createHasSelectedXob ? 'selected' : ''} ${createIsDragOver ? 'drag-active' : ''}`}
                aria-label="Select .xob file"
                on:click={chooseCreatePrefabFiles}
                on:dragover={handleDragOverCreate}
                on:dragenter={handleDragEnterCreate}
                on:dragleave={handleDragLeaveCreate}
                on:drop={handleDropCreate}
                bind:this={createDropEl}
              >
                <div class="drop-label">
                  <strong>Select .xob file(s)</strong>
                  <span>click to browse via system dialog (multi-select supported)</span>
                </div>
                {#if createPrefabQueue.length}
                  <p class="selected-file">Queued: {createPrefabQueue.length} file(s)</p>
                  <div class="queue-list">
                    {#each createPrefabQueue as p, i}
                      <p class="queue-item" title={p}>{i + 1}. {p.split(/[\\/]/).pop() ?? p}</p>
                    {/each}
                  </div>
                {:else if createSelectedFileName}
                  <p class="selected-file">Selected: {createSelectedFileName}</p>
                  {#if createSelectedFilePath}
                    <p class="selected-path">{createSelectedFilePath}</p>
                  {/if}
                {/if}
              </button>
              <div style="display:flex; justify-content:flex-end; margin-top: 10px;">
                <button
                  type="button"
                  class="ghost-btn small"
                  on:click={() => {
                    activeDropSection = 'create';
                    importFromClipboard();
                  }}
                >
                  Import from Workbench (Clipboard)
                </button>
              </div>
            </div>

            <div class="svn-config" style="margin-top: 10px;">
              <div class="svn-row">
                <label for="svn-folder-create">Folder SVN</label>
                <div class="svn-field">
                  <input
                    id="svn-folder-create"
                    type="text"
                    placeholder="Auto-detected SVN path"
                    value={svnFolderPath}
                    readonly
                    title={svnFolderPath || 'No folder selected'}
                    class:error={autoDetectError}
                    class:auto-detecting={autoDetecting && !svnFolderPath && !autoDetectError}
                  />
                  <div class="tooltip">
                    <button class="icon-btn" class:attention={autoDetectError} on:click={manualPickSvnFolder} aria-label="Select folder">
                      📂
                    </button>
                    <span class="tooltip-text">Choose folder manually</span>
                  </div>
                </div>
                {#if autoDetectError}
                  <p class="auto-detect-error">can't find your SVN folder, please use Browse Folder Button</p>
                {/if}
              </div>
              <div class="scan-row">
                <button
                  class={`scan-btn ${scanStatus}`}
                  on:click={triggerScan}
                  disabled={!svnFolderPath || scanStatus === 'scanning'}
                >
                  <span>Scan</span>
                  <span class={`scan-dot ${scanStatus}`} aria-hidden="true"></span>
                </button>
                <p class="scan-message">{scanMessage}</p>
              </div>
            </div>

            <div class="form-row" style="margin-top: 10px;">
              <span class="row-title">Save as</span>
              <button class="save-picker" type="button" on:click={pickSaveLocation}>
                {saveLocation || 'please click to select save location'}
              </button>
            </div>

            <div class="form-row">
              <span class="row-title">Blender Path</span>
              <button class="save-picker" type="button" on:click={pickBlenderPath}>
                {blenderPath || 'please click to select blender.exe'}
              </button>
            </div>

            {#if createdPrefabEtPath}
              <div class="completion-panel" style="margin-top: 10px;">
                <p class="completion-title">Created prefab .et</p>
                <p class="selected-path">{createdPrefabEtPath}</p>
                {#if createdPrefabMetaPath}
                  <p class="selected-path">{createdPrefabMetaPath}</p>
                {/if}
              </div>
            {/if}

            {#if createPrefabResults.length}
              <div class="completion-panel" style="margin-top: 10px;">
                <p class="completion-title">Batch results ({createPrefabResults.length})</p>
                <div class="file-summary">
                  {#each createPrefabResults.slice(-6) as r (r.et_path)}
                    <p class="file-name">{r.xob_path.split(/[\\/]/).pop() ?? r.xob_path} — {r.matched}/{r.sockets}</p>
                    <p class="file-path">{r.et_path}</p>
                    {#if r.meta_path}
                      <p class="file-path">{r.meta_path}</p>
                    {/if}
                  {/each}
                </div>
              </div>
            {/if}

            <div class="process-track" aria-hidden="true">
              <div class={`process-track-fill ${processRunning && processPct == null ? 'indeterminate' : ''}`} style={processPct != null ? `width:${processPct}%` : ''}></div>
            </div>
          </div>
        </div>
      </div>
    {/if}
  </section>

  <section class="section">
    <button class="section-header" on:click={() => toggle('log')}>
      <span>LOG</span>
      <span class="chevron" aria-hidden="true">{logExpanded ? '▾' : '▸'}</span>
    </button>
    {#if logExpanded}
      <div class="section-body log-body">
        <div class="log-controls">
          <input
            class="filter-input"
            type="text"
            placeholder="Filter logs..."
            bind:value={filterTerm}
          />
          <div class="log-actions">
            <button class="ghost-btn" on:click={copyLogs} disabled={!logs.length}>Copy</button>
            <button class="ghost-btn" on:click={exportLogs} disabled={!logs.length}>Export</button>
            <button class="clear-btn" on:click={clearLogs} disabled={!logs.length}>Clear</button>
          </div>
        </div>

        {#if filteredLogs.length === 0}
          <p class="placeholder">No log entries</p>
        {:else}
          <ul class="log-list">
            {#each filteredLogs as entry (entry.id)}
              <li class={`log-entry ${entry.type}`}>
                <span class="badge">{entry.type.toUpperCase()}</span>
                <span>{entry.message}</span>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    {/if}
  </section>

  {#if scanPopupOpen}
    <div class="scan-modal" role="dialog" aria-modal="true" aria-label="Scanning prefabs">
      <button type="button" class="scan-modal__backdrop" aria-label="Close" on:click={closeScanPopup}></button>
      <div class="scan-modal__panel">
        <div class="scan-modal__top">
          <div class="scan-modal__main">
            <div class="scan-modal__icon" aria-hidden="true">⬆</div>
            <div class="scan-modal__text">
              <div class="scan-modal__title">{scanPopupTitle}</div>
              <div class="scan-modal__sub">
                {#if scanPopupPct != null}
                  <span class="accent">{scanPopupCurrent}</span>
                  <span class="divider">/</span>
                  <span>{scanPopupTotal}</span>
                {:else}
                  <span>{scanPopupSub || 'Working...'}</span>
                {/if}
              </div>
            </div>
          </div>
          <button
            type="button"
            class="scan-modal__close"
            aria-label="Close"
            disabled={scanPopupBusy}
            on:click={closeScanPopup}
          >
            ✕
          </button>
        </div>

        <div class="scan-modal__track" aria-hidden="true">
          <div
            class="scan-modal__track-fill"
            style={scanPopupPct != null ? `width:${scanPopupPct}%` : 'width:25%'}
            class:indeterminate={scanPopupPct == null}
          ></div>
        </div>

        <div class="scan-modal__body">
          <div class="scan-modal__owl" aria-hidden="true">
            {#if scanOwlImgOk}
              <img class="scan-modal__owl-img" src="/owl_find.png" alt="" on:error={() => (scanOwlImgOk = false)} />
            {/if}
          </div>
          <div class="scan-modal__log">
            <div class="scan-modal__log-title">LOG</div>
            <div class="scan-modal__log-box">
              {#if scanPopupLogs.length}
                {#each scanPopupLogs as line, idx (idx)}
                  <div class="scan-modal__log-line">{line}</div>
                {/each}
              {:else}
                <div class="scan-modal__log-line">info: waiting...</div>
              {/if}
            </div>
            <div class="scan-modal__actions">
              <button type="button" class="ghost-btn small" disabled={scanPopupBusy} on:click={() => (scanPopupLogs = [])}>
                Clear
              </button>
              <button type="button" class="primary-btn slim" disabled={scanPopupBusy} on:click={triggerScan}>
                Retry
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .socket-manager {
    display: flex;
    flex-direction: column;
    gap: 16px;
    width: 100%;
  }

  .scan-modal {
    position: fixed;
    inset: 0;
    z-index: 50;
    display: grid;
    place-items: center;
    padding: 20px;
  }

  .scan-modal__backdrop {
    position: absolute;
    inset: 0;
    border: none;
    padding: 0;
    margin: 0;
    background: rgba(0, 0, 0, 0.55);
  }

  .scan-modal__panel {
    position: relative;
    width: min(980px, 100%);
    background: rgba(25, 28, 36, 0.98);
    border: 1px solid rgba(255, 255, 255, 0.12);
    border-radius: 16px;
    overflow: hidden;
    box-shadow: 0 22px 70px rgba(0, 0, 0, 0.55);
  }
  :global(body.theme-light) .scan-modal__panel {
    background: rgba(255, 255, 255, 0.98);
    border-color: rgba(0, 0, 0, 0.12);
    box-shadow: 0 22px 70px rgba(0, 0, 0, 0.25);
  }

  .scan-modal__top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 14px 16px;
  }

  .scan-modal__main {
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
  }

  .scan-modal__icon {
    width: 34px;
    height: 34px;
    display: grid;
    place-items: center;
    border-radius: 999px;
    background: rgba(91, 124, 250, 0.18);
  }

  .scan-modal__text {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }

  .scan-modal__title {
    font-weight: 700;
  }

  .scan-modal__sub {
    opacity: 0.85;
    font-size: 0.92rem;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .scan-modal__close {
    border: 1px solid rgba(255, 255, 255, 0.16);
    background: rgba(255, 255, 255, 0.06);
    border-radius: 10px;
    padding: 8px 10px;
    color: inherit;
    cursor: pointer;
  }
  .scan-modal__close:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  :global(body.theme-light) .scan-modal__close {
    border-color: rgba(0, 0, 0, 0.16);
    background: rgba(0, 0, 0, 0.04);
  }

  .scan-modal__track {
    height: 6px;
    background: rgba(255, 255, 255, 0.1);
  }
  :global(body.theme-light) .scan-modal__track {
    background: rgba(0, 0, 0, 0.08);
  }

  .scan-modal__track-fill {
    height: 100%;
    background: linear-gradient(90deg, #5b7cfa, #7f92ff);
    transition: width 180ms ease;
  }
  .scan-modal__track-fill.indeterminate {
    width: 35%;
    animation: scanIndeterminate 1200ms ease-in-out infinite;
  }
  @keyframes scanIndeterminate {
    0% { transform: translateX(-40%); }
    50% { transform: translateX(120%); }
    100% { transform: translateX(240%); }
  }

  .scan-modal__body {
    display: grid;
    grid-template-columns: 280px 1fr;
    gap: 16px;
    padding: 16px;
  }
  @media (max-width: 860px) {
    .scan-modal__body {
      grid-template-columns: 1fr;
    }
  }

  .scan-modal__owl {
    min-height: 260px;
    border-radius: 14px;
    background: radial-gradient(circle at 30% 20%, rgba(255, 181, 46, 0.25), transparent 55%),
      radial-gradient(circle at 70% 70%, rgba(91, 124, 250, 0.22), transparent 60%),
      rgba(255, 255, 255, 0.06);
    border: 1px solid rgba(255, 255, 255, 0.12);
    display: grid;
    place-items: center;
    overflow: hidden;
  }
  :global(body.theme-light) .scan-modal__owl {
    background: radial-gradient(circle at 30% 20%, rgba(255, 181, 46, 0.2), transparent 55%),
      radial-gradient(circle at 70% 70%, rgba(91, 124, 250, 0.18), transparent 60%),
      rgba(0, 0, 0, 0.03);
    border-color: rgba(0, 0, 0, 0.12);
  }

  .scan-modal__owl-img {
    width: 100%;
    height: 100%;
    max-width: 320px;
    object-fit: contain;
    filter: drop-shadow(0 10px 30px rgba(0, 0, 0, 0.35));
  }
  :global(body.theme-light) .scan-modal__owl-img {
    filter: drop-shadow(0 10px 30px rgba(0, 0, 0, 0.2));
  }

  .scan-modal__log {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .scan-modal__log-title {
    font-weight: 700;
  }

  .scan-modal__log-box {
    height: 300px;
    overflow: auto;
    border-radius: 14px;
    padding: 12px;
    background: rgba(0, 0, 0, 0.35);
    border: 1px solid rgba(255, 255, 255, 0.12);
    font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace;
    font-size: 0.9rem;
    line-height: 1.45;
  }
  :global(body.theme-light) .scan-modal__log-box {
    background: rgba(255, 255, 255, 0.7);
    border-color: rgba(0, 0, 0, 0.12);
  }

  .scan-modal__log-line {
    white-space: pre-wrap;
    word-break: break-word;
  }

  .scan-modal__actions {
    display: flex;
    gap: 10px;
    justify-content: flex-end;
  }

  h2 {
    margin: 0;
    font-size: 1.4rem;
  }

  .section {
    border: 1px solid rgba(255, 255, 255, 0.15);
    border-radius: 12px;
    background: rgba(255, 255, 255, 0.04);
    overflow: hidden;
  }
  :global(body.theme-light) .section {
    border-color: rgba(0, 0, 0, 0.12);
    background: rgba(0, 0, 0, 0.03);
  }

  .section-header {
    width: 100%;
    background: transparent;
    border: none;
    display: flex;
    justify-content: space-between;
    align-items: center;
    font-size: 1rem;
    font-weight: 600;
    padding: 12px 16px;
    cursor: pointer;
    color: inherit;
  }

  .section-header:hover {
    background: rgba(255, 255, 255, 0.06);
  }
  :global(body.theme-light) .section-header:hover {
    background: rgba(0, 0, 0, 0.05);
  }

  .chevron {
    font-size: 1.1rem;
  }

  .section-body {
    padding: 16px;
    border-top: 1px solid rgba(255, 255, 255, 0.08);
    resize: vertical;
    overflow: auto;
    min-height: 140px;
    max-height: 80vh;
  }
  :global(body.theme-light) .section-body {
    border-top-color: rgba(0, 0, 0, 0.08);
  }

  .placeholder {
    margin: 0;
    opacity: 0.75;
  }

  .stepper-header {
    display: flex;
    flex-direction: column;
    gap: 16px;
    margin-bottom: 20px;
  }
  .progress-bar {
    position: relative;
    height: 6px;
    background: rgba(255, 255, 255, 0.1);
    border-radius: 999px;
    overflow: hidden;
  }
  :global(body.theme-light) .progress-bar {
    background: rgba(0, 0, 0, 0.08);
  }
  .progress-fill {
    position: absolute;
    top: 0;
    left: 0;
    bottom: 0;
    width: 0;
    background: linear-gradient(90deg, #5b7cfa, #7f92ff);
    border-radius: inherit;
    transition: width 180ms ease;
  }
  .bubble-row {
    display: flex;
    justify-content: space-between;
    gap: 12px;
  }
  .bubble-group {
    display: flex;
    flex-direction: column;
    align-items: center;
    flex: 1;
    gap: 6px;
    text-align: center;
  }
  .bubble {
    width: 34px;
    height: 34px;
    border-radius: 50%;
    border: 2px solid #7f92ff;
    color: #7f92ff;
    display: flex;
    align-items: center;
    justify-content: center;
    font-weight: 600;
    background: #151a33;
  }
  .bubble.active {
    background: #7f92ff;
    color: #fff;
  }
  .bubble.complete {
    background: #5fc4a9;
    border-color: #5fc4a9;
    color: #0e2721;
  }
  :global(body.theme-light) .bubble {
    background: #fff;
  }
  :global(body.theme-light) .bubble.active {
    color: #000;
  }
  .bubble-label {
    margin: 0;
    font-size: 0.85rem;
    color: rgba(255, 255, 255, 0.9);
  }
  :global(body.theme-light) .bubble-label {
    color: rgba(0, 0, 0, 0.7);
  }
  .bubble-subtitle {
    margin: 0;
    font-size: 0.8rem;
    color: rgba(255, 255, 255, 0.65);
  }
  :global(body.theme-light) .bubble-subtitle {
    color: rgba(0, 0, 0, 0.55);
  }

  .step-panel {
    border-radius: 12px;
    padding: 28px;
    margin-bottom: 16px;
    background: rgba(255, 255, 255, 0.03);
    min-height: 80px;
  }
  :global(body.theme-light) .step-panel {
    background: rgba(0, 0, 0, 0.03);
  }

  .drop-zone {
    border: 2px dashed rgba(255, 255, 255, 0.3);
    border-radius: 12px;
    padding: 40px;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-direction: column;
    gap: 8px;
    width: 100%;
    min-height: 140px;
  }
  .drop-zone.drag-active {
    border-color: #3ecbff;
    background: rgba(62, 203, 255, 0.08);
  }
  .drop-zone.selected {
    border-color: #36d399;
    background: rgba(54, 211, 153, 0.12);
  }
  .drop-label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    transition: transform 0.2s ease, opacity 0.2s ease;
  }
  .drop-label strong {
    font-size: 1.1rem;
  }
  .drop-label span {
    font-size: 0.95rem;
  }
  .selected-file {
    margin: 0;
    font-size: 0.9rem;
    opacity: 0.85;
    transition: transform 0.2s ease, color 0.2s ease, font-size 0.2s ease;
  }
  .drop-zone.selected .drop-label strong,
  .drop-zone.selected .drop-label span {
    transform: translateY(-10px) scale(0.8);
    opacity: 0.5;
  }
  .selected-path {
    margin: 0;
    font-size: 0.85rem;
    opacity: 0.7;
  }
  .drop-actions {
    margin-top: 12px;
    display: flex;
    justify-content: flex-end;
  }
  .native-picker {
    border: 1px solid rgba(255, 255, 255, 0.2);
    padding: 6px 12px;
    border-radius: 8px;
    background: transparent;
    color: inherit;
  }
  .drop-zone.selected .selected-file {
    font-size: 1.05rem;
    font-weight: 600;
    color: #36d399;
    transform: translateY(4px);
  }

  .queue-list {
    margin-top: 6px;
    max-height: 110px;
    overflow: auto;
  }
  .queue-item {
    margin: 0;
    font-size: 0.85rem;
    opacity: 0.8;
    text-align: left;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .svn-config {
    display: flex;
    flex-direction: column;
    gap: 18px;
  }
  .svn-row label {
    font-weight: 600;
    margin-bottom: 6px;
    display: inline-block;
  }
  .svn-field {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .svn-field input[type='text'] {
    flex: 1;
    padding: 10px 14px;
    border-radius: 10px;
    border: 1px solid rgba(255, 255, 255, 0.2);
    background: rgba(255, 255, 255, 0.02);
    color: inherit;
  }
  .svn-field input[type='text'].error {
    border-color: #ff6b6b;
    box-shadow: 0 0 0 1px rgba(255, 107, 107, 0.4);
    background: rgba(255, 107, 107, 0.08);
  }
  .svn-field input[type='text'].auto-detecting {
    border-color: rgba(62, 203, 255, 0.8);
    box-shadow: 0 0 12px rgba(62, 203, 255, 0.6), inset 0 0 12px rgba(62, 203, 255, 0.2);
    animation: sweep 1.2s linear infinite;
  }
  :global(body.theme-light) .svn-field input[type='text'] {
    background: rgba(255, 255, 255, 0.9);
    border-color: rgba(0, 0, 0, 0.12);
  }
  :global(body.theme-light) .svn-field input[type='text'].error {
    border-color: #e64545;
    box-shadow: 0 0 0 1px rgba(230, 69, 69, 0.4);
    background: rgba(230, 69, 69, 0.08);
  }
  :global(body.theme-light) .svn-field input[type='text'].auto-detecting {
    border-color: rgba(62, 131, 255, 0.8);
    box-shadow: 0 0 12px rgba(62, 131, 255, 0.5), inset 0 0 12px rgba(62, 131, 255, 0.15);
  }
  .icon-btn {
    width: 38px;
    height: 38px;
    border-radius: 10px;
    border: 1px solid rgba(255, 255, 255, 0.25);
    background: rgba(255, 255, 255, 0.05);
    cursor: pointer;
    font-size: 1rem;
  }
  .icon-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
  :global(body.theme-light) .icon-btn {
    background: rgba(0, 0, 0, 0.05);
    border-color: rgba(0, 0, 0, 0.12);
  }
  .icon-btn.attention {
    box-shadow: 0 0 12px rgba(255, 107, 107, 0.6), 0 0 24px rgba(255, 107, 107, 0.4);
    animation: glow 1.4s ease-in-out infinite;
    border-color: rgba(255, 107, 107, 0.6);
  }
  .tooltip {
    position: relative;
    display: inline-block;
  }
  .tooltip-text {
    position: absolute;
    bottom: calc(100% + 6px);
    left: 50%;
    transform: translateX(-50%);
    background: rgba(17, 20, 40, 0.95);
    color: #fff;
    padding: 6px 10px;
    border-radius: 6px;
    font-size: 0.75rem;
    white-space: nowrap;
    opacity: 0;
    pointer-events: none;
    transition: opacity 0.15s ease;
  }
  :global(body.theme-light) .tooltip-text {
    background: rgba(20, 20, 20, 0.9);
  }
  .tooltip:hover .tooltip-text {
    opacity: 1;
  }
  .native-folder-input {
    display: none;
  }

  .scan-row {
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .scan-btn {
    display: inline-flex;
    align-items: center;
    gap: 10px;
    padding: 10px 18px;
    border-radius: 999px;
    border: 1px solid rgba(255, 255, 255, 0.25);
    background: rgba(255, 255, 255, 0.05);
    color: inherit;
    cursor: pointer;
    font-weight: 600;
  }
  .scan-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  :global(body.theme-light) .scan-btn {
    background: rgba(0, 0, 0, 0.04);
    border-color: rgba(0, 0, 0, 0.15);
  }
  .scan-dot {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    border: 2px solid transparent;
  }
  .scan-dot.idle {
    background: #8d93a5;
  }
  .scan-dot.scanning {
    background: #f5a524;
    animation: pulse 1s infinite;
  }
  .scan-dot.success {
    background: #36d399;
  }
  .scan-dot.error {
    background: #ff6b6b;
  }
  .scan-message {
    margin: 0;
    font-size: 0.9rem;
    opacity: 0.8;
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .auto-detect-error {
    margin: 6px 0 0;
    color: #ff6b6b;
    font-size: 0.9rem;
    font-weight: 600;
  }

  .add-folder {
    margin-top: 8px;
  }

  .placeholder.small {
    font-size: 0.85rem;
    opacity: 0.6;
  }

  .custom-prefab-section {
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 12px;
    padding: 16px;
    background: rgba(255, 255, 255, 0.02);
  }
  :global(body.theme-light) .custom-prefab-section {
    border-color: rgba(0, 0, 0, 0.08);
    background: rgba(0, 0, 0, 0.02);
  }

  .section-header-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
    margin-bottom: 12px;
  }
  .section-title {
    margin: 0;
    font-weight: 700;
    font-size: 1rem;
  }
  .section-subtitle {
    margin: 2px 0 0;
    opacity: 0.65;
    font-size: 0.9rem;
  }

  .prefab-folders {
    display: flex;
    flex-wrap: wrap;
    gap: 16px;
  }

  .prefab-config {
    display: flex;
    flex-direction: column;
    gap: 24px;
  }

  .row-title {
    font-weight: 600;
    min-width: 130px;
  }

  .preset-controls {
    display: flex;
    gap: 12px;
    align-items: center;
    flex-wrap: wrap;
  }
  .preset-select {
    min-width: 200px;
    padding: 8px 12px;
    border-radius: 10px;
    border: 1px solid rgba(255, 255, 255, 0.2);
    background: rgba(255, 255, 255, 0.02);
    color: inherit;
  }
  .preset-input {
    min-width: 200px;
    padding: 8px 12px;
    border-radius: 10px;
    border: 1px solid rgba(255, 255, 255, 0.2);
    background: rgba(255, 255, 255, 0.02);
    color: inherit;
  }
  :global(body):not(.theme-light) .preset-select {
    background: rgba(255, 255, 255, 0.92);
    color: #0f0f10;
    border-color: rgba(255, 255, 255, 0.4);
  }
  :global(body):not(.theme-light) .preset-input {
    background: rgba(255, 255, 255, 0.92);
    color: #0f0f10;
    border-color: rgba(255, 255, 255, 0.4);
  }
  .preset-actions {
    display: flex;
    gap: 8px;
  }
  .ghost-btn.small {
    padding: 6px 10px;
    font-size: 0.9rem;
  }

  .file-summary {
    margin-bottom: 12px;
  }
  .file-name {
    margin: 0;
    font-weight: 600;
  }
  .file-path {
    margin: 4px 0 0;
    font-size: 0.9rem;
    opacity: 0.8;
  }
  .file-placeholder {
    margin: 0 0 12px;
    font-style: italic;
    opacity: 0.6;
  }

  .save-picker {
    padding: 10px 16px;
    border-radius: 10px;
    border: 1px dashed rgba(255, 255, 255, 0.3);
    background: rgba(255, 255, 255, 0.02);
    color: inherit;
  }
  :global(body.theme-light) .save-picker {
    border-color: rgba(0, 0, 0, 0.28);
  }

  .prefab-board {
    border: 1px solid rgba(255, 255, 255, 0.12);
    border-radius: 10px;
    padding: 12px;
    background: rgba(0, 0, 0, 0.25);
  }
  :global(body.theme-light) .prefab-board {
    background: rgba(255, 255, 255, 0.95);
    border-color: rgba(0, 0, 0, 0.1);
  }
  .prefab-column {
    flex: 1;
  }

  .custom-panel-body {
    display: flex;
    gap: 12px;
  }
  .custom-list {
    flex: 1;
    max-height: 120px;
    background: rgba(0, 0, 0, 0.3);
    border-radius: 8px;
    padding: 8px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    overflow-y: auto;
  }
  :global(body.theme-light) .custom-list {
    background: rgba(0, 0, 0, 0.04);
  }
  .custom-entry {
    text-align: left;
    width: 100%;
    border: none;
    border-radius: 6px;
    padding: 6px 8px;
    background: transparent;
    color: inherit;
    cursor: pointer;
  }
  .custom-entry:hover {
    background: rgba(255, 255, 255, 0.08);
  }
  .custom-entry.active {
    background: rgba(127, 146, 255, 0.25);
    border: 1px solid rgba(127, 146, 255, 0.6);
  }
  .custom-actions {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .circle-btn {
    width: 36px;
    height: 36px;
    border-radius: 50%;
    border: 1px solid rgba(255, 255, 255, 0.4);
    background: rgba(127, 146, 255, 0.1);
    color: inherit;
    font-size: 1.2rem;
    line-height: 1;
    cursor: pointer;
  }
  .circle-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .processing-panel {
    display: flex;
    flex-direction: column;
    gap: 18px;
    padding: 18px;
    border-radius: 16px;
    background: rgba(0, 0, 0, 0.25);
    border: 1px solid rgba(255, 255, 255, 0.08);
  }
  :global(body.theme-light) .processing-panel {
    background: rgba(255, 255, 255, 0.95);
    border-color: rgba(0, 0, 0, 0.08);
  }
  .process-card {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  .process-main {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .process-icon {
    width: 48px;
    height: 48px;
    border-radius: 12px;
    border: 1px solid rgba(255, 255, 255, 0.25);
    background: rgba(255, 255, 255, 0.05);
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 1.2rem;
  }
  :global(body.theme-light) .process-icon {
    border-color: rgba(0, 0, 0, 0.1);
    background: rgba(0, 0, 0, 0.05);
  }
  .process-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .process-title {
    margin: 0;
    font-weight: 600;
  }
  .process-subtext {
    margin: 0;
    font-size: 0.9rem;
    display: flex;
    gap: 6px;
    align-items: baseline;
  }
  .process-subtext .accent {
    color: #45d37c;
    font-weight: 600;
  }
  .process-action {
    width: 36px;
    height: 36px;
    border-radius: 10px;
    border: 1px solid rgba(255, 255, 255, 0.25);
    background: transparent;
    color: inherit;
    cursor: not-allowed;
  }
  :global(body.theme-light) .process-action {
    border-color: rgba(0, 0, 0, 0.15);
  }
  .process-track {
    width: 100%;
    height: 8px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.08);
    overflow: hidden;
  }
  :global(body.theme-light) .process-track {
    background: rgba(0, 0, 0, 0.08);
  }
  .process-track-fill {
    height: 100%;
    width: 0;
    border-radius: inherit;
    background: linear-gradient(90deg, #57cf63, #8ee59c);
    transition: width 400ms ease;
  }
  .process-track-fill.indeterminate {
    width: 35%;
    animation: processIndeterminate 1200ms ease-in-out infinite;
  }
  @keyframes processIndeterminate {
    0% { transform: translateX(-40%); }
    50% { transform: translateX(120%); }
    100% { transform: translateX(240%); }
  }
  .process-log {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  @keyframes sweep {
    0% {
      box-shadow: 0 0 12px rgba(62, 203, 255, 0.6), inset 0 0 12px rgba(62, 203, 255, 0.2);
    }
    50% {
      box-shadow: 0 0 18px rgba(62, 203, 255, 0.9), inset 0 0 16px rgba(62, 203, 255, 0.35);
    }
    100% {
      box-shadow: 0 0 12px rgba(62, 203, 255, 0.6), inset 0 0 12px rgba(62, 203, 255, 0.2);
    }
  }

  @keyframes pulse {
    0% { transform: scale(1); opacity: 1; }
    50% { transform: scale(1.3); opacity: 0.6; }
    100% { transform: scale(1); opacity: 1; }
  }

  .step-footer {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }
  .step-count {
    font-size: 0.9rem;
    opacity: 0.7;
  }
  .step-actions {
    display: flex;
    gap: 10px;
  }
  .primary-btn {
    padding: 8px 18px;
    border-radius: 10px;
    background: #2f80ed;
    border: 1px solid #2f80ed;
    color: white;
    font-weight: 600;
    cursor: pointer;
  }
  .primary-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .log-body {
    display: flex;
    flex-direction: column;
    gap: 12px;
    background: rgba(0, 0, 0, 0.2);
    border-radius: 8px;
    min-height: 0;
  }
  :global(body.theme-light) .log-body {
    background: rgba(0, 0, 0, 0.04);
  }

  .log-controls {
    display: flex;
    gap: 10px;
    flex-wrap: wrap;
  }

  .filter-input {
    flex: 1;
    min-width: 200px;
    padding: 6px 10px;
    border-radius: 8px;
    border: 1px solid rgba(255, 255, 255, 0.2);
    background: rgba(0, 0, 0, 0.2);
    color: inherit;
  }
  :global(body.theme-light) .filter-input {
    background: rgba(255, 255, 255, 0.9);
    border-color: rgba(0, 0, 0, 0.1);
  }

  .log-actions {
    display: flex;
    gap: 6px;
  }

  .ghost-btn,
  .clear-btn {
    padding: 6px 12px;
    border-radius: 8px;
    border: 1px solid rgba(255, 255, 255, 0.3);
    background: transparent;
    color: inherit;
    cursor: pointer;
  }
  .ghost-btn:disabled,
  .clear-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
  :global(body.theme-light) .ghost-btn,
  :global(body.theme-light) .clear-btn {
    border-color: rgba(0, 0, 0, 0.2);
  }

  .log-list {
    list-style: none;
    margin: 0;
    padding: 0;
    font-family: 'Fira Code', 'SFMono-Regular', Consolas, monospace;
    font-size: 0.9rem;
    flex: 1;
    min-height: 0;
    overflow: auto;
    border-radius: 8px;
    border: 1px solid rgba(255, 255, 255, 0.08);
  }
  :global(body.theme-light) .log-list {
    border-color: rgba(0, 0, 0, 0.08);
  }

  .log-entry {
    padding: 8px 12px;
    border-bottom: 1px solid rgba(255, 255, 255, 0.08);
    display: flex;
    gap: 12px;
    align-items: center;
  }
  :global(body.theme-light) .log-entry {
    border-bottom-color: rgba(0, 0, 0, 0.08);
  }
  .log-entry:last-child {
    border-bottom: none;
  }

  .badge {
    font-size: 0.75rem;
    letter-spacing: 0.05em;
    padding: 4px 8px;
    border-radius: 999px;
    font-weight: 600;
  }

  .log-entry.info .badge {
    background: rgba(0, 170, 255, 0.15);
    color: #3ecbff;
  }
  .log-entry.warn .badge {
    background: rgba(255, 196, 0, 0.18);
    color: #ffb400;
  }
  .log-entry.error .badge {
    background: rgba(255, 77, 77, 0.18);
    color: #ff5757;
  }
</style>
