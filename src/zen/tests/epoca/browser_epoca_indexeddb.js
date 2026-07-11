/* Any copyright is dedicated to the Public Domain.
   https://creativecommons.org/publicdomain/zero/1.0/ */
"use strict";
const { EpocaDotAppRegistry } = ChromeUtils.importESModule(
  "resource:///modules/EpocaDotAppRegistry.sys.mjs"
);
add_setup(function () {
  EpocaDotAppRegistry.register("idbprobe", {
    "/index.html": new TextEncoder().encode(
      "<!DOCTYPE html><html><head><title>idbprobe</title></head><body></body></html>"
    ),
  });
  registerCleanupFunction(() => EpocaDotAppRegistry.unregister("idbprobe"));
});
add_task(async function probe() {
  await BrowserTestUtils.withNewTab("dot://idbprobe.dot/", async browser => {
    const r = await SpecialPowers.spawn(browser, [], async () => {
      const out = { factory: typeof content.indexedDB };
      const res = await new Promise(resolve => {
        let req;
        try {
          req = content.indexedDB.open("epoca-probe", 1);
        } catch (e) {
          resolve({ phase: "open-throw", name: e.name, message: e.message });
          return;
        }
        req.onupgradeneeded = () => {
          try {
            req.result.createObjectStore("s", { keyPath: "id" });
          } catch (e) {
            resolve({ phase: "upgrade-throw", name: e.name, message: e.message });
          }
        };
        req.onsuccess = () => resolve({ phase: "success" });
        req.onerror = () =>
          resolve({ phase: "error", name: req.error?.name, message: req.error?.message });
        req.onblocked = () => resolve({ phase: "blocked" });
      });
      Object.assign(out, res);
      return out;
    });
    info("IDBPROBE " + JSON.stringify(r));
    is(r.phase, "success", "IndexedDB opens on a dot:// product origin");
  });
});
