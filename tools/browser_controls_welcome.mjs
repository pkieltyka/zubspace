// Follow the first-play UI in browser tests/probes without bypassing its storage gate.
export async function acknowledgeControlsWelcome(send, evaluate) {
  const needed = await evaluate(`typeof window.zubspaceMenu?.welcomeControls === "function" &&
    (window.zubspaceMenu.welcomeControls.needed?.() ?? localStorage.getItem("zubspace.controls-welcome.v1") !== "1")`);
  if (!needed) return false; // Returning browsers and older deployed versions.
  const deadline = Date.now() + 5000;
  while (!await evaluate('document.querySelector("#controls-dialog")?.open && document.querySelector("#controls-dialog").dataset.welcome === "true"')) {
    if (Date.now() >= deadline) throw new Error('first-play controls welcome did not open');
    await new Promise(resolve => setTimeout(resolve, 25));
  }
  await evaluate('document.querySelector("#controls-ready").focus(); true');
  for (const type of ['keyDown', 'keyUp']) {
    await send('Input.dispatchKeyEvent', { type, key: ' ', code: 'Space', windowsVirtualKeyCode: 32, text: type === 'keyDown' ? ' ' : undefined });
  }
  return true;
}
