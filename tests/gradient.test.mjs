import { test } from 'node:test';
import assert from 'node:assert/strict';
import { gradientPixels } from '../src/lib/gradient.ts';

test('dark gradients use deterministic monochrome dithering instead of long color bands', () => {
  const colors = [[35, 35, 35], [50, 50, 50]];
  const pixels = gradientPixels(2048, 1, colors);
  assert.deepEqual(pixels, gradientPixels(2048, 1, colors));
  let transitions = 0;
  for (let i = 0; i < pixels.length; i += 4) {
    assert.equal(pixels[i], pixels[i + 1]);
    assert.equal(pixels[i], pixels[i + 2]);
    assert.equal(pixels[i + 3], 255);
    if (i && pixels[i] !== pixels[i - 4]) transitions++;
    if (i) assert.ok(Math.abs(pixels[i] - pixels[i - 4]) <= 2);
  }
  assert.ok(transitions > 200, `${transitions} transitions left broad flat bands`);
});

test('gradient work runs in a worker while the calling thread keeps processing events', async () => {
  const { Worker } = await import('node:worker_threads');
  const entry = new URL('../src/lib/gradient.worker.ts', import.meta.url).href;
  const source = `import { parentPort } from 'node:worker_threads';
    globalThis.self = { postMessage: (...args) => parentPort.postMessage(...args) };
    await import(${JSON.stringify(entry)});
    parentPort.on('message', data => self.onmessage({ data }));`;
  const worker = new Worker(new URL(`data:text/javascript,${encodeURIComponent(source)}`));
  let ticks = 0;
  const heartbeat = setInterval(() => ticks++, 5);
  try {
    const result = new Promise((resolve, reject) => { worker.once('message', resolve); worker.once('error', reject); });
    worker.postMessage({ id: 7, width: 1920, height: 1080, colors: [[140, 85, 25], [50, 100, 140]] });
    const frame = await result;
    assert.equal(frame.id, 7);
    assert.equal(frame.pixels.length, 1920 * 1080 * 4);
    assert.ok(ticks > 0, 'rendering blocked the calling event loop');
  } finally { clearInterval(heartbeat); await worker.terminate(); }
});

test('hover colors become brighter and more saturated while neutrals stay neutral', async () => {
  const { vividColor } = await import('../src/lib/gradient.ts');
  const original = [110, 70, 40];
  const vivid = vividColor(original);
  assert.ok(Math.max(...vivid) > Math.max(...original));
  assert.ok(Math.max(...vivid) - Math.min(...vivid) > Math.max(...original) - Math.min(...original));
  const gray = vividColor([70, 70, 70]);
  assert.equal(gray[0], gray[1]);
  assert.equal(gray[1], gray[2]);
});
