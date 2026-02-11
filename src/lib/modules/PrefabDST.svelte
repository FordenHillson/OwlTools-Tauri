<script lang="ts">
  import { open } from '@tauri-apps/plugin-dialog';
  import { onMount } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { invoke } from '@tauri-apps/api/core';
  import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';

  type ProjectOption = { key: string; label: string };
  type PresetOption = { key: string; label: string; projectKey: string; generator: string; description: string; file: string };

  let title = $state<string>('Build Prefab DST');
  let projects = $state<ProjectOption[]>([]);
  let presets = $state<PresetOption[]>([]);
  let isPresetLoading = $state<boolean>(true);
  let projectKey = $state<string>('');
  let presetKey = $state<string>('');
  let zones = $state<number>(3);
  let hpZone = $state<number>(50);
  let debrisMass = $state<number>(500);

  let modelFiles = $state<string[]>([]);
  let saveFolder = $state<string>('');
  let isBuilding = $state<boolean>(false);
  let activeModelIndex = $state<number>(0);
  const activeModelFile = $derived(modelFiles[activeModelIndex] || '');

  type PrefabDstLogPayload = { level: string; message: string; current?: number | null; total?: number | null };
  let unlistenBuildLog: (() => void) | null = null;

  type LogLine = { id: string; level: 'INFO' | 'WARN' | 'ERROR'; text: string };
  let logLines = $state<LogLine[]>([
    { id: 'init', level: 'INFO', text: 'Ready.' }
  ]);

  function slugifyProject(s: string) {
    return (s || '').trim().toLowerCase().replace(/\s+/g, '').replace(/[^a-z0-9_\-]/g, '');
  }

  function dirname(p: string): string {
    if (!p) return '';
    const i1 = p.lastIndexOf('\\');
    const i2 = p.lastIndexOf('/');
    const i = Math.max(i1, i2);
    return i >= 0 ? p.slice(0, i) : '';
  }

  function basename(p: string): string {
    if (!p) return '';
    const i1 = p.lastIndexOf('\\');
    const i2 = p.lastIndexOf('/');
    const i = Math.max(i1, i2);
    return i >= 0 ? p.slice(i + 1) : p;
  }

  function joinPath(dir: string, name: string): string {
    const d = String(dir || '').trim();
    const n = String(name || '').trim();
    if (!d) return n;
    if (!n) return d;
    const sep = d.includes('\\') ? '\\' : '/';
    return d.endsWith('\\') || d.endsWith('/') ? `${d}${n}` : `${d}${sep}${n}`;
  }

  function fileStem(p: string): string {
    const b = basename(p);
    const i = b.lastIndexOf('.');
    return i > 0 ? b.slice(0, i) : b;
  }

  async function tryReadXobMeta(xobPath: string): Promise<{ guid: string; path: string } | null> {
    try {
      const meta = await invoke<{ guid: string; path: string }>('prefabdst_read_meta', { xobPath });
      const g = String((meta as any)?.guid || '');
      const p = String((meta as any)?.path || xobPath);
      if (!g) return null;
      return { guid: g, path: p };
    } catch {
      return null;
    }
  }

  async function resolveSiblingRuinXobIfAny(xobPath: string): Promise<string> {
    const base = normalizeXobPath(xobPath);
    try {
      const found = await invoke<string | null>('prefabdst_find_ruin_xob', { xobPath: base });
      if (typeof found === 'string' && found) return found;
    } catch {}
    return base;
  }

  // ---- Drag & Drop helpers (adapted from SocketManager) ----
  function chooseDropEffect(e: DragEvent) {
    const dt = e.dataTransfer;
    if (!dt) return;
    const types = Array.from((dt.types || []) as any);
    if (types.includes('text/uri-list')) { dt.dropEffect = 'link'; return; }
    if (types.includes('Files')) { dt.dropEffect = 'copy'; return; }
    const allowed = (dt.effectAllowed || '').toLowerCase();
    if (allowed.includes('copy') || allowed === 'all' || allowed === 'uninitialized' || allowed === '') dt.dropEffect = 'copy';
    else if (allowed.includes('link')) dt.dropEffect = 'link';
    else if (allowed.includes('move')) dt.dropEffect = 'move';
    else dt.dropEffect = 'copy';
  }

  function parseWorkbenchDrag(data: string): string | null {
    let s = String(data || '').trim();
    if (!s) return null;
    try { s = decodeURIComponent(s); } catch {}
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

  function normalizeXobPath(p: string): string {
    const s = String(p || '').trim();
    const lower = s.toLowerCase();
    if (lower.endsWith('.xob.meta')) return s.slice(0, -5);
    return s;
  }

  function addModelFileNormalized(path: string) {
    const n = normalizeXobPath(path);
    if (!n.toLowerCase().endsWith('.xob')) return;
    modelFiles = Array.from(new Set([...modelFiles, n]));
    if (!saveFolder && modelFiles.length > 0) saveFolder = dirname(modelFiles[0]);
  }

  function extractWorkbenchText(dt: DataTransfer): string {
    const candidates = ['text/uri-list', 'text/plain', 'text/x-moz-url', 'text/x-url', 'URL'];
    for (const t of candidates) {
      try { const v = dt.getData(t); if (v) return v; } catch {}
    }
    if (dt.types) {
      for (const t of Array.from(dt.types as any)) {
        try { const v = dt.getData(t as string); if (v) return v; } catch {}
      }
    }
    return '';
  }

  function moveDebris(delta: number) {
    const sel = scrSel;
    if (!sel || sel.kind !== 'debris') return;
    const { pid, idx } = sel;
    const ph = scrPhases.find((p) => p.pid === pid);
    if (!ph) return;
    const arr = [...ph.debris];
    const ni = idx + delta;
    if (ni < 0 || ni >= arr.length) return;
    const tmp = arr[idx]; arr[idx] = arr[ni]; arr[ni] = tmp;
    scrPhases = scrPhases.map((p) => (p.pid === pid ? { ...p, debris: arr } : p));
    scrSel = { kind: 'debris', pid, idx: ni };
  }

  function parsePresetHeader(text: string): { id: string; title: string; project: string; generator: string; description: string } {
    const lines = (text || '').split(/\r?\n/);
    const out = { id: '', title: '', project: '', generator: '', description: '' };
    for (const raw of lines) {
      const line = raw.trim();
      if (!line) break;
      const idx = line.indexOf(':');
      if (idx < 0) continue;
      const k = line.slice(0, idx).trim().toLowerCase();
      const v = line.slice(idx + 1).trim();
      if (k === 'id') out.id = v;
      else if (k === 'title') out.title = v;
      else if (k === 'project') out.project = v;
      else if (k === 'generator') out.generator = v;
      else if (k === 'description') out.description = v;
    }
    return out;
  }

  async function loadPresetsFromStatic() {
    isPresetLoading = true;
    try {
      const res = await fetch('/presets/index.json', { cache: 'no-store' });
      if (!res.ok) {
        pushLog('ERROR', 'Failed to load presets/index.json');
        return;
      }
      const index = (await res.json()) as any;
      const files = Array.isArray(index?.files) ? (index.files as string[]) : [];
      const metas: PresetOption[] = [];
      const projectLabels = new Map<string, string>();
      for (const f of files) {
        try {
          const r = await fetch(`/presets/${encodeURIComponent(f)}`, { cache: 'no-store' });
          if (!r.ok) continue;
          const txt = await r.text();
          const h = parsePresetHeader(txt);
          const pKey = slugifyProject(h.project);
          if (pKey && h.project) projectLabels.set(pKey, h.project);
          const key = (h.id || f).trim();
          metas.push({
            key,
            label: (h.title || key).trim(),
            projectKey: pKey || 'unknown',
            generator: (h.generator || '').trim(),
            description: (h.description || '').trim(),
            file: f
          });

  
        } catch {}
      }
      const proj: ProjectOption[] = Array.from(projectLabels.entries())
        .sort((a, b) => a[1].localeCompare(b[1]))
        .map(([key, label]) => ({ key, label }));
      projects = proj.length ? proj : [{ key: 'unknown', label: 'Unknown' }];
      presets = metas;
      pushLog('INFO', `Loaded ${metas.length} preset(s).`);
    } catch (e) {
      console.error(e);
      pushLog('ERROR', 'Failed to load presets.');
    } finally {
      isPresetLoading = false;
    }
  }

  function loadFullFromCacheForActive() {
    const f = activeModelFile;
    if (!f) return;
    const s = fullByFile[f];
    if (s) {
      fullBaseGuid = s.base_guid || '';
      fullBasePath = s.base_path || '';
      fullV2Guid = s.v2_guid || '';
      fullV2Path = s.v2_path || '';
      fullZones = Array.isArray(s.zones) ? s.zones : [];
      fullSel = null;
    } else {
      fullBaseGuid = '';
      fullBasePath = '';
      fullV2Guid = '';
      fullV2Path = '';
      fullZones = [];
      fullSel = null;
    }
  }

  const filteredPresets = $derived(presets.filter((p) => p.projectKey === projectKey));
  $effect(() => {
    if (!projectKey && projects.length) {
      projectKey = projects[0]?.key ?? '';
    }
    const ok = filteredPresets.some((p) => p.key === presetKey);
    if (!ok) presetKey = filteredPresets[0]?.key ?? '';
  });

  onMount(() => {
    loadPresetsFromStatic();

    listen<PrefabDstLogPayload>('prefabdst_log', (event) => {
      const p = event?.payload;
      if (!p) return;
      const lvl = String(p.level || 'info').toLowerCase();
      const msg = String(p.message || '');
      const mapped: 'INFO' | 'WARN' | 'ERROR' =
        lvl === 'error' ? 'ERROR' : lvl === 'warn' || lvl === 'warning' ? 'WARN' : 'INFO';
      pushLog(mapped, msg);
    }).then((u) => {
      unlistenBuildLog = u;
    });

    // Enable the global drop overlay while this module is mounted.
    try {
      window.dispatchEvent(new CustomEvent('app:set-allow-drop', { detail: true }));
    } catch {}

    // Accept resolved drops produced by global overlay
    const onWorkbench = (e: any) => {
      try {
        const detail = e?.detail;
        if (typeof detail === 'string' && detail) {
          const parsed = parseWorkbenchDrag(detail) || detail;
          if (parsed) addModelFileNormalized(parsed);
        }
      } catch {}
    };
    window.addEventListener('workbench-drop-path', onWorkbench as any);

    // Accept native file drops
    const unsubs: Array<() => void> = [];
    listen<string[] | string>('tauri://file-drop', (e) => {
      const payload = e.payload as any;
      const paths = Array.isArray(payload) ? payload : [payload];
      for (const p of paths) {
        if (typeof p === 'string' && p) addModelFileNormalized(p);
      }
    }).then((u) => { unsubs.push(u); });

    // Track hover using webview API (for visual highlight)
    try {
      const webview = getCurrentWebviewWindow();
      webview.onDragDropEvent((ev: any) => {
        const t = ev?.payload?.type ?? ev?.type ?? ev?.event ?? ev?.kind;
        if (t === 'hover' || t === 'over' || t === 'enter') {
          updateFilesHoverFromCoords(ev);
        }
        if (t === 'leave' || t === 'cancelled') {
          filesIsDragOver = false;
        }
        if (t === 'drop') {
          filesIsDragOver = false;
          const pathsAny = ev?.payload?.paths ?? ev?.paths ?? ev?.payload;
          const paths = Array.isArray(pathsAny) ? pathsAny : [pathsAny];
          for (const p of paths) {
            if (typeof p === 'string' && p) addModelFileNormalized(p);
          }
        }
      }).then((u:any)=>{ /* some tauri versions return unlisten */ try{ if(typeof u==='function') unsubs.push(u);}catch{} });
    } catch {}
  });

  $effect(() => {
    return () => {
      try { unlistenBuildLog && unlistenBuildLog(); } catch {}
      unlistenBuildLog = null;
      try { window.dispatchEvent(new CustomEvent('app:set-allow-drop', { detail: false })); } catch {}
    };
  });

  // Set default save folder automatically to the directory of the first input file
  $effect(() => {
    if (!saveFolder && modelFiles.length > 0) {
      saveFolder = dirname(modelFiles[0]);
    }
  });

  // Auto-scan dst when preset is scr_destructible and a base file is present
  $effect(() => {
    const key = isScrDestructible && !!activeModelFile ? `${presetKey}|${activeModelFile}` : '';
    if (key && lastScrScanKey !== key) {
      lastScrScanKey = key;
      scanDst();
    }
  });

  // Auto-scan full dst when preset is full_dst and a base file is present
  $effect(() => {
    const key = isFullDst && !!activeModelFile ? `${presetKey}|${activeModelFile}` : '';
    if (key && lastFullScanKey !== key) {
      lastFullScanKey = key;
      scanFullDst();
    }
  });

  const selectedPreset = $derived(presets.find((p) => p.key === presetKey) || null);
  const isScrDestructible = $derived(
    !!selectedPreset && /scr_destructible/i.test(selectedPreset.file || selectedPreset.key || '') && (selectedPreset.generator || '').toLowerCase() === 'template'
  );

  const isScrDestructibleA4 = $derived(
    !!selectedPreset && /scr_destructible_A4\.preset/i.test(selectedPreset.file || '') && (selectedPreset.generator || '').toLowerCase() === 'template'
  );

  const isFullDst = $derived(
    !!selectedPreset && /fulldst|full_dst/i.test(selectedPreset.file || selectedPreset.key || '') && (selectedPreset.generator || '').toLowerCase() === 'zone_fractal'
  );

  type ScrDebris = { guid: string; path: string };
  type ScrPhase = { pid: string; model_guid: string; model_path: string; debris: ScrDebris[] };
  let scrBaseGuid = $state<string>('');
  let scrBasePath = $state<string>('');
  let scrPhases = $state<ScrPhase[]>([]);
  let scrSel: { kind: 'base' } | { kind: 'phase'; pid: string } | { kind: 'debris'; pid: string; idx: number } | null = $state(null);
  let lastScrScanKey = $state<string>('');
  type ScrState = { base_guid: string; base_path: string; phases: ScrPhase[] };
  let scrByFile = $state<Record<string, ScrState>>({});

  type FullDstDebris = { guid: string; path: string };
  type FullDstZone = { part_id: string; debris: FullDstDebris[]; colliders: string[] };
  type FullDstState = { base_guid: string; base_path: string; v2_guid: string; v2_path: string; zones: FullDstZone[] };
  let fullByFile = $state<Record<string, FullDstState>>({});
  let fullBaseGuid = $state<string>('');
  let fullBasePath = $state<string>('');
  let fullV2Guid = $state<string>('');
  let fullV2Path = $state<string>('');
  let fullZones = $state<FullDstZone[]>([]);
  let fullNewRows = $state<Record<string, boolean>>({});
  let fullSel:
    | { kind: 'base' }
    | { kind: 'v2' }
    | { kind: 'zone'; part_id: string }
    | { kind: 'debris'; part_id: string; idx: number }
    | { kind: 'collider'; part_id: string; idx: number }
    | null = $state(null);
  let lastFullScanKey = $state<string>('');

  let fullExpanded = $state<Record<string, boolean>>({});
  let fullCatExpanded = $state<Record<string, boolean>>({});
  let scrExpanded = $state<Record<string, boolean>>({});

  function isFullZoneExpanded(pid: string): boolean {
    const k = String(pid || '');
    const v = fullExpanded[k];
    return v !== false;
  }

  function toggleFullZoneExpanded(pid: string) {
    const k = String(pid || '');
    const cur = isFullZoneExpanded(k);
    fullExpanded = { ...fullExpanded, [k]: !cur };
  }

  function fullCatKey(pid: string, cat: 'debris' | 'colliders'): string {
    return `${String(pid || '')}:${cat}`;
  }

  function isFullCatExpanded(pid: string, cat: 'debris' | 'colliders'): boolean {
    const k = fullCatKey(pid, cat);
    const v = fullCatExpanded[k];
    return v !== false;
  }

  function toggleFullCatExpanded(pid: string, cat: 'debris' | 'colliders') {
    const k = fullCatKey(pid, cat);
    const cur = isFullCatExpanded(pid, cat);
    fullCatExpanded = { ...fullCatExpanded, [k]: !cur };
  }

  function isScrPhaseExpanded(pid: string): boolean {
    const k = String(pid || '');
    const v = scrExpanded[k];
    return v !== false;
  }

  function toggleScrPhaseExpanded(pid: string) {
    const k = String(pid || '');
    const cur = isScrPhaseExpanded(k);
    scrExpanded = { ...scrExpanded, [k]: !cur };
  }

  let filesDropEl: HTMLElement | null = null;
  let filesIsDragOver = $state<boolean>(false);
  function updateFilesHoverFromCoords(ev: any) {
    try {
      const rect = filesDropEl?.getBoundingClientRect();
      if (!rect) { filesIsDragOver = false; return; }
      const rawX = ev?.payload?.position?.x ?? ev?.payload?.x ?? ev?.x;
      const rawY = ev?.payload?.position?.y ?? ev?.payload?.y ?? ev?.y;
      let x = Number(rawX), y = Number(rawY);
      if (!Number.isFinite(x) || !Number.isFinite(y)) { filesIsDragOver = false; return; }
      if (x > window.innerWidth * 2 || y > window.innerHeight * 2) {
        x = x - window.screenX; y = y - window.screenY;
      }
      filesIsDragOver = x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
    } catch { filesIsDragOver = false; }
  }

  function onFilesDragOver(e: DragEvent) { e.preventDefault(); chooseDropEffect(e); filesIsDragOver = true; }
  function onFilesDragEnter(e: DragEvent) { e.preventDefault(); chooseDropEffect(e); filesIsDragOver = true; }
  function onFilesDragLeave(e: DragEvent) { e.preventDefault(); filesIsDragOver = false; }
  function onFilesDrop(e: DragEvent) {
    e.preventDefault(); filesIsDragOver = false;
    const dt = e.dataTransfer; if (!dt) return;
    if (dt.files && dt.files.length > 0) {
      for (const f of Array.from(dt.files)) {
        const p: string = ((f as any)?.path as string) || f.name;
        if (p) addModelFileNormalized(p);
      }
      return;
    }
    const raw = extractWorkbenchText(dt); if (!raw) return;
    let parsed = parseWorkbenchDrag(raw) || (raw.includes('\n') ? parseWorkbenchDrag(raw.split('\n')[0]) : null);
    addModelFileNormalized(parsed || raw);
  }

  function loadScrFromCacheForActive() {
    const f = activeModelFile;
    if (!f) return;
    const s = scrByFile[f];
    if (s) {
      scrBaseGuid = s.base_guid || '';
      scrBasePath = s.base_path || '';
      scrPhases = Array.isArray(s.phases) ? s.phases : [];
      scrSel = null;
    } else {
      scrBaseGuid = '';
      scrBasePath = '';
      scrPhases = [];
      scrSel = null;
    }
  }

  function persistScrForActive() {
    const f = activeModelFile;
    if (!f) return;
    scrByFile = { ...scrByFile, [f]: { base_guid: scrBaseGuid, base_path: scrBasePath, phases: scrPhases } };
  }

  function persistFullForActive() {
    const f = activeModelFile;
    if (!f) return;
    fullByFile = {
      ...fullByFile,
      [f]: { base_guid: fullBaseGuid, base_path: fullBasePath, v2_guid: fullV2Guid, v2_path: fullV2Path, zones: fullZones }
    };
  }

  function fullRowKey(kind: 'zone' | 'debris' | 'collider', part_id: string, idx?: number): string {
    if (kind === 'zone') return `zone:${String(part_id || '')}`;
    if (typeof idx === 'number') return `${kind}:${String(part_id || '')}:${idx}`;
    return `${kind}:${String(part_id || '')}`;
  }

  function markFullRowNew(kind: 'zone' | 'debris' | 'collider', part_id: string, idx?: number) {
    const k = fullRowKey(kind, part_id, idx);
    fullNewRows = { ...fullNewRows, [k]: true };
  }

  function clearFullRowNew(kind: 'zone' | 'debris' | 'collider', part_id: string, idx?: number) {
    const k = fullRowKey(kind, part_id, idx);
    if (!fullNewRows[k]) return;
    const next = { ...fullNewRows };
    delete next[k];
    fullNewRows = next;
  }

  function isFullRowNew(kind: 'zone' | 'debris' | 'collider', part_id: string, idx?: number): boolean {
    const k = fullRowKey(kind, part_id, idx);
    return !!fullNewRows[k];
  }

  function clearFullNewRowsFor(kind: 'debris' | 'collider', part_id: string) {
    const pid = String(part_id || '');
    const prefix = `${kind}:${pid}:`;
    const next = { ...fullNewRows };
    for (const k of Object.keys(next)) {
      if (k.startsWith(prefix)) delete next[k];
    }
    fullNewRows = next;
  }

  async function pickXobMetaFromDialog(): Promise<{ guid: string; path: string } | null> {
    try {
      const pick = await open({ multiple: false, filters: [{ name: 'XOB', extensions: ['xob'] }] });
      if (typeof pick !== 'string' || !pick) return null;
      const meta = await invoke<{ guid: string; path: string }>('prefabdst_read_meta', { xobPath: pick });
      const g = String((meta as any)?.guid || '');
      const p = String((meta as any)?.path || pick);
      return { guid: g, path: p };
    } catch {
      return null;
    }
  }

  function nextFullPartId(): string {
    let max = 0;
    for (const z of fullZones) {
      const n = parseInt(String(z.part_id || ''), 10);
      if (Number.isFinite(n) && n > max) max = n;
    }
    return String(max + 1).padStart(2, '0');
  }

  function addFullZone() {
    if (fullZones.length >= 26) return;
    const pid = nextFullPartId();
    fullZones = [...fullZones, { part_id: pid, debris: [], colliders: [] }];
    zones = clampInt(fullZones.length || 1, 1, 26);
    markFullRowNew('zone', pid);
    fullSel = { kind: 'zone', part_id: pid };
    persistFullForActive();
  }

  function removeFullZone(part_id: string) {
    if (fullZones.length <= 1) return;
    const pid = String(part_id || '');
    fullZones = fullZones.filter((z) => z.part_id !== pid);
    zones = clampInt(fullZones.length || 1, 1, 26);
    if (fullSel && fullSel.kind === 'zone' && fullSel.part_id === pid) fullSel = null;
    if (fullSel && (fullSel.kind === 'debris' || fullSel.kind === 'collider') && fullSel.part_id === pid) fullSel = null;
    persistFullForActive();
  }

  function addFullDebris(part_id: string) {
    const pid = String(part_id || '');
    fullZones = fullZones.map((z) => {
      if (z.part_id !== pid) return z;
      return { ...z, debris: [...z.debris, { guid: '', path: '' }] };
    });
    const z = fullZones.find((x) => x.part_id === pid);
    const idx = (z?.debris.length ?? 1) - 1;
    markFullRowNew('debris', pid, idx);
    fullSel = { kind: 'debris', part_id: pid, idx };
    persistFullForActive();
  }

  function removeFullDebris(part_id: string) {
    const pid = String(part_id || '');
    const z = fullZones.find((x) => x.part_id === pid);
    const len = z?.debris.length ?? 0;
    if (len <= 0) return;
    const removeIdx = len - 1;
    fullZones = fullZones.map((zz) => (zz.part_id === pid ? { ...zz, debris: zz.debris.slice(0, -1) } : zz));
    if (fullSel && fullSel.kind === 'debris' && fullSel.part_id === pid && fullSel.idx >= removeIdx) fullSel = null;
    clearFullNewRowsFor('debris', pid);
    persistFullForActive();
  }

  function removeFullDebrisAt(part_id: string, idx: number) {
    const pid = String(part_id || '');
    const i = clampInt(idx, 0, 999999);
    fullZones = fullZones.map((z) => (z.part_id === pid ? { ...z, debris: z.debris.filter((_, di) => di !== i) } : z));
    if (fullSel && fullSel.kind === 'debris' && fullSel.part_id === pid) {
      if (fullSel.idx === i) fullSel = null;
      else if (fullSel.idx > i) fullSel = { kind: 'debris', part_id: pid, idx: fullSel.idx - 1 };
    }
    clearFullNewRowsFor('debris', pid);
    persistFullForActive();
  }

  function addFullCollider(part_id: string) {
    const pid = String(part_id || '');
    fullZones = fullZones.map((z) => {
      if (z.part_id !== pid) return z;
      return { ...z, colliders: [...z.colliders, ''] };
    });
    const z = fullZones.find((x) => x.part_id === pid);
    const idx = (z?.colliders.length ?? 1) - 1;
    markFullRowNew('collider', pid, idx);
    fullSel = { kind: 'collider', part_id: pid, idx };
    persistFullForActive();
  }

  function removeFullCollider(part_id: string) {
    const pid = String(part_id || '');
    const z = fullZones.find((x) => x.part_id === pid);
    const len = z?.colliders.length ?? 0;
    if (len <= 0) return;
    const removeIdx = len - 1;
    fullZones = fullZones.map((zz) => (zz.part_id === pid ? { ...zz, colliders: zz.colliders.slice(0, -1) } : zz));
    if (fullSel && fullSel.kind === 'collider' && fullSel.part_id === pid && fullSel.idx >= removeIdx) fullSel = null;
    clearFullNewRowsFor('collider', pid);
    persistFullForActive();
  }

  function removeFullColliderAt(part_id: string, idx: number) {
    const pid = String(part_id || '');
    const i = clampInt(idx, 0, 999999);
    fullZones = fullZones.map((z) => (z.part_id === pid ? { ...z, colliders: z.colliders.filter((_, ci) => ci !== i) } : z));
    if (fullSel && fullSel.kind === 'collider' && fullSel.part_id === pid) {
      if (fullSel.idx === i) fullSel = null;
      else if (fullSel.idx > i) fullSel = { kind: 'collider', part_id: pid, idx: fullSel.idx - 1 };
    }
    clearFullNewRowsFor('collider', pid);
    persistFullForActive();
  }

  function onFullColliderText(part_id: string, idx: number, next: string) {
    const pid = String(part_id || '');
    const i = clampInt(idx, 0, 999999);
    const v = String(next ?? '');
    fullZones = fullZones.map((z) => (z.part_id === pid ? { ...z, colliders: z.colliders.map((c, ci) => (ci === i ? v : c)) } : z));
    clearFullRowNew('collider', pid, i);
    persistFullForActive();
  }

  async function onFullTreePick(kind: 'debris' | 'collider', part_id: string, idx: number) {
    const pid = String(part_id || '');
    if (!pid || idx < 0) return;
    const meta = await pickXobMetaFromDialog();
    if (!meta) return;

    if (kind === 'debris') {
      fullZones = fullZones.map((z) =>
        z.part_id === pid
          ? { ...z, debris: z.debris.map((d, i) => (i === idx ? { guid: meta.guid, path: meta.path } : d)) }
          : z
      );
      clearFullRowNew('debris', pid, idx);
      fullSel = { kind: 'debris', part_id: pid, idx };
    } else {
      fullZones = fullZones.map((z) => (z.part_id === pid ? { ...z, colliders: z.colliders.map((c, i) => (i === idx ? meta.path : c)) } : z));
      clearFullRowNew('collider', pid, idx);
      fullSel = { kind: 'collider', part_id: pid, idx };
    }
    persistFullForActive();
  }

  async function scanDst() {
    if (!activeModelFile) {
      pushLog('WARN', 'Pick a base .xob first.');
      return;
    }
    try {
      const res = await invoke<{ base_guid: string; base_path: string; phases: ScrPhase[] }>('prefabdst_scan_dst', {
        xobPath: activeModelFile
      });
      const r: any = res as any;
      scrBaseGuid = String(r.base_guid || '');
      scrBasePath = String(r.base_path || '');
      scrPhases = Array.isArray(r.phases) ? (r.phases as ScrPhase[]) : [];

      if (isScrDestructibleA4 && scrPhases.length === 0) {
        const baseMeta = await tryReadXobMeta(activeModelFile);
        if (baseMeta) {
          scrBaseGuid = baseMeta.guid;
          scrBasePath = baseMeta.path;
        }

        const ruinAbs = await resolveSiblingRuinXobIfAny(activeModelFile);
        if (ruinAbs && ruinAbs !== normalizeXobPath(activeModelFile)) {
          const ruinMeta = await tryReadXobMeta(ruinAbs);
          if (ruinMeta) {
            scrPhases = [{ pid: '01', model_guid: ruinMeta.guid, model_path: ruinMeta.path, debris: [] }];
          }
        }
        if (scrPhases.length) {
          pushLog('INFO', 'Auto-filled dst_01 from sibling ruin/ruined .xob');
        } else {
          pushLog('WARN', 'No _Ruin/_Ruined .xob found for this base file.');
        }
      }

      scrSel = null;
      persistScrForActive();
      pushLog('INFO', `Scan dst done. Found ${scrPhases.length} phase(s).`);
    } catch (e: any) {
      console.error(e);
      pushLog('ERROR', String(e?.toString?.() || e || 'Scan dst failed'));
    }
  }

  async function scanFullDst() {
    if (!activeModelFile) {
      pushLog('WARN', 'Pick a base .xob first.');
      return;
    }
    try {
      const res = await invoke<FullDstState>('prefabdst_scan_full_dst', {
        xobPath: activeModelFile
      });
      const r: any = res as any;
      fullBaseGuid = String(r.base_guid || '');
      fullBasePath = String(r.base_path || '');
      fullV2Guid = String(r.v2_guid || '');
      fullV2Path = String(r.v2_path || '');
      fullZones = Array.isArray(r.zones) ? (r.zones as FullDstZone[]) : [];
      zones = clampInt(fullZones.length || 1, 1, 26);
      fullSel = null;
      persistFullForActive();
      pushLog('INFO', `Scan full dst done. Found ${fullZones.length} zone(s).`);
    } catch (e: any) {
      console.error(e);
      pushLog('ERROR', String(e?.toString?.() || e || 'Scan full dst failed'));
    }
  }

  function removeScrSelected() {
    const sel = scrSel;
    if (!sel) return;
    if (sel.kind === 'phase') {
      scrPhases = scrPhases.filter((p) => p.pid !== sel.pid);
    } else if (sel.kind === 'debris') {
      scrPhases = scrPhases.map((p) => (p.pid === sel.pid ? { ...p, debris: p.debris.filter((_, i) => i !== sel.idx) } : p));
    } else {
      // base
      scrBaseGuid = '';
      scrBasePath = '';
    }
    scrSel = null;
    persistScrForActive();
  }

  function nextPid(): string {
    let max = 0;
    for (const ph of scrPhases) {
      const n = parseInt(ph.pid, 10);
      if (Number.isFinite(n) && n > max) max = n;
    }
    const nn = String(max + 1).padStart(2, '0');
    return nn;
  }

  function sortPhasesInPlace() {
    scrPhases = [...scrPhases].sort((a, b) => parseInt(a.pid, 10) - parseInt(b.pid, 10));
  }

  function ensurePhase(pid: string) {
    if (!scrPhases.some((p) => p.pid === pid)) {
      scrPhases = [...scrPhases, { pid, model_guid: '', model_path: '', debris: [] }];
      sortPhasesInPlace();
    }
  }

  async function addDst() {
    const pid = nextPid();
    ensurePhase(pid);
    scrSel = { kind: 'phase', pid };
    pushLog('INFO', `Added phase dst_${pid}`);
    persistScrForActive();
  }

  async function addDbr() {
    let pid: string | null = null;
    if (scrSel && scrSel.kind === 'phase') pid = scrSel.pid;
    else if (scrSel && scrSel.kind === 'debris') pid = scrSel.pid;
    else if (scrPhases.length) pid = scrPhases[0].pid;
    if (!pid) {
      pushLog('WARN', 'Select a phase to add debris.');
      return;
    }
    try {
      const pick = await open({ multiple: false, filters: [{ name: 'XOB', extensions: ['xob'] }] });
      if (typeof pick !== 'string' || !pick) return;
      const meta = await invoke<{ guid: string; path: string }>('prefabdst_read_meta', { xobPath: pick });
      const g = (meta as any)?.guid || '';
      const p = (meta as any)?.path || pick;
      scrPhases = scrPhases.map((ph) => (ph.pid === pid ? { ...ph, debris: [...ph.debris, { guid: g, path: p }] } : ph));
      const idx = scrPhases.find((ph) => ph.pid === pid)?.debris.length ?? 1;
      scrSel = { kind: 'debris', pid, idx: idx - 1 };
      pushLog('INFO', `Added debris to dst_${pid}`);
      persistScrForActive();
    } catch (e: any) {
      console.error(e);
      pushLog('ERROR', 'Add debris failed');
    }
  }

  async function onTreeDblClick(kind: 'base' | 'phase' | 'debris', pid?: string, idx?: number) {
    try {
      const pick = await open({ multiple: false, filters: [{ name: 'XOB', extensions: ['xob'] }] });
      if (typeof pick !== 'string' || !pick) return;
      const meta = await invoke<{ guid: string; path: string }>('prefabdst_read_meta', { xobPath: pick });
      const g = (meta as any)?.guid || '';
      const p = (meta as any)?.path || pick;
      if (kind === 'base') {
        scrBaseGuid = g;
        scrBasePath = p;
        scrSel = { kind: 'base' };
        pushLog('INFO', 'Updated BASE from file');
      } else if (kind === 'phase' && pid) {
        scrPhases = scrPhases.map((ph) => (ph.pid === pid ? { ...ph, model_guid: g, model_path: p } : ph));
        scrSel = { kind: 'phase', pid };
        pushLog('INFO', `Updated phase dst_${pid}`);
      } else if (kind === 'debris' && pid != null && typeof idx === 'number') {
        scrPhases = scrPhases.map((ph) =>
          ph.pid === pid
            ? { ...ph, debris: ph.debris.map((d, i) => (i === idx ? { guid: g, path: p } : d)) }
            : ph
        );
        scrSel = { kind: 'debris', pid, idx };
        pushLog('INFO', `Updated debris in dst_${pid}`);
      }
      persistScrForActive();
    } catch (e: any) {
      console.error(e);
      pushLog('ERROR', 'Update from file failed');
    }
  }

  function pushLog(level: LogLine['level'], text: string) {
    const id = `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    logLines = [...logLines, { id, level, text }].slice(-500);
  }

  function clampInt(n: number, min: number, max: number) {
    const v = Math.round(Number.isFinite(n) ? n : min);
    return Math.max(min, Math.min(max, v));
  }

  async function pickModelFiles() {
    try {
      const selection = await open({
        multiple: true,
        filters: [{ name: 'XOB', extensions: ['xob'] }]
      });
      if (Array.isArray(selection)) {
        const picked = isScrDestructibleA4
          ? await Promise.all(selection.map((p) => resolveSiblingRuinXobIfAny(p)))
          : selection.map((p) => normalizeXobPath(p));
        const next = Array.from(new Set([...modelFiles, ...picked]));
        modelFiles = next;
        pushLog('INFO', `Picked ${selection.length} file(s).`);
        if (!saveFolder && modelFiles.length > 0) {
          saveFolder = dirname(modelFiles[0]);
        }
      } else if (typeof selection === 'string') {
        const resolved = isScrDestructibleA4
          ? await resolveSiblingRuinXobIfAny(selection)
          : normalizeXobPath(selection);
        modelFiles = Array.from(new Set([...modelFiles, resolved]));
        pushLog('INFO', 'Picked 1 file.');
        if (!saveFolder && modelFiles.length > 0) {
          saveFolder = dirname(modelFiles[0]);
        }
      }
    } catch (e) {
      console.error(e);
      pushLog('ERROR', 'Pick file cancelled/failed.');
    }
  }

  async function pickSaveFolder() {
    try {
      const selection = await open({ directory: true, multiple: false });
      if (typeof selection === 'string') {
        saveFolder = selection;
        pushLog('INFO', 'Picked output folder.');
      }
    } catch (e) {
      console.error(e);
      pushLog('ERROR', 'Pick folder cancelled/failed.');
    }
  }

  function removeModelFile(p: string) {
    const idx = modelFiles.findIndex((x) => x === p);
    modelFiles = modelFiles.filter((x) => x !== p);
    // drop cached state
    const cache = { ...scrByFile }; delete cache[p]; scrByFile = cache;
    const cacheFull = { ...fullByFile }; delete cacheFull[p]; fullByFile = cacheFull;
    if (activeModelIndex >= modelFiles.length) activeModelIndex = Math.max(0, modelFiles.length - 1);
    loadScrFromCacheForActive();
    loadFullFromCacheForActive();
  }

  function clearModelFiles() {
    modelFiles = [];
    saveFolder = '';
    lastScrScanKey = '';
    lastFullScanKey = '';
    scrBaseGuid = '';
    scrBasePath = '';
    scrPhases = [];
    scrSel = null;
    scrByFile = {};
    fullBaseGuid = '';
    fullBasePath = '';
    fullV2Guid = '';
    fullV2Path = '';
    fullZones = [];
    fullSel = null;
    fullByFile = {};
    activeModelIndex = 0;
    pushLog('INFO', 'Cleared file list.');
  }

  async function onBuild() {
    if (isPresetLoading) {
      pushLog('WARN', 'Presets are still loading...');
      return;
    }
    if (isBuilding) return;
    pushLog('INFO', `Project: ${projectKey}, Preset: ${presetKey}` + (isScrDestructible ? '' : `, Zones: ${zones}`));
    if (!modelFiles.length) {
      pushLog('WARN', 'No model files selected.');
      return;
    }
    if (!saveFolder) {
      pushLog('WARN', 'No output folder selected.');
      return;
    }
    if (!selectedPreset) {
      pushLog('ERROR', 'No preset selected.');
      return;
    }
    const gen = (selectedPreset.generator || '').toLowerCase();
    if (gen !== 'zone_fractal' && gen !== 'template') {
      pushLog('ERROR', `Unsupported generator: ${selectedPreset.generator || '(none)'}`);
      return;
    }

    isBuilding = true;
    try {
      pushLog('INFO', `Loading preset file: ${selectedPreset.file}`);
      const r = await fetch(`/presets/${encodeURIComponent(selectedPreset.file)}`, { cache: 'no-store' });
      if (!r.ok) {
        pushLog('ERROR', 'Failed to fetch preset file.');
        return;
      }
      const presetText = await r.text();
      pushLog('INFO', 'Invoking prefabdst_build...');
      const scrOverride = isScrDestructible
        ? { base_guid: scrBaseGuid, base_path: scrBasePath, phases: scrPhases }
        : null;
      let allOut: string[] = [];
      if (gen === 'template') {
        for (const f of modelFiles) {
          const per = scrByFile[f] || null;
          pushLog('INFO', `Building (template) for ${f.split('\\').pop()}`);
          const res = await invoke<{ out_paths: string[] }>('prefabdst_build', {
            presetFile: selectedPreset.file,
            presetText: presetText,
            zones,
            hpZone: hpZone,
            debrisMass: debrisMass,
            modelFiles: [f],
            saveFolder: saveFolder,
            scrOverride: per
          });
          const out = (res && Array.isArray((res as any).out_paths)) ? ((res as any).out_paths as string[]) : [];
          allOut.push(...out);
        }
      } else {
        const res = await invoke<{ out_paths: string[] }>('prefabdst_build', {
          presetFile: selectedPreset.file,
          presetText: presetText,
          zones,
          hpZone: hpZone,
          debrisMass: debrisMass,
          modelFiles: modelFiles,
          saveFolder: saveFolder,
          scrOverride
        });
        const out = (res && Array.isArray((res as any).out_paths)) ? ((res as any).out_paths as string[]) : [];
        allOut.push(...out);
      }
      if (allOut.length) {
        pushLog('INFO', `Generated ${allOut.length} file(s).`);
        for (const p of allOut) pushLog('INFO', `Saved: ${p}`);
      } else {
        pushLog('WARN', 'Build finished but no output paths returned.');
      }
    } catch (e: any) {
      console.error(e);
      pushLog('ERROR', String(e?.toString?.() || e || 'Build failed'));
    } finally {
      isBuilding = false;
    }
  }
</script>

<div class="root">
  <div class="header">
    <div class="title">{title}</div>
    <div class="subtitle">สร้าง Prefab DST โดยใช้ preset</div>
  </div>

  <div class="form">
    <div class="row">
      <label for="prefabdst-project">Project:</label>
      <select id="prefabdst-project" bind:value={projectKey} disabled={isPresetLoading || isBuilding}>
        {#each projects as p (p.key)}
          <option value={p.key}>{p.label}</option>
        {/each}
      </select>
    </div>

    <div class="row">
      <label for="prefabdst-preset">Preset:</label>
      <select id="prefabdst-preset" bind:value={presetKey} disabled={isPresetLoading || isBuilding || filteredPresets.length === 0}>
        {#each filteredPresets as p (p.key)}
          <option value={p.key}>{p.label}</option>
        {/each}
      </select>
    </div>

    {#if !isScrDestructible}
      <div class="row">
        <label for="prefabdst-zones">Zones:</label>
        <input
          id="prefabdst-zones"
          type="number"
          min="1"
          max="26"
          step="1"
          bind:value={zones}
          oninput={() => (zones = clampInt(zones, 1, 26))}
          disabled={isBuilding}
        />
      </div>
    {/if}

    <div class="row">
      <label for="prefabdst-model-files">model files:</label>
      <div class="files">
        <input id="prefabdst-model-files" class="visually-hidden" type="text" readonly value={String(modelFiles.length)} />
        <div
          class="files-box"
          role="region"
          aria-label="Selected files drop zone"
          bind:this={filesDropEl}
          ondragover={onFilesDragOver}
          ondragenter={onFilesDragEnter}
          ondragleave={onFilesDragLeave}
          ondrop={onFilesDrop}
        >
          <div class="drop-mask {filesIsDragOver ? 'drag' : ''}"></div>
          {#if modelFiles.length === 0}
            <div class="files-empty">No file selected</div>
          {:else}
            <div class="chips">
              {#each modelFiles as f, i (f)}
                <div
                  class="chip {i===activeModelIndex ? 'active' : ''}"
                  title={f}
                  role="button"
                  tabindex="0"
                  aria-pressed={i===activeModelIndex}
                  onclick={() => { activeModelIndex = i; loadScrFromCacheForActive(); loadFullFromCacheForActive(); lastScrScanKey=''; lastFullScanKey=''; }}
                  onkeydown={(e) => { if (e.key==='Enter' || e.key===' ') { e.preventDefault(); activeModelIndex = i; loadScrFromCacheForActive(); loadFullFromCacheForActive(); lastScrScanKey=''; lastFullScanKey=''; } }}
                >
                  <span class="chip-name">{f.split('\\').pop()}</span>
                  <button class="chip-x" type="button" onclick={(e) => { e.stopPropagation(); removeModelFile(f); }} aria-label="Remove">×</button>
                </div>
              {/each}
            </div>
          {/if}
        </div>
        <div class="files-actions">
          <button class="btn" type="button" onclick={pickModelFiles} disabled={isBuilding}>Pick .xob(s)</button>
          <button class="btn" type="button" onclick={clearModelFiles} disabled={isBuilding || modelFiles.length === 0}>Clear</button>
        </div>
      </div>
    </div>

    <div class="row">
      <label for="prefabdst-savefolder">save to folder:</label>
      <div class="path">
        <input id="prefabdst-savefolder" class="path-input" type="text" bind:value={saveFolder} placeholder="Pick output folder..." disabled={isBuilding} />
        <button class="btn" type="button" onclick={pickSaveFolder} disabled={isBuilding}>Pick folder...</button>
      </div>
    </div>

    <div class="row build-row">
      <div></div>
      <button class="build" type="button" onclick={onBuild} disabled={isBuilding}>
        Build prefab
      </button>
    </div>
  </div>

  {#if !isScrDestructible}
    <div class="section-title">Full DST settings</div>
    <div class="settings">
      <div class="row">
        <label for="prefabdst-hpzone">HP Zone:</label>
        <input id="prefabdst-hpzone" type="number" min="1" max="9999" step="1" bind:value={hpZone} oninput={() => (hpZone = clampInt(hpZone, 1, 9999))} disabled={isBuilding} />
      </div>
      <div class="row">
        <label for="prefabdst-debrismass">Debris Mass (m_fMass):</label>
        <input
          id="prefabdst-debrismass"
          type="number"
          min="0"
          max="999999"
          step="1"
          bind:value={debrisMass}
          oninput={() => (debrisMass = clampInt(debrisMass, 0, 999999))}
          disabled={isBuilding}
        />
      </div>
    </div>
  {/if}

  {#if isFullDst}
    <div class="section-title">tree (full_dst)</div>
    <div class="scr-controls">
      <div class="left">
        <label><input type="checkbox" checked disabled /> Scan from base xob + sibling *_V2_dst.xob.meta + Dst/ debris</label>
      </div>
      <div class="right">
        <button class="btn" type="button" onclick={scanFullDst} disabled={isBuilding || modelFiles.length === 0}>Scan full dst</button>
        <button class="btn" type="button" onclick={addFullZone} disabled={isBuilding || fullZones.length >= 26}>+ Zone</button>
      </div>
    </div>

    <div class="scr-tree" role="tree">
      <div class="tree-row header" aria-hidden="true">
        <div class="col name"><strong>item</strong></div>
        <div class="col path"><strong>value</strong></div>
        <div class="col actions"><strong></strong></div>
      </div>
      <div
        class="tree-row level-0 {fullSel && fullSel.kind==='base' ? 'sel' : ''}"
        role="treeitem"
        tabindex="0"
        aria-selected={!!(fullSel && fullSel.kind==='base')}
        onclick={() => (fullSel = { kind: 'base' })}
        onkeydown={(e) => { if (e.key==='Enter'||e.key===' ') { e.preventDefault(); fullSel = { kind: 'base' }; } }}
      >
        <div class="col name">Building {fullBaseGuid ? `{${fullBaseGuid}}` : '{ }'}</div>
        <div class="col path">{fullBasePath || '(not found)'}</div>
        <div class="col actions"></div>
      </div>
      <div
        class="tree-row level-0 {fullSel && fullSel.kind==='v2' ? 'sel' : ''}"
        role="treeitem"
        tabindex="0"
        aria-selected={!!(fullSel && fullSel.kind==='v2')}
        onclick={() => (fullSel = { kind: 'v2' })}
        onkeydown={(e) => { if (e.key==='Enter'||e.key===' ') { e.preventDefault(); fullSel = { kind: 'v2' }; } }}
      >
        <div class="col name">Building ruin (v2_dst) {fullV2Guid ? `{${fullV2Guid}}` : '{ }'}</div>
        <div class="col path">{fullV2Path || '(not found)'}</div>
        <div class="col actions"></div>
      </div>

      {#each fullZones as z (z.part_id)}
        <div
          class="tree-row level-0 {fullSel && fullSel.kind==='zone' && fullSel.part_id===z.part_id ? 'sel' : ''}"
          role="treeitem"
          tabindex="0"
          aria-selected={!!(fullSel && fullSel.kind==='zone' && fullSel.part_id===z.part_id)}
          onclick={() => (fullSel = { kind: 'zone', part_id: z.part_id })}
          onkeydown={(e) => { if (e.key==='Enter'||e.key===' ') { e.preventDefault(); fullSel = { kind: 'zone', part_id: z.part_id }; } }}
        >
          <div class="col name">
            <button
              class="expander"
              type="button"
              aria-label={isFullZoneExpanded(z.part_id) ? 'Collapse' : 'Expand'}
              onclick={(e) => { e.stopPropagation(); toggleFullZoneExpanded(z.part_id); }}
              onkeydown={(e) => { if (e.key==='Enter' || e.key===' ') { e.preventDefault(); e.stopPropagation(); toggleFullZoneExpanded(z.part_id); } }}
            >
              {isFullZoneExpanded(z.part_id) ? '▾' : '▸'}
            </button>
            {#if isFullRowNew('zone', z.part_id)}
              <span class="folder-ico" aria-hidden="true">
                <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
                </svg>
              </span>
            {/if}
            Zone {z.part_id}
          </div>
          <div class="col path">debris: {z.debris.length}, collider: {z.colliders.length}</div>
          <div class="col actions">
            <button class="icon-btn" type="button" title="Add Zone" onclick={(e) => { e.stopPropagation(); addFullZone(); }} disabled={isBuilding || fullZones.length >= 26}>+</button>
            <button class="icon-btn" type="button" title="Remove Zone" onclick={(e) => { e.stopPropagation(); removeFullZone(z.part_id); }} disabled={isBuilding || fullZones.length <= 1}>-</button>
            <button class="icon-btn danger" type="button" title="Delete Zone" onclick={(e) => { e.stopPropagation(); removeFullZone(z.part_id); }} disabled={isBuilding || fullZones.length <= 1}>Del</button>
          </div>
        </div>

        {#if isFullZoneExpanded(z.part_id)}
          <div
            class="tree-row category level-1"
            role="treeitem"
            tabindex="0"
            aria-selected={false}
            aria-expanded={isFullCatExpanded(z.part_id, 'debris')}
            onclick={() => toggleFullCatExpanded(z.part_id, 'debris')}
            onkeydown={(e) => { if (e.key==='Enter'||e.key===' ') { e.preventDefault(); toggleFullCatExpanded(z.part_id, 'debris'); } }}
          >
            <div class="col name">
              <button
                class="expander"
                type="button"
                aria-label={isFullCatExpanded(z.part_id, 'debris') ? 'Collapse' : 'Expand'}
                onclick={(e) => { e.stopPropagation(); toggleFullCatExpanded(z.part_id, 'debris'); }}
                onkeydown={(e) => { if (e.key==='Enter' || e.key===' ') { e.preventDefault(); e.stopPropagation(); toggleFullCatExpanded(z.part_id, 'debris'); } }}
              >
                {isFullCatExpanded(z.part_id, 'debris') ? '▾' : '▸'}
              </button>
              Debris
            </div>
            <div class="col path">{z.debris.length}</div>
            <div class="col actions">
              <button class="icon-btn" type="button" title="Add Debris" onclick={(e) => { e.stopPropagation(); addFullDebris(z.part_id); }} disabled={isBuilding}>+</button>
              <button class="icon-btn" type="button" title="Remove Debris" onclick={(e) => { e.stopPropagation(); removeFullDebris(z.part_id); }} disabled={isBuilding || z.debris.length === 0}>-</button>
            </div>
          </div>

          {#if isFullCatExpanded(z.part_id, 'debris')}
            {#each z.debris as db, i (`${z.part_id}-d-${i}`)}
              <div
                class="tree-row debris level-2 {fullSel && fullSel.kind==='debris' && fullSel.part_id===z.part_id && fullSel.idx===i ? 'sel' : ''}"
                role="treeitem"
                tabindex="0"
                aria-selected={!!(fullSel && fullSel.kind==='debris' && fullSel.part_id===z.part_id && fullSel.idx===i)}
                onclick={() => (fullSel = { kind: 'debris', part_id: z.part_id, idx: i })}
                onkeydown={(e) => { if (e.key==='Enter'||e.key===' ') { e.preventDefault(); fullSel = { kind: 'debris', part_id: z.part_id, idx: i }; } }}
                ondblclick={() => onFullTreePick('debris', z.part_id, i)}
              >
                <div class="col name">
                  {#if isFullRowNew('debris', z.part_id, i) || (!db.path && !db.guid)}
                    <button class="folder-btn" type="button" title="Pick file" onclick={(e) => { e.stopPropagation(); onFullTreePick('debris', z.part_id, i); }}>
                      <span class="folder-ico" aria-hidden="true">
                        <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                          <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
                        </svg>
                      </span>
                    </button>
                  {/if}
                  debris dbr_{String(i + 1).padStart(2, '0')} {db.guid ? `{${db.guid}}` : '{ }'}
                </div>
                <div class="col path">{db.path}</div>
                <div class="col actions">
                  <button class="icon-btn danger" type="button" title="Delete" onclick={(e) => { e.stopPropagation(); removeFullDebrisAt(z.part_id, i); }} disabled={isBuilding}>Del</button>
                </div>
              </div>
            {/each}
          {/if}

          <div
            class="tree-row category level-1"
            role="treeitem"
            tabindex="0"
            aria-selected={false}
            aria-expanded={isFullCatExpanded(z.part_id, 'colliders')}
            onclick={() => toggleFullCatExpanded(z.part_id, 'colliders')}
            onkeydown={(e) => { if (e.key==='Enter'||e.key===' ') { e.preventDefault(); toggleFullCatExpanded(z.part_id, 'colliders'); } }}
          >
            <div class="col name">
              <button
                class="expander"
                type="button"
                aria-label={isFullCatExpanded(z.part_id, 'colliders') ? 'Collapse' : 'Expand'}
                onclick={(e) => { e.stopPropagation(); toggleFullCatExpanded(z.part_id, 'colliders'); }}
                onkeydown={(e) => { if (e.key==='Enter' || e.key===' ') { e.preventDefault(); e.stopPropagation(); toggleFullCatExpanded(z.part_id, 'colliders'); } }}
              >
                {isFullCatExpanded(z.part_id, 'colliders') ? '▾' : '▸'}
              </button>
              Colliders
            </div>
            <div class="col path">{z.colliders.length}</div>
            <div class="col actions">
              <button class="icon-btn" type="button" title="Add Collider" onclick={(e) => { e.stopPropagation(); addFullCollider(z.part_id); }} disabled={isBuilding}>+</button>
              <button class="icon-btn" type="button" title="Remove Collider" onclick={(e) => { e.stopPropagation(); removeFullCollider(z.part_id); }} disabled={isBuilding || z.colliders.length === 0}>-</button>
            </div>
          </div>

          {#if isFullCatExpanded(z.part_id, 'colliders')}
            {#each z.colliders as c, j (`${z.part_id}-c-${j}`)}
              <div
                class="tree-row debris level-2 {fullSel && fullSel.kind==='collider' && fullSel.part_id===z.part_id && fullSel.idx===j ? 'sel' : ''}"
                role="treeitem"
                tabindex="0"
                aria-selected={!!(fullSel && fullSel.kind==='collider' && fullSel.part_id===z.part_id && fullSel.idx===j)}
                onclick={() => (fullSel = { kind: 'collider', part_id: z.part_id, idx: j })}
                onkeydown={(e) => { if (e.key==='Enter'||e.key===' ') { e.preventDefault(); fullSel = { kind: 'collider', part_id: z.part_id, idx: j }; } }}
              >
                <div class="col name">collider {j + 1}</div>
                <div class="col path">
                  <input
                    class="inline-input"
                    type="text"
                    value={c}
                    placeholder="Collider text..."
                    disabled={isBuilding}
                    onfocus={() => (fullSel = { kind: 'collider', part_id: z.part_id, idx: j })}
                    oninput={(e) => onFullColliderText(z.part_id, j, (e.currentTarget as HTMLInputElement).value)}
                  />
                </div>
                <div class="col actions">
                  <button class="icon-btn danger" type="button" title="Delete" onclick={(e) => { e.stopPropagation(); removeFullColliderAt(z.part_id, j); }} disabled={isBuilding}>Del</button>
                </div>
              </div>
            {/each}
          {/if}
        {/if}
      {/each}
    </div>
  {/if}

  {#if isScrDestructible}
    <div class="section-title">phase/debris (scr_destructible)</div>
    <div class="scr-controls">
      <div class="left">
        <label><input type="checkbox" checked disabled /> Case-insensitive (dst/Dst)</label>
      </div>
      <div class="right">
        <button class="btn" type="button" onclick={scanDst} disabled={isBuilding || modelFiles.length === 0}>Scan dst</button>
        <button class="btn" type="button" onclick={addDst} title="Add phase (dst_XX)" disabled={isBuilding}>Add dst</button>
        <button class="btn" type="button" onclick={addDbr} title="Add debris (dbr)" disabled={isBuilding || scrPhases.length === 0}>Add dbr</button>
        <button class="btn" type="button" onclick={() => moveDebris(-1)} disabled={!scrSel || scrSel.kind!=='debris'} title="Move debris up">Up</button>
        <button class="btn" type="button" onclick={() => moveDebris(1)} disabled={!scrSel || scrSel.kind!=='debris'} title="Move debris down">Down</button>
        <button class="btn" type="button" onclick={removeScrSelected} disabled={!scrSel}>Remove</button>
      </div>
    </div>

    <div class="scr-tree" role="tree">
      <div class="tree-row header" aria-hidden="true">
        <div class="col name"><strong>phase/debris</strong></div>
        <div class="col path"><strong>Path file</strong></div>
      </div>
      <div class="tree-row level-0 {scrSel && scrSel.kind==='base' ? 'sel' : ''}" role="treeitem" tabindex="0" aria-selected={!!(scrSel && scrSel.kind==='base')} onclick={() => (scrSel = { kind: 'base' })} ondblclick={() => onTreeDblClick('base')} onkeydown={(e) => { if (e.key==='Enter'||e.key===' ') { e.preventDefault(); scrSel = { kind: 'base' }; } }}>
        <div class="col name">BASE {scrBaseGuid ? `{${scrBaseGuid}}` : '{ }'}</div>
        <div class="col path">{scrBasePath || '(not found)'}</div>
      </div>
      {#each scrPhases as ph (ph.pid)}
        <div class="tree-row level-0 {scrSel && scrSel.kind==='phase' && scrSel.pid===ph.pid ? 'sel' : ''}" role="treeitem" tabindex="0" aria-selected={!!(scrSel && scrSel.kind==='phase' && scrSel.pid===ph.pid)} onclick={() => (scrSel = { kind: 'phase', pid: ph.pid })} ondblclick={() => onTreeDblClick('phase', ph.pid)} onkeydown={(e) => { if (e.key==='Enter'||e.key===' ') { e.preventDefault(); scrSel = { kind: 'phase', pid: ph.pid }; } }}>
          <div class="col name">
            <button
              class="expander"
              type="button"
              aria-label={isScrPhaseExpanded(ph.pid) ? 'Collapse' : 'Expand'}
              onclick={(e) => { e.stopPropagation(); toggleScrPhaseExpanded(ph.pid); }}
              onkeydown={(e) => { if (e.key==='Enter' || e.key===' ') { e.preventDefault(); e.stopPropagation(); toggleScrPhaseExpanded(ph.pid); } }}
            >
              {isScrPhaseExpanded(ph.pid) ? '▾' : '▸'}
            </button>
            dst_{ph.pid} {ph.model_guid ? `{${ph.model_guid}}` : '{ }'}
          </div>
          <div class="col path">{ph.model_path}</div>
        </div>
        {#if isScrPhaseExpanded(ph.pid)}
          {#each ph.debris as db, i (`${ph.pid}-${i}`)}
            <div class="tree-row debris level-1 {scrSel && scrSel.kind==='debris' && scrSel.pid===ph.pid && scrSel.idx===i ? 'sel' : ''}" role="treeitem" tabindex="0" aria-selected={!!(scrSel && scrSel.kind==='debris' && scrSel.pid===ph.pid && scrSel.idx===i)} onclick={() => (scrSel = { kind: 'debris', pid: ph.pid, idx: i })} ondblclick={() => onTreeDblClick('debris', ph.pid, i)} onkeydown={(e) => { if (e.key==='Enter'||e.key===' ') { e.preventDefault(); scrSel = { kind: 'debris', pid: ph.pid, idx: i }; } }}>
              <div class="col name">dbr_{String(i + 1).padStart(2, '0')} {db.guid ? `{${db.guid}}` : '{ }'}</div>
              <div class="col path">{db.path}</div>
            </div>
          {/each}
        {/if}
      {/each}
    </div>
  {/if}

  <div class="log" aria-label="Log">
    <div class="log-inner">
      {#each logLines as l (l.id)}
        <div class="log-line" data-level={l.level}>
          <span class="log-level">[{l.level}]</span>
          <span class="log-text">{l.text}</span>
        </div>
      {/each}
    </div>
  </div>
</div>

<style>
  .root {
    width: 100%;
    height: 100%;
    display: flex;
    flex-direction: column;
    gap: 14px;
    min-height: 0;
  }

  .header {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .title {
    font-size: 18px;
    font-weight: 700;
    color: rgba(255, 255, 255, 0.92);
  }

  :global(body.theme-light) .title {
    color: rgba(17, 17, 17, 0.92);
  }

  .subtitle {
    font-size: 13px;
    color: rgba(255, 255, 255, 0.65);
  }

  :global(body.theme-light) .subtitle {
    color: rgba(17, 17, 17, 0.65);
  }

  .form {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .row {
    display: grid;
    grid-template-columns: 170px 1fr;
    gap: 10px;
    align-items: center;
    min-height: 32px;
  }

  label {
    font-size: 13px;
    color: rgba(255, 255, 255, 0.78);
  }

  :global(body.theme-light) label {
    color: rgba(17, 17, 17, 0.78);
  }

  select,
  input[type='text'],
  input[type='number'] {
    height: 34px;
    border-radius: 10px;
    border: 1px solid rgba(255, 255, 255, 0.10);
    background: rgba(0, 0, 0, 0.18);
    padding: 0 12px;
    color: rgba(255, 255, 255, 0.90);
    outline: none;
  }

  :global(body.theme-light) select,
  :global(body.theme-light) input[type='text'],
  :global(body.theme-light) input[type='number'] {
    background: rgba(255, 255, 255, 0.7);
    border-color: rgba(17, 17, 17, 0.14);
    color: rgba(17, 17, 17, 0.90);
  }

  .files {
    display: grid;
    grid-template-columns: 1fr 140px;
    gap: 10px;
    align-items: stretch;
  }

  .files-box {
    border-radius: 12px;
    border: 1px solid rgba(255, 255, 255, 0.10);
    background: rgba(0, 0, 0, 0.18);
    min-height: 70px;
    padding: 10px;
    overflow: auto;
  }

  .files-box { position: relative; }
  .drop-mask { position: absolute; inset: 0; border-radius: 12px; pointer-events: none; }
  .drop-mask.drag { box-shadow: 0 0 0 2px rgba(59,130,246,0.65) inset, 0 0 0 4px rgba(59,130,246,0.25) inset; }

  :global(body.theme-light) .files-box {
    background: rgba(255, 255, 255, 0.7);
    border-color: rgba(17, 17, 17, 0.14);
  }

  .files-empty {
    font-size: 12px;
    color: rgba(255, 255, 255, 0.55);
  }

  :global(body.theme-light) .files-empty {
    color: rgba(17, 17, 17, 0.55);
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .chip {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    border-radius: 999px;
    border: 1px solid rgba(138, 180, 248, 0.35);
    background: rgba(138, 180, 248, 0.12);
    padding: 6px 10px;
    max-width: 260px;
    cursor: pointer;
  }
  .chip:hover {
    filter: brightness(1.05);
    border-color: rgba(138, 180, 248, 0.55);
  }
  .chip.active {
    background: rgba(59, 130, 246, 0.18);
    border-color: rgba(59, 130, 246, 0.85);
    box-shadow: 0 0 0 2px rgba(59, 130, 246, 0.25) inset;
  }
  .chip:focus-visible {
    outline: 2px solid rgba(59, 130, 246, 0.75);
    outline-offset: 2px;
  }

  .chip-name {
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .visually-hidden {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }

  .expander {
    width: 20px;
    height: 20px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    margin-right: 6px;
    border: 1px solid rgba(255, 255, 255, 0.10);
    border-radius: 6px;
    background: rgba(0, 0, 0, 0.18);
    color: rgba(255, 255, 255, 0.78);
    cursor: pointer;
    padding: 0;
    flex: 0 0 auto;
  }

  .expander:hover { filter: brightness(1.1); }

  :global(body.theme-light) .expander {
    background: rgba(255, 255, 255, 0.7);
    border-color: rgba(17, 17, 17, 0.14);
    color: rgba(17, 17, 17, 0.78);
  }

  .tree-row.category {
    background: rgba(0, 0, 0, 0.10);
  }

  :global(body.theme-light) .tree-row.category {
    background: rgba(255, 255, 255, 0.45);
  }

  .tree-row.level-0 .col.name { padding-left: 6px; }
  .tree-row.level-1 .col.name { padding-left: 26px; }
  .tree-row.level-2 .col.name { padding-left: 46px; }

  .chip-x {
    border: 0;
    background: transparent;
    color: rgba(255, 255, 255, 0.85);
    cursor: pointer;
    padding: 0 2px;
    font-size: 16px;
    line-height: 1;
  }

  :global(body.theme-light) .chip-x {
    color: rgba(17, 17, 17, 0.75);
  }

  .files-actions {
    display: flex;
    flex-direction: column;
    gap: 10px;
    justify-content: center;
  }

  .btn {
    height: 34px;
    border-radius: 999px;
    border: 1px solid rgba(255, 255, 255, 0.14);
    background: rgba(255, 255, 255, 0.08);
    color: rgba(255, 255, 255, 0.92);
    cursor: pointer;
    padding: 0 14px;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: default;
  }

  :global(body.theme-light) .btn {
    background: rgba(17, 17, 17, 0.05);
    border-color: rgba(17, 17, 17, 0.14);
    color: rgba(17, 17, 17, 0.9);
  }

  .path {
    display: grid;
    grid-template-columns: 1fr 140px;
    gap: 10px;
    align-items: center;
  }

  .path-input {
    width: 90%;
  }

  .build-row {
    margin-top: 6px;
  }

  .build {
    height: 72px;
    border-radius: 14px;
    border: 0;
    background: #3b82f6;
    color: #0b1220;
    font-size: 18px;
    font-weight: 800;
    cursor: pointer;
    justify-self: end;
    width: min(360px, 100%);
  }

  .build:hover {
    filter: brightness(1.05);
  }

  .section-title {
    font-size: 13px;
    font-weight: 700;
    color: rgba(255, 255, 255, 0.9);
    margin-top: 6px;
  }

  :global(body.theme-light) .section-title {
    color: rgba(17, 17, 17, 0.9);
  }

  .settings {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .log {
    flex: 1 1 auto;
    min-height: 160px;
    border-radius: 12px;
    border: 1px solid rgba(255, 255, 255, 0.10);
    background: rgba(0, 0, 0, 0.18);
    overflow: hidden;
  }

  :global(body.theme-light) .log {
    background: rgba(255, 255, 255, 0.7);
    border-color: rgba(17, 17, 17, 0.14);
  }

  .log-inner {
    height: 100%;
    overflow: auto;
    padding: 12px;
    font: 12px/1.55 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    color: rgba(255, 255, 255, 0.84);
  }

  :global(body.theme-light) .log-inner {
    color: rgba(17, 17, 17, 0.84);
  }

  .log-line {
    white-space: pre-wrap;
    word-break: break-word;
  }

  .log-level {
    color: rgba(138, 180, 248, 0.95);
    margin-right: 8px;
  }

  .log-line[data-level='WARN'] .log-level {
    color: rgba(250, 204, 21, 0.95);
  }

  .log-line[data-level='ERROR'] .log-level {
    color: rgba(248, 113, 113, 0.95);
  }

  /* scr_destructible panel */
  .scr-controls {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 10px;
    align-items: center;
    margin: 8px 0 6px 0;
  }
  .scr-controls .right { display: flex; gap: 8px; }

  .scr-tree {
    border: 1px solid rgba(255,255,255,0.1);
    border-radius: 8px;
    overflow: hidden;
  }
  .scr-tree .tree-row {
    display: grid;
    grid-template-columns: 320px 1fr 150px;
    gap: 8px;
    padding: 8px 10px;
    border-top: 1px solid rgba(255,255,255,0.06);
  }
  .scr-tree .tree-row.header { background: rgba(255,255,255,0.06); font-size: 12px; }
  .scr-tree .tree-row:first-child { border-top: 0; }
  .scr-tree .tree-row.debris { padding-left: 24px; }
  .scr-tree .tree-row.sel { background: rgba(59,130,246,0.15); }
  .scr-tree .col.name { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .scr-tree .col.path { opacity: 0.9; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .scr-tree .col.actions { display: flex; justify-content: end; gap: 6px; }

  .icon-btn {
    width: 28px;
    height: 28px;
    border-radius: 999px;
    border: 1px solid rgba(255, 255, 255, 0.14);
    background: rgba(255, 255, 255, 0.08);
    color: rgba(255, 255, 255, 0.92);
    cursor: pointer;
    padding: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    line-height: 1;
    font-size: 12px;
  }

  .icon-btn.danger {
    border-color: rgba(248, 113, 113, 0.35);
    background: rgba(248, 113, 113, 0.12);
  }

  .icon-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }

  :global(body.theme-light) .icon-btn {
    background: rgba(17, 17, 17, 0.05);
    border-color: rgba(17, 17, 17, 0.14);
    color: rgba(17, 17, 17, 0.9);
  }

  :global(body.theme-light) .icon-btn.danger {
    border-color: rgba(220, 38, 38, 0.35);
    background: rgba(220, 38, 38, 0.10);
  }

  .folder-btn {
    border: 0;
    background: transparent;
    color: inherit;
    padding: 0;
    margin-right: 6px;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    opacity: 0.9;
  }

  .folder-btn:hover { opacity: 1; }

  .folder-ico {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    margin-right: 6px;
    opacity: 0.9;
  }

  .inline-input {
    width: 100%;
    height: 28px;
    border-radius: 8px;
    border: 1px solid rgba(255, 255, 255, 0.10);
    background: rgba(0, 0, 0, 0.18);
    padding: 0 10px;
    color: rgba(255, 255, 255, 0.90);
    outline: none;
  }

  :global(body.theme-light) .inline-input {
    background: rgba(255, 255, 255, 0.7);
    border-color: rgba(17, 17, 17, 0.14);
    color: rgba(17, 17, 17, 0.90);
  }

  @media (max-width: 720px) {
    .row {
      grid-template-columns: 1fr;
      gap: 6px;
    }

    .files {
      grid-template-columns: 1fr;
    }

    .path {
      grid-template-columns: 1fr;
    }

    .build {
      width: 100%;
    }
  }
</style>
