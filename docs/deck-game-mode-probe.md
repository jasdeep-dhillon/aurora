# Steam Deck launch-route probe

Use these observations before picking Aurora’s Game Mode input backend.
Start with a native Aurora build in Desktop Mode, then repeat the supported
cases in stock Steam Game Mode. These are separate baselines for the future
standalone Aurora session. The probe reads local device and session state.
It does not install anything, take control of a device, launch Steam, or change
the boot session.

1. Record the Deck model, SteamOS build, Aurora build SHA (or release version),
   and installed SDL version if available. Run `./tools/deck-session-probe.sh`
   to identify the built-in Deck controller's event and HID nodes. Choose only
   nodes whose names and sysfs paths match the built-in controller. Use the
   interface path and event capability mask to distinguish similarly named
   devices. Ignore unrelated keyboards and controllers.
2. Use an installed title you own for each route below. Record the route
   Aurora reports for the actual launch in its `<game> starts as <mode>` log
   line. Process ancestry corroborates it but can change as processes exit or
   reparent. Record Prefer Official Steam, the experimental headless anti-cheat
   setting, any per-game Proton override, and whether the title is on the
   Steam DRM app list. With Steam closed and the helper available, a DRM title
   can select HeadlessHost. For an official launch with Steam closed, use
   the normal Prefer Official Steam option where applicable or a title that
   already selects OfficialSteam. If a route is unavailable, mark it as such.
3. For each case take snapshots before launch, while the game has focus,
   and after exit. Replace the example node numbers with the ones found above
   and use a label such as `proton-focused` or `official-already-open-after`.
   Save output locally if you want to attach it to the review.

   ```bash
   ./tools/deck-session-probe.sh proton-focused /dev/input/event12 /dev/hidraw3
   ```

   A second terminal, a delayed command, or an existing SSH session lets the
   snapshot run without taking focus from the game. X11 properties are optional
   evidence: run from the intended session's environment and note when a
   property or display is absent. Over SSH, explicitly pass the intended
   session’s `DISPLAY` and, if required, its `XAUTHORITY`. Gamescope may have
   multiple XWayland displays, so confirm the session values locally and keep
   the authorization file private. The Gamescope base-layer app ID is a
   request, and a focused app ID can be shared by windows in a shortcut.
   Compare window IDs, process identity and device holders across snapshots.
   If a node disappeared, keep the snapshot and use its new inventory to find
   a replacement. No visible holders is inconclusive under limited permissions.

4. While the game has focus, try STEAM, Quick Access (⋯), and STEAM + X.
   Record which menu appears, whether the game also sees the press, and what
   receives focus when the menu closes or the game exits. If `evtest` is
   already available and the chosen controller node is readable, optionally
   observe only the verified gamepad `/dev/input/eventN` node with
   `evtest /dev/input/eventN`. Choose the node with gamepad buttons and axes.
   Leave the built-in keyboard, mouse-emulation interfaces and `hidraw` nodes
   out of this event trace. Run without `--grab` and stop immediately after
   the test chords. If a button
   does not appear there, record that observation without assuming the
   controller or Steam never received it. Do not record text entry or a
   keyboard device.

### Desktop Mode baseline

| Case | Starting state | Expected observation to verify |
|---|---|---|
| Aurora-managed Proton | Official Steam closed | Whether Aurora can launch and whether Steam or the host starts |
| Headless host | Official Steam closed | Which process owns input and how the game returns to Aurora |
| Official handoff | Official Steam closed | Whether Steam starts, what it does with focus and buttons, and whether it exits afterward |
| Official handoff | Official Steam open | Whether the existing client stays open and control returns to Aurora |

### Stock Steam Game Mode baseline

Add native Aurora as a non-Steam shortcut and start it from the stock Steam UI.
Steam is already open, so HeadlessHost will not be selected. Record one launch
Aurora manages (including NoSteam if selected) and one that hands off to
OfficialSteam. For each, check whether the two menus and the keyboard belong to
Steam or Aurora, and whether the Aurora shortcut and its game share a Gamescope
app ID. Keep this stock-session result separate from the standalone Aurora
session. After the latter exists, rerun the Desktop matrix inside its embedded
Gamescope session to confirm the actual backend and handoff.

For each row, report: title or app ID, observed launch route, client/process
changes, device holders, button/keyboard behavior, focus after exit, and any
observation blocked by permissions. Share only the relevant excerpts.
Keep raw Aurora logs local because they can contain account names and paths.

## Desktop check when a Deck is unavailable

A synthetic XWayland fixture checks one narrow prerequisite: Gamescope focuses a
shell window, gives focus to a second app-ID window, then restores the shell when
the second window closes. It does not start Aurora or use the logged-in Steam
profile. Run the fixture inside the disposable compositor, with X11 development
headers and a C compiler installed:

```bash
probe_dir=$(mktemp -d)
cc -Wall -Wextra -Werror tools/gamescope-focus-probe.c -o "$probe_dir/focus" -lX11
timeout -k 3 18 gamescope --backend headless --steam -w 1280 -h 800 -- "$probe_dir/focus"
```

If the local GPU driver cannot run the headless backend, retry with
`VK_DRIVER_FILES` pointing to the installed Mesa lavapipe ICD. On the initial
CachyOS development host, Gamescope 3.17.0 reached XWayland with NVIDIA Vulkan,
then reported an NVVM compute pipeline error. The lavapipe XWayland run
reported all three expected focus transitions and exited successfully.
This result leaves physical Deck input, real-game overlays and text delivery,
official Steam coexistence, embedded boot, and boot recovery unverified.
