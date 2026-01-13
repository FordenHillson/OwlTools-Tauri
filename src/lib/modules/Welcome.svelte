<script lang="ts">
  import { createEventDispatcher, onMount } from 'svelte';
  const dispatch = createEventDispatcher();
  function openModule(key: string) {
    if (key === 'updater') {
      try {
        if (latestAvailable) {
          localStorage.setItem('updaterLastSeenLatest', latestAvailable);
        }
      } catch {}
      updaterHasNew = false;
    }
    dispatch('open-module', { key });
  }

  let versionStamp = '';

  function pad2(n: number) {
    return String(n).padStart(2, '0');
  }

  function formatStamp(date: Date, minor: number) {
    return `${pad2(date.getDate())}.${pad2(date.getMonth() + 1)}.${date.getFullYear()}.${pad2(minor)}`;
  }

  const minorStr = (import.meta as any).env?.VITE_BUILD_MINOR as string | undefined;
  const minor = Number.parseInt(minorStr || '0', 10);
  versionStamp = formatStamp(new Date(), Number.isFinite(minor) ? minor : 0);

  let latestAvailable = '';
  let updaterHasNew = false;

  onMount(async () => {
    try {
      const manifestUrl = 'https://raw.githubusercontent.com/FordenHillson/OwlTools-Tauri/main/manifest.json';
      const url = `${manifestUrl}?ts=${Date.now()}`;
      const res = await fetch(url, { cache: 'no-store' });
      if (!res.ok) return;
      const j = (await res.json()) as any;
      const latest = (j && typeof j.latest === 'string') ? j.latest.trim() : '';
      if (!latest) return;
      latestAvailable = latest;
      let lastSeen = '';
      try {
        lastSeen = (localStorage.getItem('updaterLastSeenLatest') || '').trim();
      } catch {}
      updaterHasNew = lastSeen !== '' && lastSeen !== latest;
      if (lastSeen === '') {
        // First run: treat current latest as seen to avoid flashing NEW
        try { localStorage.setItem('updaterLastSeenLatest', latest); } catch {}
        updaterHasNew = false;
      }
    } catch {}
  });

  const shortcuts: Array<{ key: string; label: string; glyph?: string; iconSrc?: string }> = [
    { key: 'fulldst', label: 'Full DST View', iconSrc: '/FullDST.svg' },
    { key: 'socket', label: 'Socket Manager', iconSrc: '/Socket_Manager.svg' },
    { key: 'updater', label: 'Updater', glyph: '⬆' },
    { key: 'settings', label: 'Settings', glyph: '⚙' }
  ];
</script>

<div class="welcome">
  <img class="logo" src="/owl_Valen.png" alt="Owl" />
  <h1>Owl Tools</h1>  
  <p>Tool for Enfusion Engine</p>
  <div class="actions" aria-label="Modules">
    {#each shortcuts as s (s.key)}
      <button class="shortcut" type="button" on:click={() => openModule(s.key)} aria-label={s.label}>
        <span class="icon" aria-hidden="true">
          {#if s.iconSrc}
            <img class="icon-img" src={s.iconSrc} alt="" loading="lazy" />
          {:else}
            {s.glyph}
          {/if}
          {#if s.key === 'updater' && updaterHasNew}
            <span class="badge" aria-label="New update">NEW</span>
          {/if}
        </span>
        <span class="label">{s.label}</span>
      </button>
    {/each}
  </div>

  {#if versionStamp}
    <div class="version" aria-label="Version">
      OwlTools Version {versionStamp}
    </div>
  {/if}
</div>

<style>
  .welcome {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    text-align: center;
    width: 100%;
    min-height: 60vh;
    gap: 12px;
    position: relative;
    padding-bottom: 20px;
  }

  :global(body.theme-light) .welcome {
    color: #111;
  }
  .actions {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(120px, 1fr));
    gap: 18px;
    width: min(680px, calc(100% - 24px));
    padding: 8px 0;
  }
  .logo { width: 30vh; height: auto; }

  .shortcut {
    appearance: none;
    border: 0;
    background: transparent;
    color: inherit;
    cursor: pointer;
    padding: 10px 6px;
    border-radius: 12px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    transition: background 120ms ease, transform 120ms ease, box-shadow 120ms ease;
  }

  .shortcut:hover {
    background: rgba(255, 255, 255, 0.06);
    transform: translateY(-1px);
  }

  .shortcut:active {
    transform: translateY(0px);
  }

  .shortcut:focus-visible {
    outline: 2px solid rgba(255, 255, 255, 0.28);
    outline-offset: 2px;
  }

  .icon {
    width: 56px;
    height: 56px;
    border-radius: 999px;
    position: relative;
    display: grid;
    place-items: center;
    font-size: 20px;
    font-weight: 700;
    color: #111;
    background: #f1f3f4;
    box-shadow: 0 1px 0 rgba(0, 0, 0, 0.25);
    user-select: none;
    overflow: visible;
  }

  .icon-img {
    width: 32px;
    height: 32px;
    object-fit: contain;
    border-radius: 999px;
    display: block;
  }

  .badge {
    position: absolute;
    top: -6px;
    right: -6px;
    background: #e53935;
    color: #fff;
    border: 2px solid rgba(17, 17, 17, 0.25);
    border-radius: 999px;
    z-index: 2;
    font-size: 10px;
    line-height: 1;
    padding: 4px 6px;
    font-weight: 700;
    letter-spacing: 0.3px;
  }

  .label {
    font-size: 13px;
    line-height: 16px;
    color: rgba(255, 255, 255, 0.92);
    max-width: 120px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  :global(body.theme-light) .welcome .label {
    color: rgba(17, 17, 17, 0.92);
  }

  .version {
    position: absolute;
    bottom: 4px;
    left: 50%;
    transform: translateX(-50%);
    font-size: 12px;
    line-height: 14px;
    color: rgba(255, 255, 255, 0.65);
    user-select: none;
  }

  :global(body.theme-light) .welcome .version {
    color: rgba(17, 17, 17, 0.65);
  }
</style>
