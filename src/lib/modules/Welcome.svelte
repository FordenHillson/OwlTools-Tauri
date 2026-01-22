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

  type Env = {
    id: string;
    left: number;
    size: number;
    durationMs: number;
    driftPx: number;
    fallPx: number;
    rotDeg: number;
  };

  let envs: Env[] = [];
  let envInterval: number | null = null;
  const envTimeouts = new Set<number>();
  let welcomeEl: HTMLDivElement;
  let resizeObserver: ResizeObserver | null = null;
  let welcomeW = 0;
  let welcomeH = 0;
  let logoSrc = '/icon/ChineseNewYear/owl_Chinese.png';
  let logoAudio: HTMLAudioElement | null = null;
  let logoAudioNeedsGesture = false;
  function startLogoAudioOnGesture() {
    if (!logoAudio) return;
    logoAudio.play().catch(() => {});
    window.removeEventListener('click', startLogoAudioOnGesture);
    window.removeEventListener('keydown', startLogoAudioOnGesture);
    logoAudioNeedsGesture = false;
  }

  function clamp(n: number, min: number, max: number) {
    return Math.max(min, Math.min(max, n));
  }

  function spawnEnv() {
    const id = `e-${Math.random().toString(36).slice(2, 9)}-${Date.now()}`;
    const left = Math.random() * 100;
    const baseSize = 14 + Math.floor(Math.random() * 18);
    const sizeBoost = welcomeW > 0 ? clamp(Math.round(welcomeW / 110), 0, 10) : 0;
    const size = baseSize + sizeBoost;
    const fallPx = welcomeH > 0 ? clamp(Math.round(welcomeH * 0.65), 220, 580) : 260;
    const durationMs = welcomeH > 0
      ? clamp(2000 + Math.floor(welcomeH * 2.0) + Math.floor(Math.random() * 900), 2400, 5600)
      : (2600 + Math.floor(Math.random() * 1800));
    const driftRange = welcomeW > 0 ? clamp(Math.round(welcomeW * 0.14), 28, 180) : 56;
    const driftPx = -Math.floor(driftRange / 2) + Math.floor(Math.random() * driftRange);
    const rotDeg = -18 + Math.floor(Math.random() * 36);
    const env: Env = { id, left, size, durationMs, driftPx, fallPx, rotDeg };
    envs = [...envs, env].slice(-40);
    const t = window.setTimeout(() => {
      envs = envs.filter((h) => h.id !== id);
      envTimeouts.delete(t);
    }, durationMs + 150);
    envTimeouts.add(t);
  }

  let latestAvailable = '';
  let updaterHasNew = false;

  onMount(async () => {
    // Randomize Welcome logo: 10% dek_Im variant, 90% standard Chinese icon
    logoSrc = Math.random() < 0.1
      ? '/icon/ChineseNewYear/owl_Chinese_dek_Im.png'
      : '/icon/ChineseNewYear/owl_Chinese.png';

    if (logoSrc.endsWith('owl_Chinese_dek_Im.png')) {
      try {
        logoAudio = new Audio('/icon/ChineseNewYear/ching-chong_.mp3');
        logoAudio.loop = true;
        await logoAudio.play().catch(() => {
          logoAudioNeedsGesture = true;
        });
        if (logoAudioNeedsGesture) {
          window.addEventListener('click', startLogoAudioOnGesture);
          window.addEventListener('keydown', startLogoAudioOnGesture);
        }
      } catch {}
    }

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

    envInterval = window.setInterval(() => {
      if (Math.random() < 0.85) spawnEnv();
    }, 420);

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
    if (envInterval != null) {
      window.clearInterval(envInterval);
      envInterval = null;
    }
    if (resizeObserver) {
      resizeObserver.disconnect();
      resizeObserver = null;
    }
    for (const t of envTimeouts) window.clearTimeout(t);
    envTimeouts.clear();
    if (logoAudio) {
      try { logoAudio.pause(); } catch {}
      logoAudio = null;
    }
    if (logoAudioNeedsGesture) {
      window.removeEventListener('click', startLogoAudioOnGesture);
      window.removeEventListener('keydown', startLogoAudioOnGesture);
      logoAudioNeedsGesture = false;
    }
  });

  const shortcuts: Array<{ key: string; label: string; glyph?: string; iconSrc?: string }> = [
    { key: 'fulldst', label: 'Full DST View', iconSrc: '/FullDST.svg' },
    { key: 'prefabdst', label: 'Build Prefab DST', iconSrc: '/war.png' },
    { key: 'socket', label: 'Socket Manager', iconSrc: '/Socket_Manager.svg' }
  ];

  const updaterShortcut: { key: string; label: string; glyph?: string; iconSrc?: string } = {
    key: 'updater',
    label: 'Updater',
    glyph: '⬆'
  };
</script>

<div class="welcome" bind:this={welcomeEl}>
  <div class="envs" aria-hidden="true">
    {#each envs as e (e.id)}
      <span
        class="env"
        style={`--x:${e.left}%;--size:${e.size}px;--dur:${e.durationMs}ms;--drift:${e.driftPx}px;--fall:${e.fallPx}px;--rot:${e.rotDeg}deg;`}
      >🧧</span>
    {/each}
  </div>
  <img class="logo" src={logoSrc} alt="Owl" />
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

  .envs {
    position: absolute;
    inset: 0;
    overflow: hidden;
    pointer-events: none;
    z-index: 0;
  }

  .welcome > :global(*):not(.envs) {
    position: relative;
    z-index: 1;
  }

  .env {
    position: absolute;
    left: var(--x);
    top: -18px;
    font-size: var(--size);
    line-height: 1;
    transform: translate3d(0, 0, 0) rotate(var(--rot));
    filter: drop-shadow(0 1px 1px rgba(0, 0, 0, 0.25));
    opacity: 0;
    animation: fallEnv var(--dur) linear forwards;
    will-change: transform, opacity;
  }

  @keyframes fallEnv {
    0% {
      opacity: 0;
      transform: translate3d(0, 0, 0) rotate(var(--rot));
    }
    10% {
      opacity: 0.95;
    }
    100% {
      opacity: 0;
      transform: translate3d(var(--drift), var(--fall), 0) rotate(calc(var(--rot) + 18deg));
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
