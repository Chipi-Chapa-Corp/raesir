<script lang="ts">
  import { onMount } from 'svelte';
  import GradientWorker from './gradient.worker?worker&inline';
  import type { GradientFrame, GradientRequest } from './gradient.worker';
  import { vividColor, type RGB } from './gradient';

  let { image }: { image: string } = $props();
  let front: HTMLCanvasElement;
  let back: HTMLCanvasElement;
  let active = $state(0);
  let smooth = $state(true);
  let colors: RGB[] = [[32, 42, 46], [20, 28, 28]];
  let worker: Worker | undefined;
  let busy = false;
  let desired = 0;
  let pending: GradientRequest | undefined;
  let ready: GradientFrame | undefined;
  let requested = '';
  let lastSwap = 0;
  let debounce: ReturnType<typeof setTimeout>;
  let swap: ReturnType<typeof setTimeout>;
  let animationFrame = 0;
  const cache = new Map<string, RGB[]>();

  function sendLatest() {
    if (!worker || busy || !pending) return;
    busy = true;
    worker.postMessage(pending);
    pending = undefined;
  }
  function render() {
    if (!worker || !innerWidth || !innerHeight) return;
    const ratio = Math.min(1, Math.sqrt(921_600 / (innerWidth * innerHeight)));
    const width = Math.round(innerWidth * ratio), height = Math.round(innerHeight * ratio);
    const key = `${width}:${height}:${colors.flat().join(',')}`;
    if (requested === key) return;
    requested = key;
    pending = { id: ++desired, width, height, colors };
    sendLatest();
  }
  function schedule() { clearTimeout(debounce); debounce = setTimeout(render, 100); }
  function watchFrames() {
    if (!smooth || animationFrame || document.hidden) return;
    const started = performance.now();
    let previous = started, slowFrames = 0;
    function frame(now: number) {
      animationFrame = 0;
      if (document.hidden) return;
      const elapsed = now - previous;
      if (elapsed > 50) slowFrames++;
      if (elapsed > 150 || slowFrames >= 3) { smooth = false; return; }
      previous = now;
      if (now - started < 450) animationFrame = requestAnimationFrame(frame);
    }
    animationFrame = requestAnimationFrame(frame);
  }
  function present() {
    if (!ready || ready.id !== desired) return;
    const target = active === 0 ? back : front;
    if (target.width !== ready.width) target.width = ready.width;
    if (target.height !== ready.height) target.height = ready.height;
    target.getContext('2d', { alpha: false })?.putImageData(new ImageData(ready.pixels, ready.width, ready.height), 0, 0);
    ready = undefined;
    active = active === 0 ? 1 : 0;
    lastSwap = performance.now();
    watchFrames();
  }
  $effect(() => {
    desired++;
    pending = undefined;
    ready = undefined;
    requested = '';
    clearTimeout(debounce);
    clearTimeout(swap);
    if (!image) return;
    if (cache.has(image)) { colors = cache.get(image)!; schedule(); return; }
    let cancelled = false;
    const artwork = new Image();
    artwork.crossOrigin = 'anonymous';
    artwork.onload = () => {
      if (cancelled) return;
      try {
        const canvas = document.createElement('canvas');
        canvas.width = 16; canvas.height = 16;
        const context = canvas.getContext('2d');
        if (!context) return;
        context.drawImage(artwork, 0, 0, 16, 16);
        const pixels = context.getImageData(0, 0, 16, 16).data;
        colors = [0, 1].map(side => {
          const sum = [0, 0, 0]; let total = 0;
          for (let y = 0; y < 16; y++) for (let x = side * 8; x < side * 8 + 8; x++) {
            const offset = (y * 16 + x) * 4;
            const rgb = Array.from(pixels.slice(offset, offset + 3));
            const weight = 1 + (Math.max(...rgb) - Math.min(...rgb)) / 64;
            rgb.forEach((value, channel) => sum[channel] += value * weight);
            total += weight;
          }
          return vividColor([sum[0] / total, sum[1] / total, sum[2] / total]);
        });
        if (cache.size >= 64) cache.delete(cache.keys().next().value!);
        cache.set(image, colors);
        schedule();
      } catch { /* Artwork is optional. */ }
    };
    artwork.src = image;
    return () => { cancelled = true; artwork.onload = null; };
  });
  onMount(() => {
    try { worker = new GradientWorker(); } catch { return; }
    worker.onmessage = ({ data }: MessageEvent<GradientFrame>) => {
      busy = false;
      if (data.id === desired) {
        ready = data;
        clearTimeout(swap);
        const wait = !smooth || matchMedia('(prefers-reduced-motion: reduce)').matches ? 0 : Math.max(0, lastSwap + 450 - performance.now());
        swap = setTimeout(present, wait);
      }
      sendLatest();
    };
    worker.onerror = () => { worker?.terminate(); worker = undefined; };
    schedule();
    window.addEventListener('resize', schedule);
    return () => { clearTimeout(debounce); clearTimeout(swap); cancelAnimationFrame(animationFrame); window.removeEventListener('resize', schedule); worker?.terminate(); };
  });
</script>

<div class="backdrop" class:still={!smooth} aria-hidden="true">
  <canvas bind:this={front} class:visible={active === 0}></canvas>
  <canvas bind:this={back} class:visible={active === 1}></canvas>
</div>

<style>
  .backdrop { position: fixed; inset: 0; z-index: -1; background: #101313; pointer-events: none; }
  canvas { position: absolute; inset: 0; width: 100%; height: 100%; }
  canvas.visible { z-index: 1; animation: reveal .4s ease; }
  .still canvas { animation: none; }
  @keyframes reveal { from { opacity: 0; } to { opacity: 1; } }
  @media(prefers-reduced-motion:reduce) { canvas { animation: none !important; } }
</style>
