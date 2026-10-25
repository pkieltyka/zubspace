(function installZubspaceMenu(global) {
  "use strict";
  function zoneHint(params) {
    const hint = params.get("zone") ?? params.get("arena") ?? "chaos-zone";
    return hint.toLowerCase().replace(/[^a-z0-9]/g, "") === "extremegames" ? "extreme" : hint;
  }
  function population(catalog) {
    if (!Array.isArray(catalog?.zones) ||
        !Number.isSafeInteger(catalog.total_players) || catalog.total_players < 0) return null;
    return { players: catalog.total_players };
  }
  function zoneChoices(zones, selected) {
    return zones.filter(zone => zone.listed === true || zone.id === selected ||
      (zone.listed === undefined && zone.id === "chaos-zone"));
  }
  global.zubspaceMenuZoneChoices = zoneChoices;
  function mobileDevice({ userAgent = "", userAgentData, platform = "", maxTouchPoints = 0 } = {}) {
    return userAgentData?.mobile === true || /Android|iPhone|iPad|iPod|Mobile/i.test(userAgent) ||
      // iPadOS can advertise a desktop Mac identity, but still has multitouch.
      (/Mac/i.test(platform) && maxTouchPoints > 1);
  }
  global.zubspaceMenuIsMobileDevice = mobileDevice;
  global.zubspaceMenuZoneHint = zoneHint;
  global.zubspaceMenuPopulation = population;
  const playerName = {
    normalize(value) {
      // JS \s excludes NEL; Rust White_Space includes it. Include the same
      // common invisible edge separators on both sides of admission.
      const name = String(value ?? "");
      const edge = /[\s\u0085\u180e\u200b\u2060\ufeff]/u;
      let start = 0, end = name.length;
      while (start < end && edge.test(name[start])) start++;
      while (end > start && edge.test(name[end - 1])) end--;
      return name.slice(start, end);
    },
    valid(value) {
      return typeof value === "string" && value.length > 0 && value.length <= 14 && !/[^A-Za-z0-9!*]/.test(value);
    },
  };
  global.zubspacePlayerName = playerName;
  const admissionMessages = {
    1: "That username is currently in use. Try another name.",
    2: "Sorry, the arena is full at this time.",
    3: "Unable to connect. Please try again.",
    4: "Your previous session is still connected. Close the other game and try again.",
    5: "Use 1–14 letters, numbers, ! or * for your player name.",
  };
  global.zubspaceMenuAdmission = {
    message(code) { return [1, 2, 3, 4, 5].includes(code) ? admissionMessages[code] : null; },
    recoverStoredIdentity(rejection, stored, name, playerId) {
      // The roster is only an advisory. Let the server decide whether this
      // stored identity is waiting for its own stale session or another user.
      return rejection === 1 && typeof playerId === "string" && stored?.playerId === playerId &&
        typeof stored.name === "string" && playerName.normalize(stored.name).toLowerCase() === playerName.normalize(name).toLowerCase();
    },
    // Names are process-wide; capacity belongs to the selected zone.
    rosterError(roster, name, zone = null) {
      const players = Number.isSafeInteger(roster?.total_players) &&
        Array.isArray(roster.players) && roster.total_players === roster.players.length ? roster.players : null;
      const key = playerName.normalize(name).toLowerCase();
      if (players?.some(player => typeof player?.name === "string" && playerName.normalize(player.name).toLowerCase() === key)) return 1;
      if (Number.isSafeInteger(zone?.total_players) && zone.total_players >= 0 &&
          Number.isSafeInteger(zone.max_players) && zone.max_players > 0 && zone.total_players >= zone.max_players) return 2;
      return null;
    },
    async check(url, name, zoneUrl = null) {
      if (!url) return null;
      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), 1500);
      const read = async target => {
        if (!target) return null;
        try {
          const response = await global.fetch(target, { cache: "no-store", signal: controller.signal });
          return response.ok ? await response.json() : null;
        } catch { return null; }
      };
      try {
        const [roster, zone] = await Promise.all([read(url), read(zoneUrl)]);
        return this.rosterError(roster, name, zone);
      } finally { clearTimeout(timeout); }
    },
    takeRetry(history, server, now) {
      try {
        const { zubspaceJoinRetry: retry, ...rest } = history.state || {};
        if (retry) history.replaceState(rest, "");
        if (retry?.server !== server || !this.message(retry.code) ||
            !Number.isFinite(retry.at) || now < retry.at || now - retry.at > 300000 ||
            typeof retry.name !== "string" || retry.name.length > 128 ||
            (retry.code !== 5 && !playerName.valid(playerName.normalize(retry.name))) ||
            !/^[0-9]{12}$/.test(retry.playerId || "") ||
            !Number.isInteger(retry.ship) || retry.ship < 0 || retry.ship > 7) return null;
        return { ...retry, name: playerName.normalize(retry.name) };
      } catch { return null; }
    },
  };
  if (!global.document) return;
  const overlay = document.querySelector("#loading-overlay");
  const canvas = document.querySelector("#menu-stars");
  const context = canvas?.getContext("2d", { alpha: false });
  if (!overlay || !canvas) return;
  const mobile = mobileDevice(global.navigator);
  // These thumbnails are original ships.bm2 frames, arranged in ship order.
  const ships = [
    ["Warbird", "The most agile and dangerous ship in the fleet once mastered."],
    ["Javelin", "The fastest ship in the fleet. Give yourself room to turn."],
    ["Spider", "Cloak and ambush. X-radar can spoil the surprise."],
    ["Leviathan", "Blue bombs hit hardest—and come with a bigger energy bill."],
    ["Terrier", "Powerful double-barrel guns. Wider coverage, slightly slower firing."],
    ["Weasel", "Special EMP bombs stall enemy recharge. Your opening won’t last forever."],
    ["Lancaster", "Bombs bounce once. Make the angle work for you."],
    ["Shark", "Rapid-fire guns, agility, and cloak. Watch your energy."],
  ];
  let selectedShip = 0;
  const options = document.querySelector("#ship-options");
  const preview = document.querySelector("#ship-preview");
  function showShip(index) {
    preview.style.setProperty("--ship-index", index);
    preview.querySelector("h2").textContent = ships[index][0];
    preview.querySelector(".ship-preview-number").textContent = `SHIP 0${index + 1}`;
    preview.querySelector(".ship-preview-description").textContent = ships[index][1];
    preview.hidden = false;
  }
  const shipButtons = ships.map(([name], index) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "ship-option";
    button.dataset.ship = index;
    button.style.setProperty("--ship-index", index);
    button.setAttribute("role", "radio");
    button.setAttribute("aria-checked", String(index === 0));
    button.setAttribute("aria-label", `${index + 1}. ${name}`);
    button.setAttribute("aria-describedby", "ship-preview");
    button.tabIndex = index === 0 ? 0 : -1;
    button.innerHTML = `<span class="ship-sprite" aria-hidden="true"></span><span class="ship-option-name"><span>${index + 1}</span> ${name}</span>`;
    button.addEventListener("click", () => selectShip(index));
    button.addEventListener("mouseenter", () => showShip(index));
    button.addEventListener("focus", () => showShip(index));
    button.addEventListener("mouseleave", () => { preview.hidden = true; });
    button.addEventListener("blur", () => { preview.hidden = true; });
    button.addEventListener("keydown", event => {
      if (event.key === "Escape") { preview.hidden = true; event.preventDefault(); return; }
      const offset = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: 4, ArrowUp: -4 }[event.key];
      if (offset === undefined && event.key !== "Home" && event.key !== "End") return;
      event.preventDefault();
      const next = event.key === "Home" ? 0 : event.key === "End" ? 7 : (index + offset + 8) % 8;
      selectShip(next);
      shipButtons[next].focus();
    });
    options.appendChild(button);
    return button;
  });
  function selectShip(index) {
    selectedShip = index;
    shipButtons.forEach((button, i) => {
      button.setAttribute("aria-checked", String(i === index));
      button.tabIndex = i === index ? 0 : -1;
    });
  }
  options.addEventListener("mouseleave", () => { preview.hidden = true; });
  overlay.dataset.device = mobile ? "mobile" : "desktop";
  document.querySelector("#mobile-join-message").hidden = !mobile;
  const dialog = document.querySelector("#controls-dialog");
  document.body.appendChild(dialog);
  global.zubspaceShowControls = () => {
    if (!dialog.open) {
      dispatchEvent(new Event("zubspace-controls-open"));
      dialog.showModal();
    }
  };
  // The guide lights both keyboard layouts, or just one; the game accepts both.
  const layoutButtons = [...dialog.querySelectorAll(".controls-layouts button")];
  const showLayout = event => {
    dialog.dataset.layout = event.currentTarget.dataset.layoutOption;
    for (const button of layoutButtons) button.setAttribute("aria-pressed", String(button === event.currentTarget));
  };
  for (const button of layoutButtons) button.addEventListener("click", showLayout);
  const controlsOpen = document.querySelector("#controls-open");
  const controlsClose = document.querySelector("#controls-close");
  const credits = document.querySelector("#credits-dialog");
  const creditsOpen = document.querySelector("#credits-open");
  const creditsClose = document.querySelector("#credits-close");
  const openControls = () => { if (active && !dialog.open && !credits.open) dialog.showModal(); };
  const closeControls = () => dialog.close();
  const openCredits = () => { if (active && !credits.open && !dialog.open) credits.showModal(); };
  const closeCredits = () => credits.close();
  // Keep the global game keyboard bridge out of the modal's input layer.
  // Native dialog defaults still provide Escape, Tab and button activation.
  const modalKeys = event => {
    if (!dialog.open && !credits.open) return;
    event.stopImmediatePropagation();
    if (dialog.open && event.code === "KeyK") {
      event.preventDefault();
      if (event.type === "keydown" && !event.repeat) dialog.close();
    }
  };
  const backdropClick = event => {
    const modal = event.currentTarget;
    const rect = modal.getBoundingClientRect();
    if (event.target === modal && (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom)) modal.close();
  };
  controlsOpen.addEventListener("click", openControls);
  controlsClose.addEventListener("click", closeControls);
  dialog.addEventListener("click", backdropClick);
  creditsOpen.addEventListener("click", openCredits);
  creditsClose.addEventListener("click", closeCredits);
  credits.addEventListener("click", backdropClick);
  addEventListener("keydown", modalKeys, true);
  addEventListener("keyup", modalKeys, true);
  const reduced = matchMedia("(prefers-reduced-motion: reduce)");
  let width = 0, height = 0, frame = 0, previous = 0, rotation = 0, active = true, launching = false;
  const stars = Array.from({ length: 260 }, () => ({ x: Math.random() * 2 - 1, y: Math.random() * 2 - 1, z: Math.random() * 1.8 + .2 }));
  function resize() {
    width = innerWidth; height = innerHeight;
    const ratio = Math.min(devicePixelRatio || 1, 2);
    canvas.width = Math.round(width * ratio); canvas.height = Math.round(height * ratio);
    context?.setTransform(ratio, 0, 0, ratio, 0, 0);
    draw(0);
  }
  function draw(dt) {
    if (!context) return;
    context.fillStyle = "#030303"; context.fillRect(0, 0, width, height);
    const scale = Math.max(width, height) * .6;
    rotation += dt * .017;
    for (const star of stars) {
      star.z -= dt * (launching ? .7 : .10);
      if (star.z < .08) { star.z = 2; star.x = Math.random() * 2 - 1; star.y = Math.random() * 2 - 1; }
      const angle = rotation + .045 / star.z;
      const sx = star.x * Math.cos(angle) - star.y * Math.sin(angle);
      const sy = star.x * Math.sin(angle) + star.y * Math.cos(angle);
      const x = width * .5 + sx * scale / star.z;
      const y = height * .44 + sy * scale / star.z;
      if (x < -10 || x > width + 10 || y < -10 || y > height + 10) continue;
      const alpha = Math.min(.8, Math.max(.08, (2 - star.z) * .4));
      context.strokeStyle = `rgba(255,255,255,${alpha})`;
      context.lineWidth = Math.max(.5, 1.35 - star.z * .45);
      context.beginPath();
      const trail = reduced.matches ? 0 : (launching ? .055 : .008);
      context.moveTo(width * .5 + sx * scale / (star.z + trail), height * .44 + sy * scale / (star.z + trail));
      context.lineTo(x + .25, y + .25); context.stroke();
    }
  }
  function stop() {
    if (!active) return;
    active = false;
    if (dialog.open) closeControls();
    if (credits.open) closeCredits();
    controlsOpen.removeEventListener("click", openControls);


    creditsOpen.removeEventListener("click", openCredits);
    creditsClose.removeEventListener("click", closeCredits);
    credits.removeEventListener("click", backdropClick);


    canvas.width = 0; canvas.height = 0; stars.length = 0;
    dispatchEvent(new Event("zubspace-menu-exit"));
    cancelAnimationFrame(frame); clearInterval(watch);
    removeEventListener("resize", resize); document.removeEventListener("visibilitychange", visibility);
    reduced.removeEventListener("change", motion);
  }
  function tick(now) {
    if (!active || !overlay.isConnected || overlay.classList.contains("hidden")) { stop(); return; }
    frame = requestAnimationFrame(tick);
    if (now - previous < 33) return;
    draw(Math.min((now - (previous || now)) / 1000, .05)); previous = now;
  }
  function motion() {
    cancelAnimationFrame(frame); previous = 0;
    if (active && context && !document.hidden && !reduced.matches) frame = requestAnimationFrame(tick);
    else if (active) draw(0);
  }
  function visibility() { motion(); }
  addEventListener("resize", resize); document.addEventListener("visibilitychange", visibility);
  reduced.addEventListener("change", motion);
  // Also retire a reduced-motion/static canvas when the game removes its menu.
  const watch = setInterval(() => { if (!overlay.isConnected || overlay.classList.contains("hidden")) stop(); }, 500);
  addEventListener("pagehide", stop, { once: true });
  global.zubspaceMenu = {
    get active() { return active; },
    mobile,
    get selectedShip() { return selectedShip; },
    setSelectedShip(index) {
      if (!Number.isInteger(index) || index < 0 || index > 7) return false;
      selectShip(index);
      return true;
    },
    setPopulation(status, arena) {
      const element = document.querySelector("#arena-population");
      const counts = population(status, arena);
      element.dataset.state = counts ? "live" : "unavailable";
      element.textContent = counts ? `${counts.players} ${counts.players === 1 ? "player" : "players"} online` : "Arena status unavailable";
    },
    launch() {
      launching = true;
      shipButtons.forEach(button => { button.disabled = true; });
      preview.hidden = true;
      document.querySelector("#loading-panel").classList.add("launching");
    },
    stop,
  };
  resize(); motion();
})(typeof window === "undefined" ? globalThis : window);
