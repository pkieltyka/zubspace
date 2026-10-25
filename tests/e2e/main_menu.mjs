#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";

import {
  freePort,
  repoRoot,
  sleep,
  startRealServer,
  startStaticServer,
  stopProcess,
  waitForServerListening,
} from "../../tools/browser_firefox_rtc_smoke.mjs";
import {
  browserE2eFailureFooter,
  openChromeClient,
  startChrome,
  waitForChrome,
  waitForPlayersReady, rendezvousPlayers,
  waitUntil,
} from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";

const arena = process.env.ZUBSPACE_E2E_ARENA || "chaos-zone";
const wasmPath = "target/wasm32-unknown-unknown/debug/zubspace.wasm";
const e2eScript = "tests/e2e/main_menu.mjs";

async function main() {
  if (!fs.existsSync(path.join(repoRoot, wasmPath))) {
    throw new Error(`missing ${wasmPath}; run make build-client first`);
  }
  const artifactDir = process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR || `/tmp/zubspace-main-menu-${Date.now()}`;
  const evidence = createFocusedBrowserEvidence({ e2eScript, artifactDir });
  const menuScreenshots = {};
  const abilityVisibilityCases = [];

  const serverPort = await freePort();
  const alphaPort = await freePort();
  const betaPort = await freePort();
  const staticServer = await startStaticServer();
  const server = startRealServer(serverPort);
  const alphaChrome = startChrome(alphaPort, "main-menu-alpha");
  const betaChrome = startChrome(betaPort, "main-menu-beta");
  let alpha = null;
  let beta = null;
  let alphaUrl = null;
  let betaUrl = null;
  const capture = () => ({
    server, serverPort, staticServer, urls: [alphaUrl, betaUrl],
    clients: [{ label: "alpha", client: alpha }, { label: "beta", client: beta }],
    extraLogs: { "alpha-chrome": alphaChrome.logs, "beta-chrome": betaChrome.logs },
    extraScreenshots: menuScreenshots,
    // The delivery audit asserts an ingress drain, which is disconnect-cleanup evidence,
    // so it closes both sessions itself after the live-page artifact capture. Each client
    // is guarded so one already-dead session cannot make cleanup throw.
    disconnect: async () => {
      await Promise.all([
        alpha?.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}),
        beta?.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}),
      ]);
    },
  });

  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    await Promise.all([waitForChrome(alphaChrome), waitForChrome(betaChrome)]);
    const rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    const pageBase =
      `http://127.0.0.1:${staticServer.port}/index.html` +
      `?rtc=${encodeURIComponent(rtcUrl)}` +
      `&password=secret&arena=${encodeURIComponent(arena)}&packetlog=1&trace=1`;
    alphaUrl = `${pageBase}&name=menualpha`;
    betaUrl = `${pageBase}&name=menubeta&debug=31337`;
    // The observer gives the live player count a real joined client to report.
    beta = await openChromeClient(betaChrome, betaUrl);
    await beta.enterShip();
    // This witness participates before the lengthy menu/viewport tour. A
    // completely untouched new spawn correctly idles to spectator at120s.
    await beta.press("ArrowRight", 70);
    await waitUntil("debug mode retains startup diagnostics", async () =>
      (await beta.pageState()).uiMetrics.notificationMessages?.some(text => /Downloaded LVZ archive|Login accepted|Joined arena|Browser network connected|Arena directory/i.test(text)), 5000);
    alphaUrl = `${pageBase.replace("&trace=1", "")}&menu=1`;
    alpha = await openChromeClient(alphaChrome, alphaUrl, { beforeReady: async ({ send, evaluate }) => {
      await waitUntil("public menu and live population", async () => evaluate(`
        document.querySelector("#player-name-form")?.classList.contains("visible") &&
        document.querySelector("#arena-population")?.textContent.includes("1 player online")
      `), 20_000);
      // Desktop prefetch is allowed on the splash. Runtime/signaling must
      // remain unstarted until a real submission, including modal Enter.
      const initial = await evaluate(`({
        nameLabel: document.querySelector('label[for="player-name-input"]').textContent,
        nameFocused: document.activeElement === document.querySelector("#player-name-input"),
        arena: window.__zubspaceArena,
        selector: !!document.querySelector("#arena-select"),
        play: document.querySelector("#player-name-submit").textContent,
        disabled: document.querySelector("#player-name-submit").disabled,
        ships: [...document.querySelectorAll(".ship-option")].map(button => ({ name: button.getAttribute("aria-label"), selected: button.getAttribute("aria-checked") })),
        logoWhitespace: getComputedStyle(document.querySelector(".ascii-logo")).whiteSpace,
        runtimeStarted: !!document.querySelector('script[src*="mq_js_bundle.js"], script[src*="zubspace_rtc_config.js"]') || typeof wasm_exports !== "undefined"
      })`);
      if (initial.nameLabel !== "Player name" || !initial.nameFocused || initial.arena !== "chaos-zone" || initial.selector || initial.play !== "Play" || initial.logoWhitespace !== "pre" || !initial.disabled || initial.runtimeStarted) {
        throw new Error(`invalid initial menu: ${JSON.stringify(initial)}`);
      }
      if (initial.ships.length !== 8 || initial.ships[0].name !== "1. Warbird" || initial.ships[0].selected !== "true" || initial.ships.slice(1).some(ship => ship.selected !== "false")) throw new Error(`invalid starting ships: ${JSON.stringify(initial.ships)}`);
      const save = async name => {
        const shot = await send("Page.captureScreenshot", { format: "png", fromSurface: true });
        menuScreenshots[name.replace(/\.png$/, "")] = shot.data;
      };
      const guide = async size => {
        // Every visible hint fragment is part of the same native button.
        // Use physical pointer/touch input rather than DOM click handlers.
        const points = await evaluate(`[
          document.querySelector(".controls-launch-icon"),
          ...document.querySelectorAll(".menu-controls-text > span")
        ].map(element => { const r = element.getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 }; }).concat((() => {
          const r = document.querySelector("#controls-open").getBoundingClientRect();
          return { x: r.x + 3, y: r.y + r.height / 2 };
        })())`);
        for (const [index, point] of points.entries()) {
          await send("Input.dispatchMouseEvent", { type: "mouseMoved", ...point });
          if (size !== "iphone") {
            await waitUntil("whole hint hover emphasizes the keyboard icon", () => evaluate(`
              getComputedStyle(document.querySelector("#controls-open")).backgroundColor !== "rgba(0, 0, 0, 0)" &&
              getComputedStyle(document.querySelector(".controls-launch-icon")).color === "rgb(255, 255, 255)"
            `), 2000);
          }
          if (index === 1) await save(`menu-controls-hint-hover-${size}.png`);
          if (size === "iphone") {
            await send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [{ ...point, radiusX: 1, radiusY: 1 }] });
            await send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
          } else {
            await send("Input.dispatchMouseEvent", { type: "mousePressed", button: "left", clickCount: 1, ...point });
            await send("Input.dispatchMouseEvent", { type: "mouseReleased", button: "left", clickCount: 1, ...point });
          }
          await waitUntil("hint region opens the controls modal", () => evaluate('document.querySelector("#controls-dialog").open'), 2000);
          await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
          await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
          await waitUntil("hint modal closes one layer", () => evaluate('!document.querySelector("#controls-dialog").open'), 2000);
        }
        await evaluate(`document.querySelector("#controls-open").focus(); true`);
        await send("Input.dispatchKeyEvent", { type: "keyDown", key: " ", code: "Space", windowsVirtualKeyCode: 32, text: " " });
        await send("Input.dispatchKeyEvent", { type: "keyUp", key: " ", code: "Space", windowsVirtualKeyCode: 32 });
        const state = await evaluate(`(() => {
          const modal = document.querySelector("#controls-dialog");
          const rect = modal.getBoundingClientRect();
          const keys = [...modal.querySelectorAll("kbd")].map(el => el.textContent);
          return { open: modal.open, fits: rect.left >= 0 && rect.right <= innerWidth && rect.top >= 0 && rect.bottom <= innerHeight,
            keys, name: document.querySelector("#player-name-input").value,
            footer: document.querySelector(".menu-controls-text").textContent,
            noZone: !document.querySelector("#menu-arena-name"),
            build: document.querySelector("#menu-build").textContent };
        })()`);
        if (!/^v[0-9]+\.[0-9]+\.[0-9]+.* · [a-f0-9]{8}$/.test(state.build) || !state.open || !state.fits || !state.noZone || state.name || state.footer !== "ARROWS to fly · X or CTRL for bullets · Z or TAB for bombs" ||
            !["ESC", "ENTER", "CTRL", "TAB", "↑", "←", "↓", "→", ..."1234567890"].every(key => state.keys.includes(key)) ||
            state.keys.some(key => /WASD|^F[0-9]+$|HOME|END/.test(key))) throw new Error(`invalid keyboard guide: ${JSON.stringify(state)}`);
        await save(`menu-controls-${size}.png`);
        // Tab remains in the native dialog, with no game bridge stealing focus.
        await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Tab", code: "Tab", windowsVirtualKeyCode: 9 });
        await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Tab", code: "Tab", windowsVirtualKeyCode: 9 });
        if (!await evaluate(`document.querySelector("#controls-dialog").contains(document.activeElement)`)) throw new Error("keyboard guide lost focus");
        // Both layouts are lit by default. Two more Tabs reach Layout 2, and Enter dims Layout 1's keys.
        const litKeys = () => evaluate(`[...document.querySelectorAll("#controls-keyboard .flight-key")]
          .filter(key => getComputedStyle(key).backgroundColor !== "rgb(13, 13, 13)").map(key => key.querySelector("kbd").textContent)`);
        const both = await litKeys();
        for (let i = 0; i < 2; i++) {
          await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Tab", code: "Tab", windowsVirtualKeyCode: 9 });
          await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Tab", code: "Tab", windowsVirtualKeyCode: 9 });
        }
        await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13, text: "\r" });
        await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });
        const layout2 = { lit: await litKeys(), shown: await evaluate(`document.querySelector("#controls-dialog").dataset.layout`),
          pressed: await evaluate(`document.activeElement.dataset.layoutOption === "2" && document.activeElement.getAttribute("aria-pressed") === "true"`) };
        const wasdKeys = ["W", "A", "S", "D", ",", ".", "/"], arrowKeys = ["CTRL", "TAB", "Q", "Z", "X", "↑", "←", "↓", "→"];
        if (![...wasdKeys, ...arrowKeys, "SHIFT"].every(key => both.includes(key)) || layout2.shown !== "2" || !layout2.pressed ||
            ![...wasdKeys, "SHIFT"].every(key => layout2.lit.includes(key)) || arrowKeys.some(key => layout2.lit.includes(key)))
          throw new Error(`invalid keyboard layouts: ${JSON.stringify({ both, layout2 })}`);
        await save(`menu-controls-wasd-${size}.png`);
        await evaluate(`document.querySelector('[data-layout-option="all"]').click(); true`);
        await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
        await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
        await waitUntil("Escape closes keyboard guide", async () => evaluate(`!document.querySelector("#controls-dialog").open`), 2000);
        if (!await evaluate(`document.activeElement.id === "controls-open" && !document.querySelector("#player-name-input").value`)) throw new Error("guide did not restore menu focus");
        await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Tab", code: "Tab", windowsVirtualKeyCode: 9 });
        await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Tab", code: "Tab", windowsVirtualKeyCode: 9 });
        if (!await evaluate(`document.activeElement.id === "credits-open"`)) throw new Error("Credits button is not reachable by keyboard");
        // Enter activates the footer button without submitting the join form.
        await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13, text: "\r" });
        await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });
        const credits = await evaluate(`(() => {
          const modal = document.querySelector("#credits-dialog"), rect = modal.getBoundingClientRect();
          return { open: modal.open, fits: rect.left >= 0 && rect.right <= innerWidth && rect.top >= 0 && rect.bottom <= innerHeight,
            text: modal.textContent, source: modal.querySelector(".credits-source").href, focused: modal.contains(document.activeElement),
            noRuntime: !document.querySelector('script[src*="mq_js_bundle.js"], script[src*="zubspace_rtc_config.js"]') && typeof wasm_exports === "undefined" };
        })()`);
        if (!credits.open || !credits.fits || !credits.focused || !credits.noRuntime ||
            !["Subspace", "Subspace Continuum", "Pull requests", "mods"].every(text => credits.text.includes(text)) ||
            credits.source !== "https://github.com/pkieltyka/zubspace") throw new Error(`invalid credits: ${JSON.stringify(credits)}`);
        await save(`menu-credits-${size}.png`);
        await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Tab", code: "Tab", windowsVirtualKeyCode: 9 });
        await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Tab", code: "Tab", windowsVirtualKeyCode: 9 });
        if (!await evaluate(`document.querySelector("#credits-dialog").contains(document.activeElement)`)) throw new Error("credits lost focus");
        await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
        await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
        await waitUntil("Escape closes credits and restores focus", async () => evaluate(`!document.querySelector("#credits-dialog").open && document.activeElement.id === "credits-open"`), 2000);
        await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Tab", code: "Tab", windowsVirtualKeyCode: 9 });
        await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Tab", code: "Tab", windowsVirtualKeyCode: 9 });
        if (!await evaluate(`document.activeElement === document.querySelector(".menu-meta a")`)) throw new Error("GitHub link is not reachable by keyboard");
      };
      await guide("desktop");
      await save("menu-desktop.png");
      await send("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 1, mobile: false });
      await sleep(120);
      const fits = await evaluate(`(() => {
        const rect = document.querySelector("#player-name-submit").getBoundingClientRect();
        const input = document.querySelector("#player-name-input").getBoundingClientRect();
        return rect.left >= 0 && rect.right <= innerWidth && rect.bottom <= innerHeight && input.right <= innerWidth;
      })()`);
      if (!fits) throw new Error("mobile Play button does not fit");
      await guide("mobile");
      await save("menu-mobile.png");
      const desktopAgent = await evaluate(`({ userAgent: navigator.userAgent, platform: navigator.platform })`);
      // Real device identities, including the desktop-style iPad UA. Resizing a
      // desktop window alone must retain Play (covered by the narrow view above).
      const mobileProfiles = [
        { name: "iphone", userAgent: "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 Version/18.0 Mobile/15E148 Safari/604.1", platform: "iPhone", sizes: [[390, 844], [844, 390], [375, 667], [667, 375]] },
        { name: "ipad", userAgent: "Mozilla/5.0 (iPad; CPU OS 18_0 like Mac OS X) AppleWebKit/605.1.15 Version/18.0 Mobile/15E148 Safari/604.1", platform: "iPad", sizes: [[820, 1180], [1180, 820]] },
        { name: "ipad-desktop", userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15) AppleWebKit/605.1.15 Version/18.0 Safari/605.1.15", platform: "MacIntel", sizes: [[768, 1024], [1024, 768]] },
        { name: "android", userAgent: "Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 Chrome/140.0.0.0 Mobile Safari/537.36", platform: "Linux armv8l", sizes: [[393, 851], [851, 393]] },
      ];
      const storedIdentity = await evaluate(`localStorage.getItem("zubspace.identity.v1")`);
      await evaluate(`localStorage.setItem("zubspace.identity.v1", JSON.stringify({ name: "mobilepilot", playerId: "123456789012" })); true`);
      for (const profile of mobileProfiles) {
        await send("Emulation.setUserAgentOverride", { userAgent: profile.userAgent, platform: profile.platform });
        await send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 });
        const [width, height] = profile.sizes[0];
        await send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: true });
        await send("Page.navigate", { url: `${pageBase.replace("&trace=1", "")}&name=mobilepilot` });
        await waitUntil(`${profile.name} desktop-only splash and live population`, async () => evaluate(`
          window.zubspaceMenu?.mobile === true && !document.querySelector("#mobile-join-message").hidden &&
          document.querySelector("#arena-population").textContent.includes("1 player online")
        `), 15_000);
        for (const [width, height] of profile.sizes) {
          await send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: true });
          await sleep(180);
          await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });
          await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });
          const state = await evaluate(`(() => {
            const rect = selector => { const r = document.querySelector(selector).getBoundingClientRect(); return { left: r.left, right: r.right, top: r.top, bottom: r.bottom }; };
            const selectors = [".ascii-logo", "#mobile-join-message", ".menu-bottom", ".menu-meta", "#controls-open"];
            const rects = selectors.map(rect);
            const github = document.querySelector(".menu-meta a");
            return { rects, fits: rects.every(r => r.left >= 0 && r.right <= innerWidth + 1 && r.top >= 0 && r.bottom <= innerHeight + 1),
              separated: rects[1].bottom < rects[2].top,
              blocked: !document.querySelector("#player-name-form").classList.contains("visible") && document.querySelector("#player-name-submit").disabled,
              noRuntime: !performance.getEntriesByType("resource").some(r => /zubspace[.]wasm|zubspace_rtc_config|mq_js_bundle/.test(r.name)),
              stars: window.zubspaceMenu.active && document.querySelector("#menu-stars").width > 0,
              github: github.href === "https://github.com/pkieltyka/zubspace" && github.target === "_blank" && github.rel.includes("noopener") };
          })()`);
          if (!state.fits || !state.separated || !state.blocked || !state.noRuntime || !state.stars || !state.github) throw new Error(`invalid ${profile.name} ${width}x${height} splash: ${JSON.stringify(state)}`);
          await save(`menu-${profile.name}-${width}x${height}.png`);
        }
        if (profile.name === "iphone") await guide("iphone");
      }
      await send("Emulation.setUserAgentOverride", desktopAgent);
      await send("Emulation.setTouchEmulationEnabled", { enabled: false });
      await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 720, deviceScaleFactor: 1, mobile: false });
      await evaluate(`localStorage.removeItem("zubspace.identity.v1"); true`);
      if (storedIdentity !== null) await evaluate(`localStorage.setItem("zubspace.identity.v1", ${JSON.stringify(storedIdentity)}); true`);
      await send("Page.navigate", { url: `${pageBase}&menu=1` });
      await waitUntil("desktop Play returns after mobile visits", async () => evaluate(`document.querySelector("#player-name-form")?.classList.contains("visible")`), 10_000);
      await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-reduced-motion", value: "reduce" }] });
      await evaluate(`window.localStorage.setItem("zubspace.arena.v1", "extreme"); true`);
      await send("Page.reload");
      await waitUntil("Chaos default ignores old saved Extreme choice", async () => evaluate(`
        window.__zubspaceArena === "chaos-zone" && document.querySelector("#player-name-form")?.classList.contains("visible")
      `), 10_000);
      // Exercise the advanced URL alias without booting an unintended arena.
      await send("Page.navigate", { url: `${pageBase}&menu=1&zone=extreme-games` });
      await waitUntil("hidden Extreme URL alias", async () => evaluate(`
        window.__zubspaceArena === "extreme" && !document.querySelector("#menu-arena-name")
      `), 10_000);
      alphaUrl = `${pageBase}&menu=1`;
      await send("Page.navigate", { url: alphaUrl });
      await waitUntil("Chaos menu returns", async () => evaluate(`document.querySelector("#player-name-form")?.classList.contains("visible")`), 10_000);
      await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 720, deviceScaleFactor: 1, mobile: false });
      await evaluate(`window.__menuCanvasForTest = document.querySelector("#menu-stars"); document.querySelector("#player-name-input").focus(); true`);
      await send("Input.insertText", { text: "menupilot" });
      // A valid name must not make Enter on either footer button/modal join.
      for (const opener of ["controls-open", "credits-open"]) {
      await evaluate(`document.querySelector("#${opener}").focus(); true`);
      await send("Input.dispatchKeyEvent", { type: "keyDown", key: " ", code: "Space", windowsVirtualKeyCode: 32, text: " " });
      await send("Input.dispatchKeyEvent", { type: "keyUp", key: " ", code: "Space", windowsVirtualKeyCode: 32 });
      await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });
      await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });
      if (!await evaluate(`document.querySelector("#player-name-form").classList.contains("visible") && !document.querySelector('script[src*="mq_js_bundle.js"], script[src*="zubspace_rtc_config.js"]') && typeof wasm_exports === "undefined"`)) throw new Error(`Enter in ${opener} modal joined the arena`);
      await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
      await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
      }
      // Select with genuine pointer input, inspect the enlarged hover preview.
      const point = await evaluate(`(() => { const r = document.querySelector('[data-ship="7"]').getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 }; })()`);
      await send("Input.dispatchMouseEvent", { type: "mouseMoved", ...point });
      await send("Input.dispatchMouseEvent", { type: "mousePressed", button: "left", clickCount: 1, ...point });
      await send("Input.dispatchMouseEvent", { type: "mouseReleased", button: "left", clickCount: 1, ...point });
      if (!await evaluate(`(() => { const p = document.querySelector("#ship-preview"); const r = p.getBoundingClientRect(); return window.zubspaceMenu.selectedShip === 7 && !p.hidden && p.querySelector("h2").textContent === "Shark" && r.top >= 0 && r.left >= 0 && r.right <= innerWidth; })()`)) throw new Error("ship selection/preview failed");
      await save("menu-ship-preview.png");
      await evaluate(`document.querySelector("#player-name-input").focus(); true`);
      await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });
      // A held Enter cannot submit again or open gameplay chat during loading.
      await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13, autoRepeat: true });
      await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });
    }});
    await rendezvousPlayers(alpha, beta, server);
    await waitForPlayersReady(alpha, beta, server);
    await waitUntil("selected Shark reaches authoritative peer render", async () => beta.evaluate(`
      (window.__zubspaceRemoteSpriteTrace || []).some(frame => frame.entries?.some(entry => entry.ship === 7 && entry.drawn && entry.suppression === "none"))
    `), 10_000);
    const startupNotices = (await alpha.metricsState()).uiMetrics.notificationMessages;
    if (!Array.isArray(startupNotices) || startupNotices.some(text => /Downloaded|Login accepted|Joined arena|Browser network connected|Arena directory/i.test(text))) {
      throw new Error(`ordinary startup exposed diagnostics: ${JSON.stringify(startupNotices)}`);
    }
    const sharkId = (await alpha.metricsState()).uiMetrics.selfPlayerId;
    const checkVisibility = async (label, bits, worldVisible, radarVisible) => {
      let observed;
      await waitUntil(label, async () => {
        const state = await beta.pageState();
        const m = state.uiMetrics;
        const entry = state.remoteSpriteTrace?.at(-1)?.entries?.find(entry => entry.id === sharkId);
        const published = m.remoteActorPresentation.find(entry => entry.id === sharkId);
        observed = { label, bits: published?.togglables & 3, drawn: entry?.drawn,
          alpha: entry?.alpha, radar: m.radarPlayerIds.includes(sharkId), observerXradar: m.localGameplayState.abilities.xradar };
        return observed.bits === bits && (entry?.togglables & 3) === bits &&
          observed.drawn === worldVisible && observed.radar === radarVisible &&
          (!worldVisible || observed.alpha === 1);
      }, 5000);
      abilityVisibilityCases.push(observed);
    };
    await checkVisibility("ordinary Shark is visible normally", 0, true, true);
    await alpha.press("6", 60);
    await checkVisibility("stealth hides only the radar dot without tinting the ship", 1, true, false);
    menuScreenshots["stealth-local-ship"] = await alpha.screenshot();
    menuScreenshots["stealth-observer-ship"] = await beta.screenshot();
    await alpha.press("7", 60);
    await checkVisibility("stealth plus cloak hides both ship and radar", 3, false, false);
    await beta.press("8", 60);
    await checkVisibility("X-radar reveals both without changing the target's abilities", 3, true, true);
    menuScreenshots["xradar-reveals-cloaked-shark"] = await beta.screenshot();
    await beta.press("8", 60);
    await checkVisibility("turning X-radar off restores stealth and cloak concealment", 3, false, false);
    await alpha.press("6", 60);
    await checkVisibility("cloak alone hides the ship but leaves the radar dot", 2, false, true);
    await alpha.press("Control", 60);
    await checkVisibility("real weapon use reveals the cloaked ship", 0, true, true);
    if ((await alpha.metricsState()).uiMetrics.chatActive) throw new Error("join Enter leaked into gameplay chat");
    if (await alpha.evaluate("window.zubspaceMenu.active || window.__menuCanvasForTest.width !== 0 || window.__menuCanvasForTest.height !== 0")) throw new Error("menu animation or canvas allocation survived entry to gameplay");
    await alpha.evaluate("delete window.__menuCanvasForTest; true");
    await alpha.press("Escape", 60);
    await alpha.press("q", 60);
    await waitUntil("quit returns to editable saved pilot name", async () => alpha.evaluate(`
      document.querySelector("#player-name-form")?.classList.contains("visible") &&
      document.querySelector("#player-name-input")?.value === "menupilot" &&
      !document.querySelector("#player-name-input")?.classList.contains("hidden")
    `), 15_000);
    await alpha.evaluate(`document.querySelector("#player-name-input").focus(); document.querySelector("#player-name-input").select(); true`);
    // CDP's native insertText drives the ordinary HTML input/change listeners.
    // Existing press() uses the same browser keyboard path for Play.
    // Select text above so the saved identity can change before the next join.
    // The generic helper only exposes evaluate/keys after readiness, so use
    // ordinary character keys to replace the selection.
    for (const char of "newpilot") await alpha.press(char, 35, false);
    await alpha.evaluate(`document.querySelector('[data-ship="0"]').focus(); true`);
    await alpha.press("ArrowRight", 60, false);
    if (!await alpha.evaluate(`window.zubspaceMenu.selectedShip === 1`)) throw new Error("keyboard ship selection failed");
    await alpha.press("Enter", 60, false);
    await waitUntil("returning pilot rejoins", async () => (await alpha.pageState()).uiMetrics?.localGameplayState?.zoneMapReady === true, 30_000);
    await rendezvousPlayers(alpha, beta, server);
    await waitForPlayersReady(alpha, beta, server);
    await waitUntil("keyboard-selected Javelin reaches peer render", async () => beta.evaluate(`
      (window.__zubspaceRemoteSpriteTrace || []).some(frame => frame.entries?.some(entry => entry.ship === 1 && entry.drawn && entry.suppression === "none"))
    `), 10_000);
    const identity = await alpha.evaluate("window.__zubspaceIdentity.name");
    if (identity !== "newpilot") throw new Error(`saved name was not editable: ${identity}`);
    const result = await evidence.succeed({ ok: true, arena,
      abilityVisibilityCases,
      livePlayerCount: true, chaosDefault: true, hiddenExtremeAlias: true, keyboardGuide: true,
      controlsHintTarget: { iconAndAllText: true, paddingClickable: true, hoverFeedback: true, touchOpensModal: true },
      mobileDesktopOnly: true, mobileProfiles: 10, githubLink: true,
      editableSavedName: true, animationStoppedInGame: true,
      shipSelection: { defaultWarbird: true, allEightVisible: true, pointerSharkJoin: true, keyboardJavelinRejoin: true, enlargedPreview: true },
      enterJoin: { nameInput: true, shipFocus: true, modalIsolated: true, repeatGuarded: true },
    }, capture());
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    error.message = `${error.message}\n${browserE2eFailureFooter(artifacts)}`;
    throw error;
  } finally {
    await alpha?.close().catch(() => {});
    await beta?.close().catch(() => {});
    await Promise.all([
      stopProcess(alphaChrome.child),
      stopProcess(betaChrome.child),
      stopProcess(server.child),
    ]).catch(() => {});
    fs.rmSync(alphaChrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    fs.rmSync(betaChrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close();
  }
}

main().catch((error) => {
  console.error(error.stack || error.message);
  process.exit(1);
});
