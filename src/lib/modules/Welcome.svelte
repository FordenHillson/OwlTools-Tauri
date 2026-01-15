<script lang="ts">
  import { createEventDispatcher, onDestroy, onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
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

  type Heart = {
    id: string;
    left: number;
    size: number;
    durationMs: number;
    driftPx: number;
    risePx: number;
    rotDeg: number;
  };

  let hearts: Heart[] = [];
  let heartInterval: number | null = null;
  const heartTimeouts = new Set<number>();
  let welcomeEl: HTMLDivElement;
  let resizeObserver: ResizeObserver | null = null;
  let welcomeW = 0;
  let welcomeH = 0;

  function clamp(n: number, min: number, max: number) {
    return Math.max(min, Math.min(max, n));
  }

  function spawnHeart() {
    const id = `h-${Math.random().toString(36).slice(2, 9)}-${Date.now()}`;
    const left = Math.random() * 100;
    const baseSize = 12 + Math.floor(Math.random() * 18);
    const sizeBoost = welcomeW > 0 ? clamp(Math.round(welcomeW / 120), 0, 10) : 0;
    const size = baseSize + sizeBoost;
    const risePx = welcomeH > 0 ? clamp(Math.round(welcomeH * 0.55), 180, 520) : 240;
    const durationMs = welcomeH > 0
      ? clamp(1800 + Math.floor(welcomeH * 2.2) + Math.floor(Math.random() * 900), 2200, 5200)
      : (2400 + Math.floor(Math.random() * 1800));
    const driftRange = welcomeW > 0 ? clamp(Math.round(welcomeW * 0.12), 24, 160) : 48;
    const driftPx = -Math.floor(driftRange / 2) + Math.floor(Math.random() * driftRange);
    const rotDeg = -25 + Math.floor(Math.random() * 50);
    const heart: Heart = { id, left, size, durationMs, driftPx, risePx, rotDeg };
    hearts = [...hearts, heart].slice(-40);
    const t = window.setTimeout(() => {
      hearts = hearts.filter((h) => h.id !== id);
      heartTimeouts.delete(t);
    }, durationMs + 150);
    heartTimeouts.add(t);
  }

  let latestAvailable = '';
  let updaterHasNew = false;

  onMount(async () => {
    if (typeof ResizeObserver !== 'undefined') {
      resizeObserver = new ResizeObserver((entries) => {
        const entry = entries[0];
        const r = entry?.contentRect;
        if (!r) return;
        welcomeW = r.width;
        welcomeH = r.height;
      });
      if (welcomeEl) resizeObserver.observe(welcomeEl);
    }

    heartInterval = window.setInterval(() => {
      if (Math.random() < 0.85) spawnHeart();
    }, 380);

    try {
      const v = await invoke<string>('get_display_version');
      if (typeof v === 'string' && v.trim()) {
        versionStamp = v.trim();
      }
    } catch {}
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

  onDestroy(() => {
    if (heartInterval != null) {
      window.clearInterval(heartInterval);
      heartInterval = null;
    }
    if (resizeObserver) {
      resizeObserver.disconnect();
      resizeObserver = null;
    }
    for (const t of heartTimeouts) window.clearTimeout(t);
    heartTimeouts.clear();
  });

  const shortcuts: Array<{ key: string; label: string; glyph?: string; iconSrc?: string }> = [
    { key: 'fulldst', label: 'Full DST View', iconSrc: '/FullDST.svg' },
    { key: 'socket', label: 'Socket Manager', iconSrc: '/Socket_Manager.svg' }
  ];

  const updaterShortcut: { key: string; label: string; glyph?: string; iconSrc?: string } = {
    key: 'updater',
    label: 'Updater',
    glyph: '⬆'
  };
</script>

<div class="welcome" bind:this={welcomeEl}>
  <div class="hearts" aria-hidden="true">
    {#each hearts as h (h.id)}
      <span
        class="heart"
        style={`--x:${h.left}%;--size:${h.size}px;--dur:${h.durationMs}ms;--drift:${h.driftPx}px;--rise:${h.risePx}px;--rot:${h.rotDeg}deg;`}
      >❤️</span>
    {/each}
  </div>
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

  <div class="actions actions-updater" aria-label="Updater">
    <button class="shortcut" type="button" on:click={() => openModule(updaterShortcut.key)} aria-label={updaterShortcut.label}>
      <span class="icon" aria-hidden="true">
        {#if updaterShortcut.iconSrc}
          <img class="icon-img" src={updaterShortcut.iconSrc} alt="" loading="lazy" />
        {:else}
          {updaterShortcut.glyph}
        {/if}
        {#if updaterHasNew}
          <span class="badge" aria-label="New update">NEW</span>
        {/if}
      </span>
      <span class="label">{updaterShortcut.label}</span>
    </button>
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
    height: 88vh;
    min-height: 100%;
    flex: 1;
    gap: 12px;
    position: relative;
    padding-bottom: 20px;
  }

  .hearts {
    position: absolute;
    inset: 0;
    overflow: hidden;
    pointer-events: none;
    z-index: 0;
  }

  .welcome > :global(*):not(.hearts) {
    position: relative;
    z-index: 1;
  }

  .heart {
    position: absolute;
    left: var(--x);
    bottom: -18px;
    font-size: var(--size);
    line-height: 1;
    transform: translate3d(0, 0, 0) rotate(var(--rot));
    filter: drop-shadow(0 1px 1px rgba(0, 0, 0, 0.25));
    opacity: 0;
    animation: floatHeart var(--dur) linear forwards;
    will-change: transform, opacity;
  }

  @keyframes floatHeart {
    0% {
      opacity: 0;
      transform: translate3d(0, 0, 0) rotate(var(--rot));
    }
    10% {
      opacity: 0.95;
    }
    100% {
      opacity: 0;
      transform: translate3d(var(--drift), calc(var(--rise) * -1), 0) rotate(calc(var(--rot) + 18deg));
    }
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

  .actions-updater {
    width: min(240px, calc(100% - 24px));
    grid-template-columns: 1fr;
    padding-top: 0;
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
