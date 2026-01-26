<script lang="ts">
  import { open } from '@tauri-apps/plugin-dialog';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
  import { onMount, onDestroy } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  let wbPort = $state<number>(5700);
  try { const p = localStorage.getItem('workbench_port'); if (p) wbPort = Math.max(1, Math.min(65535, parseInt(p))); } catch {}
  let filePath = $state<string>('');
  let isDragOver = $state(false);
  let realtimeMode = $state(true);
  let reexportTimer: any = null;
  let arma4Root = $state<string>('');
  try { const rm = localStorage.getItem('realtime_mode'); if (rm === '0') realtimeMode = false; if (rm === '1') realtimeMode = true; } catch {}
  try { arma4Root = localStorage.getItem('arma4Root') ?? ''; } catch {}
  // Table column widths (percent)
  let colGroup = $state(80);
  let colStatus = $state(20);
  const MIN_PCT = 10, MAX_PCT = 90;
  let tableEl: HTMLDivElement | null = null;
  let rootEl: HTMLDivElement | null = null;
  let tableMax = $state(0);
  let isLoading = $state(false);
  let resizing = false;
  let startX = 0;
  let startGroupPct = 80;
  type TreeNode = { id: string; name: string; status: 'Open' | 'Hide'; expanded?: boolean; children?: TreeNode[] };
  let tree = $state<TreeNode[]>([]);
  let flatNodes = $state<TreeNode[]>([]); // base MeshParam list (leaves) for grouping/toggling
  type TunnelInfo = { url: string; pid: number };
  let groupExpanded = $state<Record<string, boolean>>({});
  let defaultGroupExpanded = $state(true);
  let lastLoadedPath = $state<string>('');
  let tunnelUrl = $state<string>('');
  let isTunnelLoading = $state(false);
  let tunnelPid = $state<number | null>(null);
  let showQr = $state(false);
  let showFilePopup = $state(false);
  let checklistItems = $state([
    { id: 1, label: 'Uncheck Merge Meshes', passed: false, checked: false, fixed: false },
    { id: 2, label: 'Uncheck Merge Tri Meshes', passed: false, checked: false, fixed: false }
  ]);
  let isCheckingFile = $state(false);
  let isFixingFile = $state(false);
  let checkMessage = $state<string>('');
  
  async function checkFileContent(path: string) {
    isCheckingFile = true;
    checkMessage = '';
    try {
      const content = await invoke<string>('read_text_file', { path });
      if (!content) {
        checkMessage = 'File is empty or unreadable';
        checklistItems[0].passed = false;
        checklistItems[1].passed = false;
        return;
      }
      
      const mergeMeshesMatch = /MergeMeshes\s+([01])/.exec(content);
      const mergeTriMeshesMatch = /MergeTriMeshes\s+([01])/.exec(content);
      
      const mergeMeshesValue = mergeMeshesMatch ? parseInt(mergeMeshesMatch[1]) : null;
      const mergeTriMeshesValue = mergeTriMeshesMatch ? parseInt(mergeTriMeshesMatch[1]) : null;
      
      checklistItems[0].passed = mergeMeshesValue === 0;
      checklistItems[0].checked = mergeMeshesValue !== null;
      checklistItems[0].fixed = false;
      
      checklistItems[1].passed = mergeTriMeshesValue === 0;
      checklistItems[1].checked = mergeTriMeshesValue !== null;
      checklistItems[1].fixed = false;
      
      const allPassed = checklistItems.every(item => item.passed);
      if (allPassed) {
        checkMessage = '✓ All requirements met. Ready to use.';
      } else {
        const failed = checklistItems.filter(item => !item.passed && item.checked).map(item => item.label);
        checkMessage = failed.length > 0 ? `⚠ must Uncheck ${failed.join(', ')}` : '⚠ Some values should uncheck';
      }
    } catch (e) {
      console.error('Failed to check file content', e);
      checkMessage = `Error: ${String(e).slice(0, 50)}`;
    } finally {
      isCheckingFile = false;
    }
  }
  
  async function fixFileContent(path: string) {
    isFixingFile = true;
    checkMessage = '';
    try {
      const content = await invoke<string>('read_text_file', { path });
      if (!content) {
        checkMessage = 'File is empty or unreadable';
        return;
      }
      
      const nl = content.includes('\r\n') ? '\r\n' : '\n';
      let updated = content;
      let fixedCount = 0;
      
      const replaceMergeMeshes = (text: string): string => {
        return text.replace(/^([ \t]*MergeMeshes[ \t]+)([01])([ \t]*)$/gm, '$10$3');
      };
      
      const mergeTriMeshes = (text: string): string => {
        return text.replace(/^([ \t]*MergeTriMeshes[ \t]+)([01])([ \t]*)$/gm, '$10$3');
      };
      
      const before1 = updated;
      updated = replaceMergeMeshes(updated);
      if (updated !== before1) {
        checklistItems[0].fixed = true;
        fixedCount++;
      }
      
      const before2 = updated;
      updated = mergeTriMeshes(updated);
      if (updated !== before2) {
        checklistItems[1].fixed = true;
        fixedCount++;
      }
      
      if (!updated.match(/^[ \t]*MergeMeshes[ \t]+[01][ \t]*$/m)) {
        const fbxMatch = updated.match(/^[ \t]*FBXResourceClass\b.*\{[ \t]*$/m);
        if (fbxMatch) {
          const lineEnd = updated.indexOf('\n', fbxMatch.index! + fbxMatch[0].length);
          const insertPos = lineEnd === -1 ? fbxMatch.index! + fbxMatch[0].length : lineEnd + 1;
          const indent = fbxMatch[0].match(/^[ \t]*/)?.[0] || '';
          updated = updated.slice(0, insertPos) + indent + ' MergeMeshes 0' + nl + updated.slice(insertPos);
          checklistItems[0].fixed = true;
          fixedCount++;
        }
      }
      
      if (!updated.match(/^[ \t]*MergeTriMeshes[ \t]+[01][ \t]*$/m)) {
        const fbxMatch = updated.match(/^[ \t]*FBXResourceClass\b.*\{[ \t]*$/m);
        if (fbxMatch) {
          const lineEnd = updated.indexOf('\n', fbxMatch.index! + fbxMatch[0].length);
          const insertPos = lineEnd === -1 ? fbxMatch.index! + fbxMatch[0].length : lineEnd + 1;
          const indent = fbxMatch[0].match(/^[ \t]*/)?.[0] || '';
          updated = updated.slice(0, insertPos) + indent + ' MergeTriMeshes 0' + nl + updated.slice(insertPos);
          checklistItems[1].fixed = true;
          fixedCount++;
        }
      }
      
      if (updated !== content) {
        await invoke('write_text_file', { path, content: updated });
        checkMessage = `✓ Fixed ${fixedCount} issue(s). Reloading...`;
        await new Promise(resolve => setTimeout(resolve, 300));
        
        await checkFileContent(path);
        await loadMetaAndPopulate(path);
        
        checkMessage = `✓ Fixed and reloaded. Exporting...`;
        await new Promise(resolve => setTimeout(resolve, 200));
        scheduleReexport();
        checkMessage = `✓ Fixed successfully!`;
      } else {
        checkMessage = 'No changes needed.';
      }
    } catch (e) {
      console.error('Failed to fix file content', e);
      checkMessage = `Error during fix: ${String(e).slice(0, 50)}`;
    } finally {
      isFixingFile = false;
    }
  }
  // VIS input variables driven by group toggles (Base and Zone letters)
  let visInputs = $state<Record<string, boolean>>({});
  function delay(ms: number) { return new Promise<void>(res => setTimeout(res, ms)); }
  function flatten(nodes: TreeNode[], depth = 0, out: Array<{ node: TreeNode; depth: number }> = []) {
    for (const n of nodes) {
      out.push({ node: n, depth });
      if (n.expanded && n.children?.length) flatten(n.children, depth + 1, out);
    }
    return out;
  }
  function shouldApplyVisForName(name: string): boolean {
    const lname = name.toLowerCase();
    if (lname.includes('base') || lname.includes('brick')) return true;
    if (detectZone(name)) return true;
    return false;
  }
  function getVisible() { return flatten(tree); }
  function toggleNode(n: TreeNode) {
    if (n.children && n.children.length) {
      n.expanded = !n.expanded;
      groupExpanded[n.name] = n.expanded;
    }
  }
  let unlistenDrop: (() => void) | null = null;
  let unlistenWebview: (() => void) | null = null;
  let unlistenRemote: (() => void) | null = null;
  let unlistenRemoteBatch: (() => void) | null = null;
  let unlistenPreviewMode: (() => void) | null = null;
  let unlistenPreviewNow: (() => void) | null = null;
  let unlistenToggleMany: (() => void) | null = null;
  let wndDragOver: ((e: DragEvent) => void) | null = null;
  let wndDragEnter: ((e: DragEvent) => void) | null = null;
  let wndDragLeave: ((e: DragEvent) => void) | null = null;
  let wndDrop: ((e: DragEvent) => void) | null = null;
  let docDragOver: ((e: DragEvent) => void) | null = null;
  let docDragEnter: ((e: DragEvent) => void) | null = null;
  let docDragLeave: ((e: DragEvent) => void) | null = null;
  let docDrop: ((e: DragEvent) => void) | null = null;
  let unlistenWorkbenchPath: (() => void) | null = null;
  function chooseDropEffect(e: DragEvent) {
    const dt = e.dataTransfer;
    if (!dt) return;
    const allowed = (dt.effectAllowed || '').toLowerCase();
    const types = Array.from((dt.types || []) as any);
    // If no OS files are offered, prefer link (external app linking semantics)
    if (!dt.files || dt.files.length === 0) {
      dt.dropEffect = 'link';
      return;
    }
    // Prefer semantics by data type first
    if (types.includes('text/uri-list')) {
      dt.dropEffect = 'link';
      return;
    }
    if (types.includes('Files')) {
      dt.dropEffect = 'copy';
      return;
    }
    // Prefer copy if allowed, otherwise use link if that's what's offered by the source app
    if (allowed === 'none') {
      dt.dropEffect = 'none';
    } else if (
      allowed.includes('copy') ||
      allowed === 'all' ||
      allowed === 'uninitialized' ||
      allowed === ''
    ) {
      dt.dropEffect = 'copy';
    } else if (allowed.includes('link')) {
      dt.dropEffect = 'link';
    } else if (allowed.includes('move')) {
      dt.dropEffect = 'move';
    } else {
      dt.dropEffect = 'copy';
    }
  }

  function onResizeDown(e: MouseEvent) {
    resizing = true;
    startX = e.clientX;
    startGroupPct = colGroup;
    window.addEventListener('mousemove', onResizeMove);
    window.addEventListener('mouseup', onResizeUp);
    e.preventDefault();
  }
  function onResizeMove(e: MouseEvent) {
    if (!resizing || !tableEl) return;
    const dx = e.clientX - startX;
    const width = tableEl.getBoundingClientRect().width || 1;
    const deltaPct = (dx / width) * 100;
    let next = Math.max(MIN_PCT, Math.min(MAX_PCT, startGroupPct + deltaPct));
    colGroup = next;
    colStatus = 100 - next;
  }
  function onResizeUp() {
    if (!resizing) return;
    resizing = false;
    window.removeEventListener('mousemove', onResizeMove);
    window.removeEventListener('mouseup', onResizeUp);
  }
  function calcTableMax() {
    if (!tableEl) return;
    const rect = tableEl.getBoundingClientRect();
    const vh = window.innerHeight || document.documentElement.clientHeight || 0;
    const marginBottom = 16; // breathing room
    const max = Math.max(120, Math.floor(vh - rect.top - marginBottom));
    tableMax = max;
  }
  function onResizeKey(e: KeyboardEvent) {
    const step = e.shiftKey ? 5 : 2; // percent
    if (e.key === 'ArrowLeft') {
      e.preventDefault();
      let next = Math.max(MIN_PCT, colGroup - step);
      colGroup = next; colStatus = 100 - next;
    } else if (e.key === 'ArrowRight') {
      e.preventDefault();
      let next = Math.min(MAX_PCT, colGroup + step);
      colGroup = next; colStatus = 100 - next;
    }
  }
  function isInsideRoot(e: DragEvent): boolean {
    if (!rootEl) return true;
    const x = (e.clientX ?? -1), y = (e.clientY ?? -1);
    if (x < 0 || y < 0) return true;
    const el = document.elementFromPoint(x, y) as Element | null;
    if (el) return rootEl.contains(el);
    const r = rootEl.getBoundingClientRect();
    return x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;
  }
  function handleDragOver(e: DragEvent) { e.preventDefault(); e.stopPropagation(); chooseDropEffect(e); isDragOver = isInsideRoot(e); }
  function handleDragEnter(e: DragEvent) { e.preventDefault(); e.stopPropagation(); chooseDropEffect(e); isDragOver = isInsideRoot(e); }
  function handleDragLeave(e: DragEvent) { e.preventDefault(); if (!isInsideRoot(e)) isDragOver = false; }
  function handleDrop(e: DragEvent) {
    e.preventDefault();
    e.stopPropagation();
    isDragOver = false;
    // If Workbench provides text-based drag (no files), parse it
    const dt = e.dataTransfer;
    if (dt) handleWorkbenchTextDrop(dt);
  }
  async function pickFile() {
    try {
      const selection = await open({ multiple: false, filters: [{ name: 'XOB Meta', extensions: ['meta'] }] });
      if (typeof selection === 'string') {
        applySelection(selection);
      }
    } catch (e) {
      console.error(e);
    }
  }

  async function importFromClipboard() {
    try {
      const text = await navigator.clipboard.readText();
      if (!text) return;
      let parsed = parseWorkbenchDrag(text);
      if (!parsed && text.includes('\n')) parsed = parseWorkbenchDrag(text.split('\n')[0]);
      if (!parsed) return;
      const resolved = await resolveArmaPath(parsed);
      if (resolved) applySelection(resolved);
    } catch (e) {
      console.error(e);
    }
  }

  function applySelection(p: string) {
    if (typeof p !== 'string') return;
    const lower = p.toLowerCase();
    if (lower.endsWith('.xob.meta')) {
      filePath = p;
      showFilePopup = true;
      checkFileContent(filePath);
      loadMetaAndPopulate(filePath);
    } else if (lower.endsWith('.xob')) {
      filePath = p + '.meta';
      showFilePopup = true;
      checkFileContent(filePath);
      loadMetaAndPopulate(filePath);
    } else {
      // ignore other extensions
    }
  }

  function parseWorkbenchDrag(data: string): string | null {
    // Example: file:$Arma4:Assets/.../AircraftShelter_E_01.xob?resourceName=...&exactPath=$Arma4:Assets/.../AircraftShelter_E_01.xob
    let s = data.trim();
    try { s = decodeURIComponent(s); } catch {}
    // Strip leading {GUID} if present
    if (s.startsWith('{') && s.indexOf('}') > 0) {
      s = s.slice(s.indexOf('}') + 1);
    }
    // Prefer exactPath= if present
    const qIdx = s.indexOf('?');
    if (qIdx >= 0) {
      const query = s.slice(qIdx + 1);
      const pairs = query.split('&');
      for (const p of pairs) {
        const [k, v] = p.split('=');
        if (k === 'exactPath' && v) return v;
      }
      s = s.slice(0, qIdx); // fallback to left of '?'
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
      // join root + sub (ensure single backslash)
      let base = root.replace(/[\\/]+$/, '');
      return base + '\\' + sub;
    }
    return s;
  }

  function parseXobMeta(content: string): TreeNode[] {
    const out: TreeNode[] = [];
    const re = /MeshParam\s+(?:"([^"]+)"|([^\s\{"]+))\s*\{([\s\S]*?)\}/g;
    let m: RegExpExecArray | null;
    const seen = new Set<string>();
    while ((m = re.exec(content)) !== null) {
      const name = (m[1] || m[2] || '').trim();
      if (!name || seen.has(name)) continue;
      seen.add(name);
      const body = m[3] || '';
      const hide = /(^|\s)Hide\s+1(\s|$)/.test(body);
      out.push({ id: name, name, status: hide ? 'Hide' : 'Open' });
    }
    return out;
  }

  function isVisNode(name: string): boolean {
    // Treat any MeshParam containing 'FDST_VIS' as VIS, plus legacy '_VIS-' pattern
    return /FDST_VIS/i.test(name) || /_VIS\-/i.test(name);
  }
  function extractVisExpr(name: string): string | null {
    const m = /_VIS\-([^_\s]+)/i.exec(name) || /FDST_VIS\-([^_\s]+)/i.exec(name);
    if (!m) return null;
    let expr = m[1];
    // Normalize filename-safe operators to parser operators
    // '#' -> AND, '@' -> OR, '^' -> XOR (unchanged)
    expr = expr.replace(/#/g, '&&').replace(/@/g, '||');
    return expr;
  }
  function detectZone(name: string): string | null {
    const m = /(?:^|[._\-\s])ID[\-_]?([A-Z])(\d*)/i.exec(name);
    return m ? m[1].toUpperCase() : null;
  }
  function isDecalName(nameLower: string): boolean {
    if (/(^|[^a-z0-9])(base|[a-z])1([^a-z0-9]|$)/i.test(nameLower)) return true;
    if (/\bdecals?\b/.test(nameLower)) return true;
    if (/[\-_]decals?[\-_]/.test(nameLower)) return true;
    return nameLower.includes('decals');
  }
  // ===== VIS logic (simplified evaluator based on Enfusion Visibility Manager) =====
  type VarsState = Record<string, boolean>;
  function buildVarsState(nodes: TreeNode[]): VarsState {
    const st: VarsState = {};
    // 1) Seed from explicit group inputs when available (Base + Zones)
    for (const k in visInputs) st[k] = !!visInputs[k];
    // 2) Fill any missing tokens from leaf states (non-VIS)
    for (const n of nodes) {
      if (isVisNode(n.name)) continue; // VIS nodes are outcomes, not inputs
      const open = n.status === 'Open';
      const z = detectZone(n.name);
      if (z && !(z in st)) st[z] = open;
      const lname = n.name.toLowerCase();
      if (lname.includes('base') && !('Base' in st)) st['Base'] = open;
      // capture tokens like A1, B2, Base1, etc. (best-effort)
      const m = /([A-Za-z]+)(\d+)/.exec(n.name);
      if (m) st[m[0]] = open;
    }
    return st;
  }
  function valOf(token: string, st: VarsState): boolean {
    const lookup = (key: string): boolean | undefined => {
      if (key in st) return !!st[key];
      const up = key.toUpperCase();
      for (const k in st) if (k.toUpperCase() === up) return !!st[k];
      return undefined;
    };
    const direct = lookup(token);
    if (direct !== undefined) return direct;
    const m = /^([A-Za-z]+)(\d+)$/.exec(token);
    if (m) {
      const base = m[1];
      const baseVal = lookup(base);
      if (baseVal !== undefined) return baseVal;
    }
    return true; // default True (do not hide) for unknown tokens
  }
  function evalVisExpr(expr: string, st: VarsState): boolean {
    // Tokenizer
    const tokens: string[] = [];
    let i = 0;
    const n = expr.length;
    const isAlphaNum = (c: string) => /[A-Za-z0-9]/.test(c);
    while (i < n) {
      const c = expr[i];
      if (c === ' ' || c === '\t' || c === '\n' || c === '\r') { i++; continue; }
      if (c === '(' || c === ')') { tokens.push(c); i++; continue; }
      if (c === '!' ) { tokens.push('!'); i++; continue; }
      if ((c === '&' || c === '|' || c === '^') && i + 1 < n && expr[i+1] === c) { tokens.push(c + c); i += 2; continue; }
      if (c === '&' || c === '|' || c === '^') { tokens.push(c); i++; continue; }
      if (isAlphaNum(c)) {
        let j = i;
        while (j < n && isAlphaNum(expr[j])) j++;
        const word = expr.slice(i, j);
        const up = word.toUpperCase();
        if (up === 'AND' || up === 'OR' || up === 'XOR' || up === 'NOT') tokens.push(up);
        else tokens.push(word);
        i = j; continue;
      }
      i++;
    }
    // Recursive descent: OR > XOR > AND > UNARY
    let p = 0;
    const peek = () => tokens[p];
    const consume = (t?: string) => (t ? (tokens[p] === t ? (p++, t) : null) : tokens[p++]);
    const parsePrimary = (): boolean => {
      const t = peek();
      if (t === '(') { consume('('); const v = parseOr(); consume(')'); return v; }
      if (t) { consume(); return valOf(t, st); }
      return true;
    };
    const parseUnary = (): boolean => {
      const t = peek();
      if (t === '!' || t === 'NOT') { consume(); return !parseUnary(); }
      return parsePrimary();
    };
    const parseAnd = (): boolean => {
      let v = parseUnary();
      while (true) {
        const t = peek();
        if (t === '&&' || t === '&' || t === 'AND') { consume(); v = v && parseUnary(); }
        else break;
      }
      return v;
    };
    const parseXor = (): boolean => {
      let v = parseAnd();
      while (true) {
        const t = peek();
        if (t === '^^' || t === '^' || t === 'XOR') { consume(); const r = parseAnd(); v = (v ? !r : r); }
        else break;
      }
      return v;
    };
    const parseOr = (): boolean => {
      let v = parseXor();
      while (true) {
        const t = peek();
        if (t === '||' || t === '|' || t === 'OR') { consume(); v = v || parseXor(); }
        else break;
      }
      return v;
    };
    return parseOr();
  }
  function applyVisOverrides(nodes: TreeNode[]): TreeNode[] {
    const st = buildVarsState(nodes);
    return nodes.map(n => {
      if (!isVisNode(n.name)) return n;
      const expr = extractVisExpr(n.name);
      if (!expr) return n;
      const visible = evalVisExpr(expr, st);
      return { ...n, status: visible ? 'Open' : 'Hide' };
    });
  }
  function groupMeshParams(nodes: TreeNode[]): TreeNode[] {
    const vis: TreeNode[] = [];
    const bases: TreeNode[] = [];
    const bricks: TreeNode[] = [];
    const decals: TreeNode[] = [];
    const singles: TreeNode[] = [];
    const zones: Record<string, TreeNode[]> = {};
    const zoneDecals: Record<string, TreeNode[]> = {};

    for (const n of nodes) {
      const lname = n.name.toLowerCase();
      const z = detectZone(n.name);
      if (lname.includes('brick')) { bricks.push(n); continue; }
      // VIS should take precedence over other categories (e.g., decals)
      if (isVisNode(n.name)) { vis.push(n); if (z && /[_-]ID[_-]?[A-Z]\d*/i.test(n.name)) (zones[z] ||= []).push(n); continue; }
      if (isDecalName(lname)) { decals.push(n); if (z) (zoneDecals[z] ||= []).push(n); continue; }
      if (lname.includes('base')) { bases.push(n); continue; }
      if (z) { (zones[z] ||= []).push(n); continue; }
      singles.push(n);
    }

    const out: TreeNode[] = [];
    const mkGroup = (title: string, arr: TreeNode[]): TreeNode | null => {
      if (!arr.length) return null;
      const anyOpen = arr.some(c => c.status === 'Open');
      const expanded = (title in groupExpanded) ? !!groupExpanded[title] : defaultGroupExpanded;
      return { id: `grp:${title}`, name: title, status: anyOpen ? 'Open' : 'Hide', expanded, children: arr };
    };
    const pushIf = (g: TreeNode | null) => { if (g) out.push(g); };

    // Zones first
    for (const z of Object.keys(zones).sort()) {
      const combined = Array.from(new Set([...(zones[z] || []), ...(zoneDecals[z] || [])]));
      pushIf(mkGroup(`Zone${z}`, combined.filter(n => !isVisNode(n.name))));
    }

    // Then other groups
    pushIf(mkGroup('VIS', vis));
    pushIf(mkGroup('Base', bases));
    pushIf(mkGroup('Brick', bricks));
    pushIf(mkGroup('Decals', decals));
    pushIf(mkGroup('Singles', singles));
    return out;
  }

  async function loadMetaAndPopulate(path: string, applyVis: boolean = true, initialCollapse: boolean = false) {
    isLoading = true;
    try {
      const content = await invoke<string>('read_text_file', { path });
      const nodes = parseXobMeta(content || '');
      // Collapse only on first open of a new file (or when explicitly requested)
      if (initialCollapse || (!lastLoadedPath || lastLoadedPath !== path)) {
        groupExpanded = {};
        defaultGroupExpanded = false;
      }
      flatNodes = applyVis ? applyVisOverrides(nodes) : nodes;
      tree = groupMeshParams(flatNodes);
      // initialize VIS inputs from group summaries (Base + Zones) on first load
      visInputs = {};
      for (const g of tree) {
        const nm = g.name;
        const isZone = /^Zone([A-Z])$/.test(nm);
        if (nm === 'Base' || isZone) {
          const key = nm === 'Base' ? 'Base' : (nm.match(/^Zone([A-Z])$/)![1]);
          visInputs[key] = (g.status === 'Open');
        }
      }
      lastLoadedPath = path;
      pushRemoteState();
    } catch (e) {
      console.error('Failed to read meta file', e);
      tree = [];
    } finally {
      isLoading = false;
    }
  }

  async function startRemote() {
    try {
      isTunnelLoading = true;
      showQr = false;
      tunnelUrl = '';
      // Stop previous instance if any
      if (tunnelPid != null) {
        try { await invoke('stop_quick_tunnel', { pid: tunnelPid }); } catch {}
        tunnelPid = null;
      }
      const info = await invoke<TunnelInfo>('start_quick_tunnel_unique');
      if (info && typeof (info as any).url === 'string' && typeof (info as any).pid === 'number') {
        tunnelUrl = (info as any).url;
        tunnelPid = (info as any).pid;
      }
    } catch (e) {
      console.error(e);
    } finally {
      isTunnelLoading = false;
    }
  }
  async function copyTunnelUrl() {
    try { if (tunnelUrl) await navigator.clipboard.writeText(tunnelUrl); } catch {}
  }

  async function onToggleNode(n: TreeNode) {
    // Preserve current expanded state of this group (if applicable)
    if (n.children && n.children.length) {
      const key = n.name;
      groupExpanded[key] = (key in groupExpanded) ? !!groupExpanded[key] : !!n.expanded;
    }
    const next = n.status === 'Open' ? 'Hide' : 'Open';
    // If group node, update all its children's names
    if (n.children && n.children.length) {
      const names = new Set(n.children.map(c => c.name));
      flatNodes = flatNodes.map(x => names.has(x.name) ? { ...x, status: next } : x);
      // update VIS input variables when Base or Zone group toggled
      if (n.name === 'Base') visInputs['Base'] = (next === 'Open');
      else {
        const m = /^Zone([A-Z])$/.exec(n.name);
        if (m) visInputs[m[1]] = (next === 'Open');
      }
    } else {
      flatNodes = flatNodes.map(x => x.name === n.name ? { ...x, status: next } : x);
    }
    const applyVis = !!(n.children && ((n.name === 'Base') || (n.name === 'Brick') || /^Zone[A-Z]$/.test(n.name)));
    if (applyVis) flatNodes = applyVisOverrides(flatNodes);
    tree = groupMeshParams(flatNodes);
    pushRemoteState();
    await saveBackToMeta(!applyVis);
    try { isLoading = true; await delay(2000); } finally { isLoading = false; }
  }

  function rebuildMetaText(original: string, nodes: TreeNode[]): string {
    // Rebuild by toggling Hide 1 lines inside each MeshParam block
    const re = /MeshParam\s+(?:"([^"]+)"|([^\s\{"]+))\s*\{([\s\S]*?)\}/g;
    const nl = original.includes('\r\n') ? '\r\n' : '\n';
    const map = new Map(nodes.map(n => [n.name, n.status]));
    return original.replace(re, (_m, qname, name2, body) => {
      const name = (qname || name2 || '').trim();
      if (!map.has(name)) return _m;
      const wantHide = map.get(name) === 'Hide';
      // Normalize body with explicit newlines
      let lines = body.split(/\r?\n/);
      // remove existing Hide lines
      lines = lines.filter((ln: string) => !/^\s*(Hide|NoImport)\s*[:=]?\s*1\s*$/i.test(ln));
      if (wantHide) {
        // remove leading blank lines
        while (lines.length && lines[0].trim() === '') lines.shift();
        // ensure first element is empty string to produce a newline after '{'
        if (lines.length === 0 || lines[0] !== '') lines.unshift('');
        // insert Hide 1 as the first content line
        lines.splice(1, 0, '      Hide 1');
        // ensure trailing newline so '}' is on its own line
        if (lines[lines.length - 1] !== '') lines.push('    ');
      }
      // ensure braces layout by rejoining using original newline
      const newBody = lines.join(nl);
      return _m.replace(body, newBody);
    });
  }

  async function saveBackToMeta(skipVisOverride: boolean = false) {
    if (!filePath) return;
    try {
      isLoading = true;
      const original = await invoke<string>('read_text_file', { path: filePath });
      const base = flatNodes; // leaves only (may be forced without VIS overrides)
      const updated = rebuildMetaText(original, base);
      if (updated !== original) {
        await invoke('write_text_file', { path: filePath, content: updated });
      }
    } catch (e) {
      console.error('Failed to save meta file', e);
    } finally {
      isLoading = false;
      // reload to reflect formatting
      if (filePath) loadMetaAndPopulate(filePath, !skipVisOverride);
      scheduleReexport();
      pushRemoteState();
    }
  }

  function pushRemoteState() {
    try {
      const items = flatNodes.map(n => ({ name: n.name, status: n.status }));
      invoke('set_mesh_state', { items, filePath });
    } catch {}
  }

  function refreshList() {
    if (filePath) {
      loadMetaAndPopulate(filePath);
    }
  }

  async function applyBatch(next: 'Open' | 'Hide') {
    // Force every MeshParam, including VIS nodes
    flatNodes = flatNodes.map(n => ({ ...n, status: next }));
    tree = groupMeshParams(flatNodes);
    pushRemoteState();
    await saveBackToMeta(true);
    try { isLoading = true; await delay(2000); } finally { isLoading = false; }
  }
  async function showAll() { await applyBatch('Open'); }
  async function hideAll() { await applyBatch('Hide'); }

  function saveWbPort() {
    try { localStorage.setItem('workbench_port', String(wbPort)); } catch {}
  }

  function setRealtimeMode(v: boolean) {
    realtimeMode = v;
    try { localStorage.setItem('realtime_mode', v ? '1' : '0'); } catch {}
    try { invoke('set_realtime_mode', { realtime: v }); } catch {}
  }

  function scheduleReexport() {
    if (!realtimeMode) return;
    if (reexportTimer) { clearTimeout(reexportTimer); reexportTimer = null; }
    reexportTimer = setTimeout(() => { exportToWorkbench(); reexportTimer = null; }, 200);
  }

  async function exportToWorkbench() {
    if (!filePath) return;
    // Derive FBX path next to XOB
    let fbx = '';
    const lower = filePath.toLowerCase();
    if (lower.endsWith('.xob.meta')) {
      const xob = filePath.slice(0, -5); // remove .meta
      fbx = xob.replace(/\.xob$/i, '.fbx');
    } else if (lower.endsWith('.xob')) {
      fbx = filePath.replace(/\.xob$/i, '.fbx');
    }
    if (!fbx) return;
    const resourcePath = fbx.replaceAll('\\', '/');
    const params = {
      resourcePath,
      exportMorphs: false,
      exportSceneHierarchy: true,
      exportSkinning: false,
      materialOverrides: {}
    };
    try {
      isLoading = true;
      const resp = await invoke<any>('wb_call', { funcName: 'ExportFBXResource', params, port: wbPort });
      console.log('ExportFBXResource resp', resp);
    } catch (e) {
      console.error('ExportFBXResource failed', e);
    } finally {
      isLoading = false;
    }
  }

  function handleWorkbenchTextDrop(dt: DataTransfer) {
    // Only try when there are no OS Files provided
    if (dt.files && dt.files.length > 0) return;
    // Try common text/link formats
    const candidates = [
      'text/uri-list',
      'text/plain',
      'text/plain;charset=utf-8',
      'text/plain; charset=utf-8',
      'text/unicode',
      'text',
      'Text',
      'text/html',
      'text/x-moz-url',
      'text/x-url',
      'UniformResourceLocatorW',
      'UniformResourceLocator',
      'URL'
    ];
    let raw = '';
    for (const t of candidates) {
      try {
        const v = dt.getData(t);
        if (v) { raw = v; break; }
      } catch {}
    }
    // If still empty, attempt all declared types
    if (!raw && dt.types) {
      for (const t of Array.from(dt.types as any)) {
        try {
          const v = dt.getData(t as string);
          if (v) { raw = v; break; }
        } catch {}
      }
    }
    if (!raw) return;
    let parsed = parseWorkbenchDrag(raw);
    // text/x-moz-url may contain "URL\nTitle"
    if (!parsed && raw.includes('\n')) parsed = parseWorkbenchDrag(raw.split('\n')[0]);
    if (!parsed) return;
    resolveArmaPath(parsed).then((resolved) => {
      if (resolved) applySelection(resolved);
    });
  }

  onMount(() => {
    (async () => {
      try {
        // scope visual feedback to component area only; rely on DOM drag events for overlay
        unlistenDrop = await listen<string[] | string>('tauri://file-drop', (e) => {
          isDragOver = false;
          const payload = e.payload as any;
          const first = Array.isArray(payload) ? payload[0] : payload;
          if (typeof first === 'string') applySelection(first);
        });

        const webview = getCurrentWebviewWindow();
        unlistenWebview = await webview.onDragDropEvent((ev: any) => {
          const t = ev?.payload?.type ?? ev?.type ?? ev?.event ?? ev?.kind;
          if (t === 'drop') {
            isDragOver = false;
            const pathsAny = ev?.payload?.paths ?? ev?.paths ?? ev?.payload;
            const first = Array.isArray(pathsAny) ? pathsAny[0] : pathsAny;
            if (typeof first === 'string') applySelection(first);
          }
          // Ignore 'cancelled' / 'leave' here; rely on DOM dragleave with bounds check to hide overlay
        });
        // Listen for remote toggle events from built-in remote server
        unlistenRemote = await listen('remote_toggle', (e: any) => {
          try {
            const payload = (e?.payload || {}) as { name?: string; status?: string };
            const target = payload.name;
            if (!target) return;
            // Update local flatNodes
            const nextStatus = (payload.status === 'Open' || payload.status === 'Hide') ? payload.status : undefined;
            let changed = false;
            flatNodes = flatNodes.map(x => {
              if (x.name === target) {
                const s = nextStatus ?? (x.status === 'Open' ? 'Hide' : 'Open');
                if (s !== x.status) changed = true;
                return { ...x, status: s };
              }
              return x;
            });
            if (changed) {
              // Mirror desktop logic: single-item toggle does not auto-apply VIS overrides
              tree = groupMeshParams(flatNodes);
              pushRemoteState();
              saveBackToMeta(true);
            }
          } catch {}
        });
        unlistenRemoteBatch = await listen('remote_batch', (e: any) => {
          try {
            const s = (e?.payload as any) as string;
            if (s === 'Open' || s === 'Hide') {
              applyBatch(s as any);
            }
          } catch {}
        });
        unlistenToggleMany = await listen('remote_toggle_many', (e: any) => {
          try {
            const payload = (e?.payload || {}) as { names?: string[]; status?: string; apply_vis?: boolean; vis_var?: string };
            const names = new Set((payload.names || []) as string[]);
            const target = (payload.status === 'Open' || payload.status === 'Hide') ? payload.status : undefined;
            if (!names.size || !target) return;
            let changed = false;
            flatNodes = flatNodes.map(x => {
              if (names.has(x.name) && x.status !== target) { changed = true; return { ...x, status: target as any }; }
              return x;
            });
            if (changed) {
              // update visInputs if specific var provided (e.g., Base or Zone letter)
              if (payload.vis_var) visInputs[payload.vis_var] = (target === 'Open');
              if (payload.apply_vis) flatNodes = applyVisOverrides(flatNodes);
              tree = groupMeshParams(flatNodes);
              pushRemoteState();
              saveBackToMeta(!payload.apply_vis);
            }
          } catch {}
        });
        unlistenPreviewMode = await listen('remote_preview_mode', (e: any) => {
          try {
            const v = !!(e?.payload as any);
            setRealtimeMode(v);
          } catch {}
        });
        unlistenPreviewNow = await listen('remote_preview_now', (_e: any) => {
          try {
            if (!realtimeMode) exportToWorkbench();
          } catch {}
        });
        startRemote();
      } catch (err) {
        console.error(err);
      }
    })();

    const onWorkbenchPath = (ev: any) => {
      const p = ev?.detail;
      if (typeof p === 'string') applySelection(p);
    };
    window.addEventListener('workbench-drop-path', onWorkbenchPath as any, { capture: true });
    unlistenWorkbenchPath = () => window.removeEventListener('workbench-drop-path', onWorkbenchPath as any);

    const onPaste = (e: ClipboardEvent) => {
      const dt = e.clipboardData;
      if (!dt) return;
      let raw = dt.getData('text/uri-list') || dt.getData('text/plain') || '';
      if (!raw) return;
      let parsed = parseWorkbenchDrag(raw);
      if (!parsed && raw.includes('\n')) parsed = parseWorkbenchDrag(raw.split('\n')[0]);
      if (!parsed) return;
      e.preventDefault();
      resolveArmaPath(parsed).then((resolved) => {
        if (resolved) applySelection(resolved);
      });
    };
    window.addEventListener('paste', onPaste as any, { capture: true });
    // store remover into unlistenDrop to clean later
    const prevUnlisten = unlistenDrop;
    unlistenDrop = () => {
      prevUnlisten && prevUnlisten();
      window.removeEventListener('paste', onPaste as any);
    };
    wndDragOver = (e: DragEvent) => { e.preventDefault(); e.stopPropagation(); chooseDropEffect(e); isDragOver = isInsideRoot(e); };
    wndDragEnter = (e: DragEvent) => { e.preventDefault(); e.stopPropagation(); chooseDropEffect(e); isDragOver = isInsideRoot(e); };
    wndDragLeave = (e: DragEvent) => { e.preventDefault(); if (!isInsideRoot(e)) isDragOver = false; };
    wndDrop = (e: DragEvent) => { e.preventDefault(); e.stopPropagation(); isDragOver = false; const dt = (e as DragEvent).dataTransfer; if (dt) handleWorkbenchTextDrop(dt); };
    window.addEventListener('dragover', wndDragOver, { capture: true });
    window.addEventListener('dragenter', wndDragEnter, { capture: true });
    window.addEventListener('dragleave', wndDragLeave, { capture: true });
    window.addEventListener('drop', wndDrop, { capture: true });

    docDragOver = (e: DragEvent) => { e.preventDefault(); e.stopPropagation(); chooseDropEffect(e); isDragOver = isInsideRoot(e); };
    docDragEnter = (e: DragEvent) => { e.preventDefault(); e.stopPropagation(); chooseDropEffect(e); isDragOver = isInsideRoot(e); };
    docDragLeave = (e: DragEvent) => { e.preventDefault(); if (!isInsideRoot(e)) isDragOver = false; };
    docDrop = (e: DragEvent) => { e.preventDefault(); e.stopPropagation(); isDragOver = false; const dt = e.dataTransfer; if (dt) handleWorkbenchTextDrop(dt); };
    document.addEventListener('dragover', docDragOver, { capture: true });
    document.addEventListener('dragenter', docDragEnter, { capture: true });
    document.addEventListener('dragleave', docDragLeave, { capture: true });
    document.addEventListener('drop', docDrop, { capture: true });
    // compute available height for the table
    setTimeout(calcTableMax, 0);
    window.addEventListener('resize', calcTableMax);
  });

  onDestroy(() => {
    unlistenDrop?.();
    unlistenWebview?.();
    unlistenRemote?.();
    unlistenRemoteBatch?.();
    unlistenPreviewMode?.();
    unlistenPreviewNow?.();
    unlistenToggleMany?.();
    unlistenWorkbenchPath?.();
    wndDragOver && window.removeEventListener('dragover', wndDragOver);
    wndDragEnter && window.removeEventListener('dragenter', wndDragEnter);
    wndDragLeave && window.removeEventListener('dragleave', wndDragLeave);
    wndDrop && window.removeEventListener('drop', wndDrop);
    docDragOver && document.removeEventListener('dragover', docDragOver);
    docDragEnter && document.removeEventListener('dragenter', docDragEnter);
    docDragLeave && document.removeEventListener('dragleave', docDragLeave);
    docDrop && document.removeEventListener('drop', docDrop);
    window.removeEventListener('resize', calcTableMax);
    // Stop per-instance tunnel if running
    if (tunnelPid != null) {
      try { invoke('stop_quick_tunnel', { pid: tunnelPid }); } catch {}
      tunnelPid = null;
      tunnelUrl = '';
    }
  });
</script>

<div class="fulldst" bind:this={rootEl} role="region" aria-label="Full DST drop zone" ondragover={handleDragOver} ondragenter={handleDragEnter} ondrop={handleDrop}>
  <section class="content">
    <div class="row">
      <button class="capsule" onclick={pickFile}>Open</button>
      <button class="capsule" onclick={importFromClipboard}>Import from Workbench (Clipboard)</button>
      {#if filePath}
        <span class="path">{filePath}</span>
      {/if}
    </div>
    <p>ลากไฟล์ model หรือใช้ปุ่ม Open ในนี้เพื่อเปิดดูรายการหมวดหมู่ต่าง ๆ</p>
    <div class="row">
      <span class="lbl">Preview Mode:</span>
      <div class="segment">
        <button class="seg" class:active={realtimeMode} onclick={() => setRealtimeMode(true)}>Realtime</button>
        <button class="seg" class:active={!realtimeMode} onclick={() => setRealtimeMode(false)}>Un-realtime</button>
      </div>
    </div>
    <div class="row">
      <label class="lbl" for="wb-port">Workbench Port</label>
      <input id="wb-port" class="port" type="number" min="1" max="65535" bind:value={wbPort} onchange={saveWbPort} />
      <div class="push-right">
        <span class="warntext">โปรดกด Show All ก่อนส่งตรวจงาน</span>
        <button class="capsule success" onclick={showAll} disabled={!filePath || isLoading}>Show All</button>
        <button class="capsule danger" onclick={hideAll} disabled={!filePath || isLoading}>Hide All</button>
      </div>
    </div>
    <div class="row">
      <span class="lbl">Remote URL</span>
      {#if tunnelUrl}
        <input class="port" type="text" readonly value={tunnelUrl} onclick={(e) => (e.target as HTMLInputElement).select()} />
        <button class="capsule" onclick={copyTunnelUrl}>Copy</button>
        <button class="qr-btn" title="Show QR" aria-label="Show QR" onclick={() => showQr = true}>
          <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor" aria-hidden="true">
            <path d="M3 3h8v8H3V3zm2 2v4h4V5H5zm6 0h2v2h-2V5zm4-2h6v6h-6V3zm2 2v2h2V5h-2zM3 13h8v8H3v-8zm2 2v4h4v-4H5zm8-2h2v2h-2v-2zm0 4h2v2h-2v-2zm4-4h2v2h-2v-2zm-4 4h6v6h-6v-6zm4 2v2h2v-2h-2z"/>
          </svg>
        </button>
      {:else}
        {#if isTunnelLoading}
          <button class="capsule" disabled><span class="spinner sm"></span> Generating...</button>
        {:else}
          <button class="capsule" onclick={startRemote}>Start Remote</button>
        {/if}
      {/if}
      <div class="push-right">
        <button class="capsule" onclick={refreshList} disabled={!filePath || isLoading}>{isLoading ? 'Loading…' : 'Refresh'}</button>
        {#if !realtimeMode}
          <button class="capsule warn" onclick={exportToWorkbench} disabled={!filePath || isLoading}>Preview</button>
        {/if}
      </div>
    </div>
  </section>
  <!-- Resizable 2-column tree table -->
  <div class="datatable" bind:this={tableEl} style={`--group-pct:${colGroup}% ; max-height:${tableMax ? tableMax + 'px' : 'none'}`} aria-busy={isLoading ? 'true' : 'false'}>
    <div class="trow thead" aria-rowindex="1">
      <div class="tcell tgroup thead">Group</div>
      <div class="tcell tstatus thead">Status</div>
    </div>
    {#each getVisible() as item, i}
      <div class="trow" role="button" tabindex="0" data-state={item.node.status} onclick={() => onToggleNode(item.node)} onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onToggleNode(item.node); } }}>
        <div class="tcell tgroup" style={`--indent:${item.depth * 16}px`}>
          {#if item.node.children?.length}
            <button class="expander" aria-label={item.node.expanded ? 'Collapse' : 'Expand'} aria-expanded={item.node.expanded ? 'true' : 'false'} onclick={(e) => { e.stopPropagation(); toggleNode(item.node); }} onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.stopPropagation(); e.preventDefault(); toggleNode(item.node); } }}>
              <span class="caret" class:open={item.node.expanded}></span>
            </button>
          {:else}
            <span class="leaf-spacer"></span>
          {/if}
          <span class="label">{item.node.name}</span>
        </div>
        <div class="tcell tstatus">
          <button class="toggle-btn" disabled={isLoading} data-state={item.node.status} onclick={(e) => { e.stopPropagation(); onToggleNode(item.node); }} onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.stopPropagation(); e.preventDefault(); onToggleNode(item.node); } }}>
            {item.node.status === 'Open' ? 'Open' : 'Hide'}
          </button>
        </div>
      </div>
    {/each}
    <button type="button" class="tresizer" aria-label="Resize columns" onmousedown={onResizeDown} onkeydown={onResizeKey}></button>
    <div class="loading-overlay" class:show={isLoading} aria-hidden={!isLoading}>
      <div class="loading-box"><span class="spinner"></span><span>Loading...</span></div>
    </div>
  </div>
  <div class="drop-capture" aria-hidden="true" class:show={isDragOver}></div>
  {#if showQr}
    <div class="qr-overlay" role="dialog" aria-modal="true" aria-label="Remote URL QR" tabindex="0" onclick={() => showQr = false} onkeydown={(e) => { if (e.key === 'Escape' || e.key === 'Esc' || e.key === 'Enter' || e.key === ' ') { e.preventDefault(); showQr = false; } }}>
      <div class="qr-box" role="document" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={(e) => { if (e.key === 'Escape') { e.preventDefault(); showQr = false; } }}>
        <img alt="Remote URL QR" src={`https://api.qrserver.com/v1/create-qr-code/?size=600x600&data=${encodeURIComponent(tunnelUrl)}`}/>
        <div class="qr-caption">Click anywhere to close</div>
      </div>
    </div>
  {/if}
  {#if showFilePopup}
    <div class="file-popup-overlay" role="dialog" aria-modal="true" aria-label="Pre-Usage File Validation" tabindex="0" onclick={() => showFilePopup = false} onkeydown={(e) => { if (e.key === 'Escape') { e.preventDefault(); showFilePopup = false; } }}>
      <div class="file-popup-box" role="document" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={(e) => { if (e.key === 'Escape') { e.preventDefault(); showFilePopup = false; } }}>
        <div class="popup-header">
          <img class="popup-icon-img" src="/icon/ChineseNewYear/owl_Chinese.png" alt="Owl" />
          <h2 class="popup-title">Check file</h2>
        </div>
        
        <div class="validation-content">
          <div class="checklist-container">
            {#each checklistItems as item (item.id)}
              <div class="checklist-item" class:passed={item.passed} class:failed={!item.passed && item.checked}>
                <div class="checklist-row">
                  <span class="status-icon">
                    {#if item.passed}
                      ✅
                    {:else if item.checked}
                      ❌
                    {:else}
                      <span class="icon-unknown">?</span>
                    {/if}
                  </span>
                  <span class="label-cell">
                    {item.label}
                  </span>
                  {#if item.fixed}
                    <span class="fixed-badge">Fixed</span>
                  {/if}
                </div>
              </div>
            {/each}
          </div>
          
          {#if checkMessage}
            <div class="check-message" class:success={checklistItems.every(i => i.passed)} class:warning={!checklistItems.every(i => i.passed)}>
              {checkMessage}
            </div>
          {/if}
        </div>
        
        <div class="popup-actions">
          {#if !checklistItems.every(i => i.passed)}
            <button 
              class="popup-fix-btn" 
              disabled={isFixingFile || isCheckingFile}
              onclick={async () => { await fixFileContent(filePath); }}
            >
              {isFixingFile ? 'Fixing...' : 'Auto-Fix'}
            </button>
          {/if}
          <button 
            class="popup-close-btn" 
            disabled={isFixingFile || isCheckingFile}
            onclick={() => { showFilePopup = false; }}
          >
            {checklistItems.every(i => i.passed) ? 'Continue' : 'Cancel'}
          </button>
        </div>
      </div>
    </div>
  {/if}
  
</div>

<style>
  .fulldst {
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100%;
    min-height: 0;
    position: relative;
  }
  
  .row { display: flex; align-items: center; gap: 12px; margin-bottom: 8px; }
  .push-right { margin-left: auto; display: inline-flex; gap: 8px; }
  .capsule { padding: 6px 14px; border-radius: 9999px; border: 1px solid #3a3a3a; background: #1f1f1f; color: #eaeaea; cursor: pointer; }
  .capsule:hover { background: #2a2a2a; }
  .capsule.success { background: rgba(46,125,50,0.25); border-color: rgba(76,175,80,0.6); color: #fff; }
  .capsule.success:hover { background: rgba(46,125,50,0.35); }
  .capsule.danger { background: rgba(183,28,28,0.25); border-color: rgba(244,67,54,0.6); color: #fff; }
  .capsule.danger:hover { background: rgba(183,28,28,0.35); }
  .capsule.warn { background: rgba(255,152,0,0.25); border-color: rgba(255,152,0,0.6); color: #fff; min-width: 184px; text-align: center; }
  .capsule.warn:hover { background: rgba(255,152,0,0.35); }
  .warntext { color: #ff5252; font-weight: 600; }
  .segment { display: inline-flex; border-radius: 9999px; border: 1px solid #444; overflow: hidden; }
  .seg { padding: 6px 12px; background: #333; color: #fff; border: 0; cursor: pointer; }
  .seg + .seg { border-left: 1px solid #444; }
  .seg.active { background: #8ab4f8; color: #111; }
  @media (prefers-color-scheme: light) {
    .capsule { background: #f5f5f5; border-color: #cfcfcf; color: #222; }
    .capsule:hover { background: #ececec; }
    .capsule.success { background: rgba(76,175,80,0.12); border-color: rgba(76,175,80,0.55); color: #1b5e20; }
    .capsule.success:hover { background: rgba(76,175,80,0.2); }
    .capsule.danger { background: rgba(244,67,54,0.12); border-color: rgba(244,67,54,0.55); color: #b71c1c; }
    .capsule.danger:hover { background: rgba(244,67,54,0.2); }
    .capsule.warn { background: rgba(255,152,0,0.12); border-color: rgba(255,152,0,0.55); color: #ef6c00; }
    .capsule.warn:hover { background: rgba(255,152,0,0.2); }
    .segment { border-color: #cfcfcf; }
    .seg { background: #f5f5f5; color: #222; }
    .seg + .seg { border-left-color: #cfcfcf; }
    .seg.active { background: #4a90e2; color: #fff; }
    .port { background: #fff; color: #222; border-color: #cfcfcf; }
    .port:focus { border-color: #4a90e2; }
  }
  :global(body.theme-dark) .capsule.success,
  :global(body.theme-dark) .capsule.danger { color: #fff !important; }
  .lbl { opacity: 0.9; }
  .port { width: 110px; padding: 6px 10px; border-radius: 8px; border: 1px solid #4a4a4a; background: #2f2f2f; color: #eaeaea; }
  .port:focus { outline: none; border-color: #8ab4f8; }
  .path { opacity: 0.9; word-break: break-all; }
  .content { flex: 0 0 auto; }
  .drop-capture { position: absolute; inset: 0; opacity: 0; pointer-events: none; z-index: 4; border: 2px dashed rgba(138,180,248,0.8); border-radius: 8px; background: rgba(138,180,248,0.08); transition: opacity .08s; }
  .drop-capture.show { opacity: 1; pointer-events: none; }
  /* Table styles */
  .datatable { position: relative; display: flex; flex-direction: column; width: 100%; box-sizing: border-box; border: 1px solid #3f3f3f; border-radius: 8px; background: rgba(255,255,255,0.02); margin-top: 12px; flex: 1 1 auto; min-height: 0; overflow: auto; overscroll-behavior: contain; }
  .trow { position: relative; width: 100%; display: grid; align-items: center; grid-template-columns: var(--group-pct, 80%) calc(100% - var(--group-pct, 80%)); border-top: 1px solid #2f2f2f; transition: background-color 0.12s ease; }
  .trow:first-child { border-top: 0; }
  .trow::before { content: ""; position: absolute; left: 0; top: 0; bottom: 0; width: 4px; background: transparent; opacity: .9; }
  .trow.thead::before { display: none; }
  .trow[data-state="Open"]::before { background: rgba(76,175,80,0.95); }
  .trow[data-state="Hide"]::before { background: rgba(244,67,54,0.95); }
  /* Row background tint matching the toggle button colors */
  .trow:not(.thead)[data-state="Open"] { background-color: rgba(46, 125, 50, 0.18); }
  .trow:not(.thead)[data-state="Hide"] { background-color: rgba(183, 28, 28, 0.18); }
  /* Hover effect for rows */
  .trow:not(.thead):hover { background-color: rgba(138, 180, 248, 0.12); }
  .tcell { padding: 12px 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; box-sizing: border-box; }
  .tcell.thead { position: sticky; top: 0; z-index: 1; font-weight: 600; background: rgba(255,255,255,0.06); backdrop-filter: saturate(120%) blur(2px); }
  .tcell.tstatus { text-align: right; opacity: 0.9; }
  .tcell.tgroup { display: flex; align-items: center; gap: 6px; padding-left: calc(10px + var(--indent, 0px)); }
  .expander { width: 20px; height: 20px; display: inline-flex; align-items: center; justify-content: center; border: 1px solid #4a4a4a; background: #2f2f2f; border-radius: 4px; cursor: pointer; padding: 0; }
  .expander:hover { background: #3a3a3a; }
  .leaf-spacer { width: 20px; height: 20px; display: inline-block; }
  .caret { display: inline-block; width: 0; height: 0; border-top: 5px solid transparent; border-bottom: 5px solid transparent; border-left: 6px solid #cbd5ff; transform: rotate(0deg); transition: transform .12s; }
  .caret.open { transform: rotate(90deg); }
  .tresizer { position: absolute; top: 0; bottom: 0; width: 6px; left: var(--group-pct, 80%); margin-left: -3px; cursor: col-resize; background: transparent; border: 0; padding: 0; z-index: 2; }
  .tresizer::after { content: ''; position: absolute; top: 0; bottom: 0; left: 2px; width: 2px; background: rgba(138,180,248,0.35); opacity: 0; transition: opacity .12s; }
  .tresizer:hover::after { opacity: 1; }
  .toggle-btn { min-width: 70px; padding: 4px 10px; border-radius: 999px; border: 1px solid #4a4a4a; background: #2c2c2c; color: #eaeaea; cursor: pointer; transition: background .12s, border-color .12s, color .12s, opacity .12s; }
  .toggle-btn[data-state="Open"] { background: rgba(46, 125, 50, 0.25); border-color: rgba(76,175,80,0.6); color: #c8f7d1; }
  .toggle-btn[data-state="Hide"] { background: rgba(183, 28, 28, 0.25); border-color: rgba(244,67,54,0.6); color: #ffd2d2; }
  .toggle-btn:hover { filter: brightness(1.05); }
  .toggle-btn:disabled { opacity: 0.6; cursor: default; }
  .loading-overlay { position: absolute; inset: 0; background: rgba(0,0,0,0.35); display: none; align-items: center; justify-content: center; z-index: 5; }
  .loading-overlay.show { display: flex; }
  .loading-box { display: flex; gap: 10px; align-items: center; padding: 10px 14px; background: rgba(32,32,32,0.9); border: 1px solid #4a4a4a; border-radius: 8px; color: #fff; }
  .spinner { width: 14px; height: 14px; border: 2px solid #8ab4f8; border-right-color: transparent; border-radius: 50%; animation: spin .8s linear infinite; display: inline-block; vertical-align: -2px; margin-right: 6px; }
  .spinner.sm { width: 12px; height: 12px; border-width: 2px; vertical-align: -1px; margin-right: 6px; }
  @keyframes spin { from { transform: rotate(0deg); } to { transform: rotate(360deg); } }
  /* QR UI */
  .qr-btn { width: 28px; height: 28px; margin-left: 6px; display: inline-flex; align-items: center; justify-content: center; border: 1px solid #3a3a3a; background: #1f1f1f; color: #eaeaea; border-radius: 6px; cursor: pointer; }
  .qr-btn:hover { background: #2a2a2a; }
  @media (prefers-color-scheme: light) {
    .qr-btn { background: #f5f5f5; border-color: #cfcfcf; color: #222; }
    .qr-btn:hover { background: #ececec; }
  }
  /* Force overrides when app uses theme-dark irrespective of OS scheme */
  :global(body.theme-dark) .capsule { background: #1b1b1b; border-color: #343434; color: #eaeaea; }
  :global(body.theme-dark) .capsule:hover { background: #262626; }
  :global(body.theme-dark) .qr-btn { background: #1b1b1b; border-color: #343434; color: #eaeaea; }
  :global(body.theme-dark) .qr-btn:hover { background: #262626; }
  /* Emphasize Show All / Hide All colors in dark theme */
  :global(body.theme-dark) .capsule.success { background: #2e7d32; border-color: #4caf50; color: #ffffff; }
  :global(body.theme-dark) .capsule.success:hover { background: #388e3c; }
  :global(body.theme-dark) .capsule.danger { background: #c62828; border-color: #f44336; color: #ffffff; }
  :global(body.theme-dark) .capsule.danger:hover { background: #d32f2f; }
  /* Preview button (warn) should be orange in dark theme */
  :global(body.theme-dark) .capsule.warn { background: #ef6c00; border-color: #ff9800; color: #ffffff; }
  :global(body.theme-dark) .capsule.warn:hover { background: #fb8c00; }
  /* Force light theme inputs (port & url) to bright when app is theme-light */
  :global(body.theme-light) .port { background: #ffffff; color: #222; border-color: #cfcfcf; }
  :global(body.theme-light) .port:focus { border-color: #4a90e2; }
  .qr-overlay { position: fixed; inset: 0; background: rgba(0,0,0,0.65); z-index: 1000; display: flex; align-items: center; justify-content: center; padding: 24px; }
  .qr-box { background: rgba(20,20,20,0.9); border: 1px solid #4a4a4a; border-radius: 12px; padding: 16px; display: flex; flex-direction: column; align-items: center; gap: 8px; max-width: 92vmin; }
  .qr-box img { width: min(80vmin, 600px); height: min(80vmin, 600px); image-rendering: pixelated; }
  .qr-caption { color: #cbd5ff; opacity: 0.85; font-size: 12px; }
  /* File Popup Styles */
  .file-popup-overlay { position: fixed; inset: 0; background: rgba(0,0,0,0.65); z-index: 1001; display: flex; align-items: center; justify-content: center; padding: 24px; }
  .file-popup-box { background: rgba(20,20,20,0.95); border: 1px solid #4a4a4a; border-radius: 12px; padding: 32px; display: flex; flex-direction: column; align-items: center; gap: 24px; max-width: 400px; box-shadow: 0 8px 32px rgba(0,0,0,0.5); }
  .popup-icon-img { width: 128px; height: 128px; object-fit: contain; }
  .popup-header { display: flex; align-items: center; gap: 16px; width: 100%; }
  .popup-title { margin: 0; font-size: 18px; font-weight: 600; color: #eaeaea; }
  .validation-content { width: 100%; }
  .checklist-container { width: 100%; display: flex; flex-direction: column; gap: 8px; }
  .checklist-item { padding: 12px; border-radius: 8px; background: rgba(255,255,255,0.02); border: 1px solid #3a3a3a; transition: background 0.12s, border-color 0.12s; }
  .checklist-item.passed { background: rgba(76,175,80,0.08); border-color: rgba(76,175,80,0.3); }
  .checklist-item.failed { background: rgba(244,67,54,0.08); border-color: rgba(244,67,54,0.3); }
  .checklist-row { display: flex; align-items: center; gap: 12px; }
  .status-icon { flex: 0 0 auto; width: 24px; height: 24px; display: flex; align-items: center; justify-content: center; font-weight: 600; font-size: 18px; }
  .icon-unknown { color: #ff9800; }
  .label-cell { flex: 1; color: #eaeaea; font-size: 14px; user-select: none; }
  .fixed-badge { flex: 0 0 auto; padding: 2px 8px; border-radius: 4px; background: rgba(76,175,80,0.25); color: #4caf50; font-size: 11px; font-weight: 600; }
  .check-message { padding: 12px; border-radius: 8px; margin-top: 12px; font-size: 13px; text-align: center; }
  .check-message.success { background: rgba(76,175,80,0.12); color: #4caf50; border: 1px solid rgba(76,175,80,0.3); }
  .check-message.warning { background: rgba(244,67,54,0.12); color: #ff5252; border: 1px solid rgba(244,67,54,0.3); }
  .popup-actions { display: flex; gap: 12px; width: 100%; margin-top: 8px; }
  .popup-fix-btn, .popup-close-btn { flex: 1; padding: 10px 16px; border-radius: 6px; border: 1px solid #8ab4f8; background: rgba(138,180,248,0.15); color: #8ab4f8; cursor: pointer; font-weight: 500; transition: background 0.12s, color 0.12s; }
  .popup-fix-btn:hover:not(:disabled), .popup-close-btn:hover:not(:disabled) { background: rgba(138,180,248,0.25); }
  .popup-fix-btn:active:not(:disabled), .popup-close-btn:active:not(:disabled) { background: rgba(138,180,248,0.35); }
  .popup-fix-btn:disabled, .popup-close-btn:disabled { opacity: 0.5; cursor: default; }
  .popup-fix-btn { background: rgba(76,175,80,0.15); border-color: #4caf50; color: #4caf50; }
  .popup-fix-btn:hover:not(:disabled) { background: rgba(76,175,80,0.25); }
  .popup-fix-btn:active:not(:disabled) { background: rgba(76,175,80,0.35); }
  @media (prefers-color-scheme: light) {
    .file-popup-box { background: rgba(245,245,245,0.98); border-color: #cfcfcf; }
    .popup-title { color: #222; }
    .label-cell { color: #222; }
    .checklist-item { background: rgba(0,0,0,0.02); border-color: #e0e0e0; }
    .checklist-item.passed { background: rgba(76,175,80,0.08); border-color: rgba(76,175,80,0.3); }
    .checklist-item.failed { background: rgba(244,67,54,0.08); border-color: rgba(244,67,54,0.3); }
    .check-message { background: rgba(76,175,80,0.08); color: #2e7d32; border-color: rgba(76,175,80,0.3); }
    .check-message.warning { background: rgba(244,67,54,0.08); color: #c62828; border-color: rgba(244,67,54,0.3); }
    .popup-fix-btn, .popup-close-btn { border-color: #4a90e2; background: rgba(74,144,226,0.12); color: #4a90e2; }
    .popup-fix-btn:hover:not(:disabled), .popup-close-btn:hover:not(:disabled) { background: rgba(74,144,226,0.2); }
    .popup-fix-btn:active:not(:disabled), .popup-close-btn:active:not(:disabled) { background: rgba(74,144,226,0.3); }
    .popup-fix-btn { background: rgba(76,175,80,0.12); border-color: #4caf50; color: #2e7d32; }
    .popup-fix-btn:hover:not(:disabled) { background: rgba(76,175,80,0.2); }
    .popup-fix-btn:active:not(:disabled) { background: rgba(76,175,80,0.3); }
  }
  :global(body.theme-dark) .file-popup-box { background: #1a1a1a; border-color: #343434; }
  :global(body.theme-dark) .popup-title { color: #ffffff; }
  :global(body.theme-dark) .label-cell { color: #ffffff; }
  :global(body.theme-dark) .popup-fix-btn, :global(body.theme-dark) .popup-close-btn { border-color: #8ab4f8; background: rgba(138,180,248,0.2); color: #8ab4f8; }
  :global(body.theme-dark) .popup-fix-btn:hover:not(:disabled), :global(body.theme-dark) .popup-close-btn:hover:not(:disabled) { background: rgba(138,180,248,0.3); }
  :global(body.theme-dark) .popup-fix-btn { background: rgba(76,175,80,0.2); border-color: #4caf50; color: #81c784; }
  :global(body.theme-dark) .popup-fix-btn:hover:not(:disabled) { background: rgba(76,175,80,0.3); }
</style>
