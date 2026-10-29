export type RGB = readonly [number, number, number];

export function vividColor(rgb: RGB): RGB {
  const luminance = rgb[0] * .2126 + rgb[1] * .7152 + rgb[2] * .0722;
  return rgb.map(value => Math.max(0, Math.min(255, (luminance + (value - luminance) * 1.9) * 1.5 + 22))) as unknown as RGB;
}

export function gradientPixels(width: number, height: number, colors: readonly RGB[]) {
  const pixels = new Uint8ClampedArray(width * height * 4);
  const horizontal = [0.15, 0.9].map(center => Float32Array.from({ length: width }, (_, x) => Math.exp(-((x / width - center) ** 2) * 2)));
  for (let y = 0; y < height; y++) {
    const top = Math.exp(-((y / height - .1) ** 2) * 2);
    const bottom = Math.exp(-((y / height - .8) ** 2) * 2);
    for (let x = 0; x < width; x++) {
      const a = horizontal[0][x] * top, b = horizontal[1][x] * bottom;
      const offset = (y * width + x) * 4;
      let hash = Math.imul(x + Math.imul(y, 65537), 0x45d9f3b);
      hash = Math.imul(hash ^ (hash >>> 16), 0x45d9f3b);
      // Stochastic rounding before quantization avoids bands without a noise overlay.
      const dither = ((hash ^ (hash >>> 16)) >>> 0) / 4294967296 - .5;
      for (let c = 0; c < 3; c++) pixels[offset + c] = Math.round((12 + colors[0][c] * a + colors[1][c] * b) / (.75 + a + b) + dither);
      pixels[offset + 3] = 255;
    }
  }
  return pixels;
}
