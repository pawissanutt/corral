// The image formats corral sends a model, recognised by their first bytes rather than a name or a declared type.

export type ImageMime = "image/png" | "image/jpeg" | "image/gif" | "image/webp";

const PNG = [137, 80, 78, 71, 13, 10, 26, 10];
const ascii = (bytes: Uint8Array, from: number, text: string) => [...text].every((char, at) => bytes[from + at] === char.charCodeAt(0));

/** The image type `bytes` start with, or undefined when they are not a PNG, JPEG, GIF or WebP image. */
export function sniffImageType(bytes: Uint8Array): ImageMime | undefined {
  if (bytes.length >= 8 && PNG.every((byte, at) => bytes[at] === byte)) return "image/png";
  if (bytes.length >= 3 && bytes[0] === 255 && bytes[1] === 216 && bytes[2] === 255) return "image/jpeg";
  if (bytes.length >= 6 && (ascii(bytes, 0, "GIF87a") || ascii(bytes, 0, "GIF89a"))) return "image/gif";
  if (bytes.length >= 12 && ascii(bytes, 0, "RIFF") && ascii(bytes, 8, "WEBP")) return "image/webp";
  return undefined;
}
