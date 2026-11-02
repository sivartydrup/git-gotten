// Minimal typing for the one Node builtin a test reaches for (no @types/node in this app).
declare module "node:fs" {
  export function readFileSync(path: string | URL, encoding?: string): string;
}
