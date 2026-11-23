import "@testing-library/jest-dom/vitest";
import { webcrypto } from "node:crypto";

if (!window.crypto?.getRandomValues) {
  Object.defineProperty(window, "crypto", {
    value: webcrypto,
    configurable: true,
  });
}
