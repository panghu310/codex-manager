import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import zlib from "node:zlib";

const iconDir = "src-tauri/icons";
mkdirSync(iconDir, { recursive: true });

function crc32(buffer) {
  let crc = 0xffffffff;
  for (const byte of buffer) {
    crc ^= byte;
    for (let i = 0; i < 8; i++) {
      crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const name = Buffer.from(type);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([name, data])));
  return Buffer.concat([len, name, data, crc]);
}

function writePng(path, width, height, paint) {
  const rows = [];
  for (let y = 0; y < height; y++) {
    const row = Buffer.alloc(1 + width * 4);
    row[0] = 0;
    for (let x = 0; x < width; x++) {
      const i = 1 + x * 4;
      const [r, g, b, a] = paint(x, y, width, height);
      row[i] = r;
      row[i + 1] = g;
      row[i + 2] = b;
      row[i + 3] = a;
    }
    rows.push(row);
  }

  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;

  const png = Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    chunk("IHDR", ihdr),
    chunk("IDAT", zlib.deflateSync(Buffer.concat(rows))),
    chunk("IEND", Buffer.alloc(0))
  ]);
  writeFileSync(path, png);
}

function blend(top, bottom) {
  const alpha = top[3] / 255;
  return [
    Math.round(top[0] * alpha + bottom[0] * (1 - alpha)),
    Math.round(top[1] * alpha + bottom[1] * (1 - alpha)),
    Math.round(top[2] * alpha + bottom[2] * (1 - alpha)),
    255
  ];
}

function roundedRectAlpha(x, y, w, h, radius) {
  const dx = Math.max(radius - x, 0, x - (w - radius));
  const dy = Math.max(radius - y, 0, y - (h - radius));
  if (dx === 0 || dy === 0) return 255;
  return dx * dx + dy * dy <= radius * radius ? 255 : 0;
}

const glyph = [
  "1...1..111.",
  ".1.1...1..1",
  "..1....1...",
  ".1.1...1..1",
  "1...1..111."
];

function iconPaint(x, y, w, h) {
  const pad = Math.round(w * 0.105);
  const ix = x - pad;
  const iy = y - pad;
  const iw = w - pad * 2;
  const ih = h - pad * 2;
  if (ix < 0 || iy < 0 || ix >= iw || iy >= ih) return [0, 0, 0, 0];

  const radius = Math.round(iw * 0.22);
  const alpha = roundedRectAlpha(ix, iy, iw - 1, ih - 1, radius);
  if (alpha === 0) return [0, 0, 0, 0];

  const nx = ix / iw;
  const ny = iy / ih;
  const base = [
    Math.round(8 + 12 * ny),
    Math.round(9 + 10 * nx),
    Math.round(12 + 18 * ny),
    255
  ];

  const border = Math.min(ix, iy, iw - 1 - ix, ih - 1 - iy);
  if (border < Math.max(2, iw * 0.018)) {
    return [54, 56, 58, 255];
  }

  const grid = Math.max(8, Math.round(iw / 22));
  const dotRadius = Math.max(1.1, iw / 170);
  const dx = (ix % grid) - grid / 2;
  const dy = (iy % grid) - grid / 2;
  let color = base;
  if (dx * dx + dy * dy <= dotRadius * dotRadius) {
    color = blend([38, 255, 181, 70], color);
  }

  const glyphCell = iw / 13.5;
  const glyphLeft = (iw - glyph[0].length * glyphCell) / 2;
  const glyphTop = ih * 0.31;
  const gx = Math.floor((ix - glyphLeft) / glyphCell);
  const gy = Math.floor((iy - glyphTop) / glyphCell);
  if (gy >= 0 && gy < glyph.length && gx >= 0 && gx < glyph[gy].length && glyph[gy][gx] === "1") {
    const cx = glyphLeft + gx * glyphCell + glyphCell / 2;
    const cy = glyphTop + gy * glyphCell + glyphCell / 2;
    const r = glyphCell * 0.28;
    if ((ix - cx) ** 2 + (iy - cy) ** 2 <= r * r) {
      return [242, 242, 232, 255];
    }
  }

  const statusX = iw * 0.5;
  const statusY = ih * 0.73;
  const statusR = iw * 0.045;
  if ((ix - statusX) ** 2 + (iy - statusY) ** 2 <= statusR ** 2) {
    return [37, 255, 181, 255];
  }

  return color;
}

const sizes = [16, 32, 64, 128, 256, 512, 1024];
for (const size of sizes) {
  writePng(join(iconDir, `${size}x${size}.png`), size, size, iconPaint);
}

writePng(join(iconDir, "icon.png"), 1024, 1024, iconPaint);
writePng(join(iconDir, "tray.png"), 32, 32, iconPaint);
