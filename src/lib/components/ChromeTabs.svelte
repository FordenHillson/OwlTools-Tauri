<script lang="ts">
  import { onMount } from 'svelte';
  import Welcome from '$lib/modules/Welcome.svelte';
  import FullDSTView from '$lib/modules/FullDSTView.svelte';
  import SocketManager from '$lib/modules/SocketManager.svelte';
  import Updater from '$lib/modules/Updater.svelte';
  import { theme, toggleTheme } from '$lib/stores/theme';
  let container: HTMLDivElement;
  let tabs: any;
  let addBtn: HTMLButtonElement;
  let themeBtn: HTMLButtonElement;
  let dragRaf = 0;
  let classObserver: MutationObserver | null = null;
  let tabModels: Array<{ id: string; type: string; title: string }> = [];
  let activeTabId: string | null = null;
  let activeModel: { id: string; type: string; title: string } | null = null;
  $: activeModel = tabModels.find(t => t.id === activeTabId) || tabModels[0] || null;

  function onToggleTheme() {
    toggleTheme();
    requestAnimationFrame(() => requestAnimationFrame(updateAddButtonPosition));
  }

  onMount(() => {
    const ChromeTabs: any = (window as any).ChromeTabs;
    if (!ChromeTabs) {
      console.error('ChromeTabs library not loaded');
      return;
    }
    tabs = new ChromeTabs();
    tabs.init(container);

    const startDragLoop = () => {
      if (dragRaf) return;
      const tick = () => {
        if (!container || !container.classList.contains('chrome-tabs-is-sorting')) {
          dragRaf = 0;
          return;
        }
        updateAddButtonPosition();
        dragRaf = requestAnimationFrame(tick);
      };
      dragRaf = requestAnimationFrame(tick);
    };

    classObserver = new MutationObserver(() => {
      if (container.classList.contains('chrome-tabs-is-sorting')) {
        startDragLoop();
      }
    });
    classObserver.observe(container, { attributes: true, attributeFilter: ['class'] });

    const onActive = (e: any) => {
      const el = e.detail?.tabEl as HTMLElement | null;
      if (!el) return;
      const id = el.getAttribute('data-tab-id');
      activeTabId = id;
    };
    const onRemoved = (e: any) => {
      const el = e.detail?.tabEl as HTMLElement | null;
      if (!el) return;
      const id = el.getAttribute('data-tab-id');
      if (!id) return;
      tabModels = tabModels.filter(t => t.id !== id);
      if (tabModels.length === 0) {
        addWelcomeTab();
      }
    };

    container.addEventListener('activeTabChange', onActive as EventListener);
    container.addEventListener('tabRemove', onRemoved as EventListener);

    addWelcomeTab();

    const scheduleUpdate = () => requestAnimationFrame(() => requestAnimationFrame(updateAddButtonPosition));

    // update on common events
    window.addEventListener('resize', scheduleUpdate);
    container.addEventListener('tabAdd', scheduleUpdate as EventListener);
    container.addEventListener('tabRemove', scheduleUpdate as EventListener);
    container.addEventListener('tabReorder', scheduleUpdate as EventListener);

    scheduleUpdate();

    return () => {
      if (dragRaf) cancelAnimationFrame(dragRaf);
      dragRaf = 0;
      if (classObserver) classObserver.disconnect();
      classObserver = null;
    };
  });

  function addNewTab() {
    if (!tabs) return;
    const id = `tab-${Math.random().toString(36).slice(2, 9)}`;
    tabModels = [...tabModels, { id, type: 'welcome', title: 'Welcome' }];
    activeTabId = id;
    tabs.addTab({ title: 'Welcome', id });
    requestAnimationFrame(updateAddButtonPosition);
  }

  function addWelcomeTab() {
    if (!tabs) return;
    const id = `tab-${Math.random().toString(36).slice(2, 9)}`;
    tabModels = [...tabModels, { id, type: 'welcome', title: 'Welcome' }];
    activeTabId = id;
    tabs.addTab({ title: 'Welcome', id });
  }

  

  function handleOpenModule(e: CustomEvent) {
    const key = (e.detail && (e.detail as any).key) as string;
    if (!key) return;
    const current = activeModel;
    if (!current) return;
    const title = key === 'fulldst' ? 'Full DST View' : key === 'socket' ? 'Socket Manager' : key === 'updater' ? 'Updater' : key;
    const updated = tabModels.map(t => t.id === current.id ? { ...t, type: key, title } : t);
    tabModels = updated;
    if (tabs && tabs.activeTabEl) {
      tabs.updateTab(tabs.activeTabEl, { title });
    }
  }

  function updateAddButtonPosition() {
    if (!container || !addBtn) return;
    const lastTab = container.querySelector('.chrome-tabs-content .chrome-tab:last-child:not(.chrome-tab-is-dragging)') as HTMLElement | null;
    const content = container.querySelector('.chrome-tabs-content') as HTMLElement | null;
    if (!lastTab || !content) return;

    const containerRect = container.getBoundingClientRect();
    const tabRect = lastTab.getBoundingClientRect();
    const btnRect = addBtn.getBoundingClientRect();
    const themeRect = themeBtn ? themeBtn.getBoundingClientRect() : ({ width: 0 } as DOMRect);

    const margin = 6;
    const btnWidth = Math.ceil(btnRect.width);
    const themeWidth = Math.ceil(themeRect.width || 0);
    const reserve = btnWidth + themeWidth + margin * 3;
    container.style.setProperty('--newtab-space', `${reserve}px`);

    // Avoid forcing layout during drag-sort; the library is already managing tab transforms.
    if (!container.classList.contains('chrome-tabs-is-sorting')) {
      if (tabs && typeof tabs.layoutTabs === 'function') {
        tabs.layoutTabs();
      }
    }
    const desiredLeft = (tabRect.left - containerRect.left) + lastTab.offsetWidth + margin;

    const maxLeft = container.clientWidth - btnWidth - themeWidth - margin * 2;
    const clampedLeft = Math.max(margin, Math.min(desiredLeft, maxLeft));

    addBtn.style.right = 'auto';
    addBtn.style.left = `${clampedLeft}px`;
  }
</script>

<div class="tab-shell">
  <div class="chrome-tabs" class:chrome-tabs-dark-theme={$theme === 'dark'} bind:this={container} style="width:100%;">
    <div class="chrome-tabs-content"></div>
    <button class="add-tab" bind:this={addBtn} aria-label="New tab" title="New tab" on:click={addNewTab}>
      <span>+</span>
    </button>
    <button class="theme-toggle" bind:this={themeBtn} on:click={onToggleTheme} aria-label="Toggle theme" title="Toggle theme">
      { $theme === 'dark' ? 'Light' : 'Dark' }
    </button>
  </div>

  <div class="tab-panes">
    {#each tabModels as t (t.id)}
      <div class="tab-pane" class:center={t.type === 'welcome'} aria-hidden={t.id !== activeTabId} style={`display:${t.id === activeTabId ? 'flex' : 'none'}`}> 
        {#if t.type === 'welcome'}
          <Welcome on:open-module={handleOpenModule} />
        {:else if t.type === 'fulldst'}
          <FullDSTView />
        {:else if t.type === 'socket'}
          <SocketManager />
        {:else if t.type === 'updater'}
          <Updater />
        {:else}
          <div class="placeholder">{t.title || 'Loading...'}</div>
        {/if}
      </div>
    {/each}
  </div>
</div>

<style>
  .chrome-tabs .chrome-tabs-content {
    width: calc(100% - var(--newtab-space, 36px));
  }

  .add-tab {
    position: absolute;
    right: auto;
    top: 8px;
    width: 24px;
    height: 24px;
    border-radius: 50%;
    border: none;
    outline: none;
    background: #e8eaed;
    color: #3c4043;
    display: flex;
    align-items: center;
    justify-content: center;
    font-weight: 600;
    line-height: 1;
    cursor: pointer;
    z-index: 20;
  }

  :global(.chrome-tabs.chrome-tabs-is-sorting) .add-tab {
    pointer-events: none;
  }
  .add-tab:hover { background: #dadce0; }
  .add-tab:active { background: #c8c9cc; }
  .add-tab span { transform: translateY(-1px); }

  .theme-toggle {
    position: absolute;
    top: 8px;
    right: 8px;
    height: 24px;
    padding: 0 10px;
    border-radius: 12px;
    border: 1px solid #444;
    background: #333;
    color: #fff;
    cursor: pointer;
    z-index: 21;
  }
  .theme-toggle:hover { background: #3a3a3a; }
  :global(body.theme-light) .theme-toggle { background: #fff; color: #111; border-color: #dadce0; }
  :global(body.theme-light) .theme-toggle:hover { background: #f6f6f6; }

  @media (prefers-color-scheme: dark) {
    .add-tab { background: #444; color: #fff; }
    .add-tab:hover { background: #555; }
    .add-tab:active { background: #333; }
  }

  .chrome-tabs-dark-theme .add-tab { background: #444; color: #fff; }
  .chrome-tabs-dark-theme .add-tab:hover { background: #555; }
  .chrome-tabs-dark-theme .add-tab:active { background: #333; }

  .tab-shell { display: flex; flex-direction: column;}
  .tab-panes { position: relative; flex: 1 1 auto; min-height: 0; }
  .tab-pane { flex: 1; align-items: stretch; justify-content: flex-start; padding: 24px; }
  .tab-pane.center { align-items: center; justify-content: center; }
  .placeholder { opacity: 0.8; }
</style>
