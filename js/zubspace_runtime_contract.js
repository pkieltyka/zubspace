(function (root) {
  "use strict";
  function verifyImports(module, imports) {
    const missing = WebAssembly.Module.imports(module).filter((entry) => {
      const value = imports[entry.module]?.[entry.name];
      return value === undefined || (entry.kind === "function" && typeof value !== "function");
    });
    if (missing.length) {
      throw new Error(`Incompatible Zubspace runtime: missing WASM imports: ${missing.map((entry) => `${entry.module}.${entry.name}`).join(", ")}`);
    }
  }
  const api = { verifyImports };
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.ZubspaceRuntimeContract = api;
})(globalThis);
