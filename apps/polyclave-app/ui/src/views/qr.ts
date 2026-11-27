// Inline SVG QR code (ISO/IEC 18004) for 10-minute pairing URIs.
// Encoding comes from qrcode-generator, which build.mjs copies into dist/assets/vendor.
import qrcode from '../vendor/qrcode-generator.js';
import { svgEl } from './elements.js';

/** Light modules around the symbol, as the standard requires. */
export const QUIET_ZONE = 4;

/**
 * Encodes `payload` in byte mode at error correction level M. The library picks the
 * smallest version that fits. Each character must be ASCII (the pairing URI always is).
 */
export function renderPairingMatrix(payload: string): SVGSVGElement {
  const code = qrcode(0, 'M');
  code.addData(payload, 'Byte');
  code.make();
  const count = code.getModuleCount();
  const size = count + QUIET_ZONE * 2;
  const svg = svgEl('svg', {
    viewBox: `0 0 ${size} ${size}`,
    class: 'qr-svg',
    role: 'img',
    'aria-label': 'One-time pairing QR code',
    'shape-rendering': 'crispEdges',
  });
  for (let row = 0; row < count; row++) {
    for (let col = 0; col < count; col++) {
      if (code.isDark(row, col)) {
        svg.append(svgEl('rect', { x: col + QUIET_ZONE, y: row + QUIET_ZONE, width: 1, height: 1 }));
      }
    }
  }
  return svg;
}
