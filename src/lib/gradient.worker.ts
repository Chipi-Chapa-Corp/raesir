import { gradientPixels, type RGB } from './gradient.ts';

export type GradientRequest = { id: number; width: number; height: number; colors: RGB[] };
export type GradientFrame = { id: number; width: number; height: number; pixels: Uint8ClampedArray<ArrayBuffer> };
const scope = self as unknown as Pick<Worker, 'onmessage' | 'postMessage'>;
scope.onmessage = ({ data }: MessageEvent<GradientRequest>) => {
  const { id, width, height, colors } = data;
  const pixels = gradientPixels(width, height, colors);
  scope.postMessage({ id, width, height, pixels } satisfies GradientFrame, [pixels.buffer]);
};
