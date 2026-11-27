// Types for the ESM build of qrcode-generator (dist/qrcode.mjs), which build.mjs
// copies to dist/assets/vendor/qrcode-generator.js. Only the calls the UI uses.
export interface QRCode {
  addData(data: string, mode?: 'Numeric' | 'Alphanumeric' | 'Byte' | 'Kanji'): void;
  make(): void;
  getModuleCount(): number;
  isDark(row: number, col: number): boolean;
}
declare function qrcode(typeNumber: number, errorCorrectionLevel: 'L' | 'M' | 'Q' | 'H'): QRCode;
export default qrcode;
