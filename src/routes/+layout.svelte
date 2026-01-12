<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
  let isAppDragOver = $state(false);
  // Gate global drag overlay: keep false so overlay won't appear on Welcome or other pages.
  // Feature can be re-enabled per-page in the future by toggling this flag via a custom event.
  let allowAppDrop = $state(false);
  let moduleDragActive = $state(false);
  let arma4Root = $state<string>('');
  try { arma4Root = localStorage.getItem('arma4Root') ?? ''; } catch {}
  let unWeb: (() => void) | null = null;
  let unAllow: (() => void) | null = null;
  let wOver: any, wEnter: any, wLeave: any, wDrop: any;
  let dOver: any, dEnter: any, dLeave: any, dDrop: any;
  let diag = $state<{ types: string; effectAllowed: string; dropEffect: string; webviewType: string }>({ types: '', effectAllowed: '', dropEffect: '', webviewType: '' });
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
  async function handleOverlayMouseUp(e: MouseEvent) {
    if (!isAppDragOver) return;
    // If drag source refuses drop (no DataTransfer types), try clipboard as fallback
    try {
      const clip = await navigator.clipboard.readText();
      if (!clip) return;
      let parsed = parseWorkbenchDrag(clip);
      if (!parsed && clip.includes('\n')) parsed = parseWorkbenchDrag(clip.split('\n')[0]);
      if (!parsed) return;
      const resolved = await resolveArmaPath(parsed);
      if (resolved) {
        const ev = new CustomEvent('workbench-drop-path', { detail: resolved });
        window.dispatchEvent(ev);
      }
    } catch {}
  }

  function updateDiag(e: DragEvent) {
    const dt = e.dataTransfer;
    if (!dt) return;
    diag = {
      types: dt.types ? Array.from(dt.types as any).join(', ') : '',
      effectAllowed: (dt.effectAllowed as any) || '',
      dropEffect: (dt.dropEffect as any) || '',
      webviewType: diag.webviewType
    };
  }

  function parseWorkbenchDrag(data: string): string | null {
    let s = data.trim();
    try { s = decodeURIComponent(s); } catch {}
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
        try { localStorage.setItem('arma4Root', arma4Root); } catch {}
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
      let base = root.replace(/[\\/]+$/, '');
      return base + '\\' + sub;
    }
    return s;
  }
  function handleOverlayDrop(e: DragEvent) {
    e.preventDefault(); e.stopPropagation(); isAppDragOver = false;
    const dt = e.dataTransfer;
    if (!dt) return;
    if (dt.files && dt.files.length > 0) return;
    const candidates = ['text/uri-list','text/plain','text/x-moz-url','text/x-url','URL'];
    let raw = '';
    for (const t of candidates) { try { const v = dt.getData(t); if (v) { raw = v; break; } } catch {} }
    if (!raw && dt.types) {
      for (const t of Array.from(dt.types as any)) { try { const v = dt.getData(t as string); if (v) { raw = v; break; } } catch {} }
    }
    const done = (text: string) => {
      if (!text) return;
      let parsed = parseWorkbenchDrag(text);
      if (!parsed && text.includes('\n')) parsed = parseWorkbenchDrag(text.split('\n')[0]);
      if (!parsed) return;
      resolveArmaPath(parsed).then((resolved) => {
        if (resolved) {
          const ev = new CustomEvent('workbench-drop-path', { detail: resolved });
          window.dispatchEvent(ev);
        }
      });
    };
    if (raw) {
      done(raw);
    } else {
      // Fallback: try clipboard text on drop
      (async () => {
        try {
          const clip = await navigator.clipboard.readText();
          done(clip || '');
        } catch {}
      })();
    }
  }
  onMount(async () => {
    const isOverModuleDropZone = (e: DragEvent): boolean => {
      const path = (e.composedPath?.() ?? []) as any[];
      for (const n of path) {
        const el = n as any;
        if (el && typeof el === 'object' && typeof el.classList?.contains === 'function' && el.classList.contains('drop-zone')) {
          return true;
        }
        if (el && typeof el === 'object' && typeof el.closest === 'function' && el.closest('.drop-zone')) {
          return true;
        }
      }
      return false;
    };

    wOver = (e: DragEvent) => {
      if (isOverModuleDropZone(e)) return;
      e.preventDefault();
      e.stopPropagation();
      chooseDropEffect(e);
      updateDiag(e);
      if (allowAppDrop && !moduleDragActive) isAppDragOver = true;
    };
    wEnter = (e: DragEvent) => {
      if (isOverModuleDropZone(e)) return;
      e.preventDefault();
      e.stopPropagation();
      chooseDropEffect(e);
      updateDiag(e);
      if (allowAppDrop && !moduleDragActive) isAppDragOver = true;
    };
    wLeave = (e: DragEvent) => { e.preventDefault(); isAppDragOver = false; };
    wDrop = (e: DragEvent) => { e.preventDefault(); e.stopPropagation(); isAppDragOver = false; };
    window.addEventListener('dragover', wOver, { capture: true });
    window.addEventListener('dragenter', wEnter, { capture: true });
    window.addEventListener('dragleave', wLeave, { capture: true });
    window.addEventListener('drop', wDrop, { capture: true });

    dOver = (e: DragEvent) => {
      if (isOverModuleDropZone(e)) return;
      e.preventDefault();
      e.stopPropagation();
      chooseDropEffect(e);
      updateDiag(e);
      if (allowAppDrop && !moduleDragActive) isAppDragOver = true;
    };
    dEnter = (e: DragEvent) => {
      if (isOverModuleDropZone(e)) return;
      e.preventDefault();
      e.stopPropagation();
      chooseDropEffect(e);
      updateDiag(e);
      if (allowAppDrop && !moduleDragActive) isAppDragOver = true;
    };
    dLeave = (e: DragEvent) => { e.preventDefault(); isAppDragOver = false; };
    dDrop = (e: DragEvent) => { e.preventDefault(); e.stopPropagation(); isAppDragOver = false; };
    document.addEventListener('dragover', dOver, { capture: true });
    document.addEventListener('dragenter', dEnter, { capture: true });
    document.addEventListener('dragleave', dLeave, { capture: true });
    document.addEventListener('drop', dDrop, { capture: true });

    const webview = getCurrentWebviewWindow();
    unWeb = await webview.onDragDropEvent((e: any) => {
      const t = e?.payload?.type ?? e?.type ?? e?.event ?? e?.kind;
      // Don't activate the full-page overlay from Tauri hover/enter events.
      // These events may not map cleanly to DOM element dragenter/dragover, and can cause the overlay
      // to appear even while the user is dragging over a module-specific drop zone.
      if (t === 'cancelled' || t === 'leave' || t === 'drop') isAppDragOver = false;
      diag = { ...diag, webviewType: String(t || '') };
    });

    const onAllow = (e: CustomEvent<boolean>) => { allowAppDrop = !!e.detail; if (!allowAppDrop) isAppDragOver = false; };
    window.addEventListener('app:set-allow-drop', onAllow as any, { capture: true } as any);
    unAllow = () => window.removeEventListener('app:set-allow-drop', onAllow as any, { capture: true } as any);

    const onModuleDrag = (e: CustomEvent<boolean>) => { moduleDragActive = !!e.detail; if (moduleDragActive) isAppDragOver = false; };
    window.addEventListener('app:set-module-drag-active', onModuleDrag as any, { capture: true } as any);
    const unModuleDrag = () => window.removeEventListener('app:set-module-drag-active', onModuleDrag as any, { capture: true } as any);
    const prevUnAllow = unAllow;
    unAllow = () => { prevUnAllow && prevUnAllow(); unModuleDrag(); };
  });
  onDestroy(() => {
    unWeb && unWeb();
    unAllow && unAllow();
    wOver && window.removeEventListener('dragover', wOver);
    wEnter && window.removeEventListener('dragenter', wEnter);
    wLeave && window.removeEventListener('dragleave', wLeave);
    wDrop && window.removeEventListener('drop', wDrop);
    dOver && document.removeEventListener('dragover', dOver);
    dEnter && document.removeEventListener('dragenter', dEnter);
    dLeave && document.removeEventListener('dragleave', dLeave);
    dDrop && document.removeEventListener('drop', dDrop);
  });
</script>

<svelte:head>
  <link rel="preconnect" href="https://fonts.googleapis.com">
  <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin="anonymous">
  <link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&display=swap" rel="stylesheet">
</svelte:head>

<div class="app-root">
  <slot />
  <div class="app-drop-overlay" class:show={isAppDragOver && allowAppDrop} aria-hidden="true" ondragover={(e) => { e.preventDefault(); e.stopPropagation(); chooseDropEffect(e as DragEvent); updateDiag(e as DragEvent); }} ondragenter={(e) => { e.preventDefault(); e.stopPropagation(); chooseDropEffect(e as DragEvent); updateDiag(e as DragEvent); }} ondrop={(e) => { handleOverlayDrop(e as DragEvent); diag = { types: '', effectAllowed: '', dropEffect: '', webviewType: diag.webviewType }; }} ondragleave={(e) => { e.preventDefault(); isAppDragOver = false; diag = { types: '', effectAllowed: '', dropEffect: '', webviewType: diag.webviewType }; }} onmouseup={(e) => handleOverlayMouseUp(e as MouseEvent)}>
    <div class="drop-text">Drop .xob.meta here</div>
    <div class="drop-diag" aria-hidden="true">
      <div>types: {diag.types}</div>
      <div>effectAllowed: {diag.effectAllowed}</div>
      <div>dropEffect: {diag.dropEffect}</div>
      <div>webview: {diag.webviewType}</div>
    </div>
  </div>
</div>

<style>
  @font-face {
    font-family: 'Google Sans';
    src: local('Google Sans Regular'), local('GoogleSans-Regular'),
         url('/fonts/GoogleSans_17pt-Regular.ttf') format('truetype');
    font-weight: 400;
    font-style: normal;
    font-display: swap;
  }
  @font-face {
    font-family: 'Google Sans';
    src: local('Google Sans Medium'), local('GoogleSans-Medium'),
         url('/fonts/GoogleSans_17pt-Medium.ttf') format('truetype');
    font-weight: 500;
    font-style: normal;
    font-display: swap;
  }
  @font-face {
    font-family: 'Google Sans';
    src: local('Google Sans Bold'), local('GoogleSans-Bold'),
         url('/fonts/GoogleSans_17pt-Bold.ttf') format('truetype');
    font-weight: 700;
    font-style: normal;
    font-display: swap;
  }
  :root {
    --font-sans: 'Google Sans', 'Google Sans Text', 'Product Sans', Inter, Roboto, system-ui, -apple-system, 'Segoe UI', Helvetica, Arial, sans-serif;
  }

  :global(html), :global(body) {
    font-family: var(--font-sans);
  }
  :global(button), :global(input), :global(select), :global(textarea) {
    font-family: inherit;
  }
  :global(.chrome-tabs) {
    font-family: var(--font-sans) !important;
  }

  .app-root { min-height: 98vh; position: relative; }
  .app-drop-overlay { position: absolute; inset: 0; border: 2px dashed #8ab4f8; border-radius: 8px; background: rgba(138, 180, 248, 0.08); pointer-events: none; display: none; z-index: 8; }
  .app-drop-overlay.show { display: block; pointer-events: auto; }
  .drop-text { position: absolute; top: 50%; left: 50%; transform: translate(-50%, -50%); color: #8ab4f8; font-size: 20px; font-weight: 600; }
  .drop-diag { position: absolute; right: 8px; bottom: 8px; background: rgba(0,0,0,0.5); color: #dbe1ff; border: 1px solid rgba(138,180,248,0.4); padding: 8px 10px; border-radius: 6px; font: 12px/1.35 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; max-width: 50vw; pointer-events: none; }
</style>

