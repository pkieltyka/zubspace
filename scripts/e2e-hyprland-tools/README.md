# e2e-hyprland-tools

Keep zubspace test browsers out of the way on a Hyprland desktop.

Every headed browser the repo launchers start carries the window class
`zubspace-e2e` (`tests/e2e/combat_replay.mjs` passes `--class` to Chrome,
`tools/browser_firefox_rtc_smoke.mjs` to Firefox). These tools register one
Hyprland runtime window rule that matches exactly that class and opens such
windows on background workspace 99 silently, so the user's focus and
workspace never move and the user's own Chrome/Firefox windows are untouched.
The rule enables `render_unfocused` only for test windows so native GPU frame
callbacks keep running on the hidden workspace; it leaves the global background
frame cap unchanged. The rule also sets `focus_on_activate = false` and `suppress_event =
"activate"`: the E2E helpers call CDP `Page.bringToFront`, an activation
request that would otherwise switch the user to workspace 99 when
`misc:focus_on_activate` is on. It floats test windows at 1440x1000 (the
Xvfb gate screen size) because a tiled window alone on the workspace fills
the monitor and swiftshader cadence drops well below 60 fps; hidden windows
themselves keep 60 fps `requestAnimationFrame`.

| Tool | What it does |
| --- | --- |
| `run [--keep] -- <command>` | Enable the rule, run the command, disable the rule again. Outside Hyprland it just runs the command. |
| `enable` | Register/re-enable the rule for the rest of the Hyprland session (idempotent). |
| `disable` | Switch the rule off again. |
| `status` | Rule state, active workspace, open test windows. |
| `close` | SIGTERM (then SIGKILL) processes owning leftover test windows. |
| `show` / `back` | Move the user's view to workspace 99 and back. Only when the user asks to look. |

```sh
scripts/e2e-hyprland-tools/run -- make test-e2e-combat
scripts/e2e-hyprland-tools/enable      # several gates in a row ...
make test-e2e-single-bullet
scripts/e2e-hyprland-tools/status
scripts/e2e-hyprland-tools/disable
```

Environment: `ZUBSPACE_E2E_WINDOW_CLASS` (default `zubspace-e2e`) and
`ZUBSPACE_E2E_HYPR_WORKSPACE` (default `99`) are read by both the tools and
the launchers. `run` also exports `MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11`
because Firefox applies `--class` only through XWayland.

Ad-hoc browsers must pass the same class to be captured:

```sh
google-chrome --class=zubspace-e2e --ozone-platform=x11 --user-data-dir=/tmp/zub-debug about:blank
MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 firefox --class zubspace-e2e --new-instance --profile /tmp/zub-ff about:blank
```

Requirements: Hyprland >= 0.55 (Lua config, `hyprctl repl`) and access to the
Hyprland unix socket. Inside a sandbox that blocks unix sockets `hyprctl`
fails with "Couldn't open a socket"; run the tools outside it. The rule and
its handle live in the Lua global `_G.__zubspace_e2e` until `disable` or a
Hyprland reload.
