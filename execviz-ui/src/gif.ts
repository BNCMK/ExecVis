// ========================================================================
//  MANIFEST
// ========================================================================
const MANIFEST = {
  script_name: 'gif.ts',
  script_path: 'execviz-ui/src/gif.ts',
  module_name: 'gif',
  version: '0.13.0',
  description: 'A GIF89a encoder, in the page.',
  kind: 'module',
  spec: 'docs/execution-visualizer-spec.md',
  internal_dependencies: [],
  external_dependencies: [],
  features: ['gif'],
  api_version: 'execvis-v1.0.0',
  last_updated: '2026-08-07',
} as const;
void MANIFEST;
// ========================================================================

/**
 * A GIF89a encoder, in the page.
 *
 * Written here rather than taken from a library, and rather than done on the
 * server, for one reason: a diagnostic that requires the recipient to install
 * something is a diagnostic nobody looks at. The browser is already drawing this
 * map, so it can hand somebody an animation of it with no dependency anywhere.
 *
 * GIF is the format because it plays inline in an issue tracker, in a chat
 * window and on a forum, with no player and no controls to find.
 */

// ========================================================================
// INTERNALS
// ========================================================================

/** Reduces a frame to a 256-colour palette by quantising each channel. */
function quantise(rgba: Uint8ClampedArray, w: number, h: number): { idx: Uint8Array; palette: number[] } {
  // 3-3-2 quantisation: a fixed palette rather than a per-frame one, so every
  // frame shares it and the file carries a single global colour table. The map
  // is drawn from a small deliberate palette, so a computed one buys little.
  const palette: number[] = [];
  for (let r = 0; r < 8; r++) {
    for (let g = 0; g < 8; g++) {
      for (let b = 0; b < 4; b++) {
        palette.push((r * 255) / 7, (g * 255) / 7, (b * 255) / 3);
      }
    }
  }
  const idx = new Uint8Array(w * h);
  for (let i = 0, p = 0; i < idx.length; i++, p += 4) {
    const r = rgba[p] >> 5, g = rgba[p + 1] >> 5, b = rgba[p + 2] >> 6;
    idx[i] = (r << 5) | (g << 2) | b;
  }
  return { idx, palette: palette.map((v) => Math.round(v)) };
}

/** The LZW variant GIF uses: variable code width, with clear and end codes. */
function lzw(idx: Uint8Array, minCodeSize: number): Uint8Array {
  const clear = 1 << minCodeSize;
  const eoi = clear + 1;
  let codeSize = minCodeSize + 1;
  let next = eoi + 1;
  let dict = new Map<string, number>();

  const out: number[] = [];
  let cur = 0, curBits = 0;
  const emit = (code: number) => {
    cur |= code << curBits;
    curBits += codeSize;
    while (curBits >= 8) { out.push(cur & 0xff); cur >>= 8; curBits -= 8; }
  };

  const reset = () => {
    dict = new Map();
    codeSize = minCodeSize + 1;
    next = eoi + 1;
  };

  emit(clear);
  reset();
  let prefix = String(idx[0]);
  for (let i = 1; i < idx.length; i++) {
    const k = idx[i];
    const combined = prefix + ',' + k;
    if (dict.has(combined)) {
      prefix = combined;
      continue;
    }
    emit(prefix.includes(',') ? dict.get(prefix)! : Number(prefix));
    dict.set(combined, next++);
    if (next > (1 << codeSize)) {
      if (codeSize < 12) codeSize++;
      else { emit(clear); reset(); }
    }
    prefix = String(k);
  }
  emit(prefix.includes(',') ? dict.get(prefix)! : Number(prefix));
  emit(eoi);
  if (curBits > 0) out.push(cur & 0xff);
  return Uint8Array.from(out);
}

function blocks(data: Uint8Array): number[] {
  // GIF carries image data in sub-blocks of at most 255 bytes, each preceded by
  // its length, terminated by a zero-length block.
  const out: number[] = [];
  for (let i = 0; i < data.length; i += 255) {
    const chunk = data.subarray(i, Math.min(i + 255, data.length));
    out.push(chunk.length, ...chunk);
  }
  out.push(0);
  return out;
}

// ========================================================================
// PUBLIC INTERFACE
// ========================================================================

/**
 * Encodes frames as an animated GIF.
 *
 * @param delayCs frame delay in hundredths of a second, which is GIF's own unit
 */
export function encodeGif(frames: ImageData[], delayCs = 12): Blob {
  if (!frames.length) throw new Error('no frames');
  const w = frames[0].width, h = frames[0].height;
  const bytes: number[] = [];
  const push16 = (n: number) => bytes.push(n & 0xff, (n >> 8) & 0xff);

  const first = quantise(frames[0].data, w, h);

  bytes.push(...[0x47, 0x49, 0x46, 0x38, 0x39, 0x61]);   // GIF89a
  push16(w); push16(h);
  bytes.push(0xf7, 0, 0);                                 // global table, 256 entries
  bytes.push(...first.palette);

  // NETSCAPE2.0: loop forever. Without it the animation plays once and a reader
  // who looked away has to reload the page to see it again.
  bytes.push(0x21, 0xff, 0x0b);
  bytes.push(...Array.from('NETSCAPE2.0', (c) => c.charCodeAt(0)));
  bytes.push(3, 1, 0, 0, 0);

  for (const frame of frames) {
    const { idx } = quantise(frame.data, w, h);
    bytes.push(0x21, 0xf9, 0x04, 0x04);                   // graphic control
    push16(delayCs);
    bytes.push(0, 0);
    bytes.push(0x2c);                                     // image descriptor
    push16(0); push16(0); push16(w); push16(h);
    bytes.push(0);
    const minCodeSize = 8;
    bytes.push(minCodeSize);
    bytes.push(...blocks(lzw(idx, minCodeSize)));
  }
  bytes.push(0x3b);                                       // trailer
  return new Blob([Uint8Array.from(bytes)], { type: 'image/gif' });
}

/**
 * Records the replay across the selected window and hands back a GIF.
 *
 * Frames are sampled across the window rather than captured in real time: a
 * replay of four minutes is not worth four minutes of anybody's attention, and a
 * diagnostic animation that runs at wall speed gets scrubbed past.
 */
export async function recordReplay(
  canvas: HTMLCanvasElement,
  setClock: (t: number) => void,
  from: number,
  to: number,
  frameCount = 40,
  scale = 0.5,
): Promise<Blob> {
  const w = Math.max(2, Math.round(canvas.width * scale));
  const h = Math.max(2, Math.round(canvas.height * scale));
  const shrink = document.createElement('canvas');
  shrink.width = w; shrink.height = h;
  const sctx = shrink.getContext('2d')!;

  const frames: ImageData[] = [];
  for (let i = 0; i < frameCount; i++) {
    const t = from + ((to - from) * i) / Math.max(1, frameCount - 1);
    setClock(t);
    // one animation frame, so the map has drawn this instant before it
    // is read back: capturing before the draw records the previous instant and
    // the animation ends up one frame behind its own clock
    await new Promise<void>((r) => requestAnimationFrame(() => r()));
    sctx.drawImage(canvas, 0, 0, w, h);
    frames.push(sctx.getImageData(0, 0, w, h));
  }
  return encodeGif(frames);
}
