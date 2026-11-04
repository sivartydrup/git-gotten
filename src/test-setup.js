import { afterAll } from "vitest";

// Node's Web Storage globals can shadow jsdom's storage (and may be undefined).
// Use the actual jsdom stores so tests exercise browser storage semantics.
const originals = new Map();
for (const key of ["localStorage", "sessionStorage"]) {
  originals.set(key, Object.getOwnPropertyDescriptor(globalThis, key));
  Object.defineProperty(globalThis, key, {
    configurable: true,
    enumerable: true,
    writable: true,
    value: jsdom.window[key],
  });
}

afterAll(() => {
  for (const [key, descriptor] of originals) {
    if (descriptor) Object.defineProperty(globalThis, key, descriptor);
    else delete globalThis[key];
  }
});
