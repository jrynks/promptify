# Linux and Steam Deck insertion investigation

Research date: 2026-10-03. Status: architecture and upstream capability evidence
plus initial destination-adapter development; not native runtime certification.

## Executive finding

A single cross-source user experience is achievable through internal adapters,
but there is no single Linux API providing activation, exact destination
identity, clipboard transfer, keyboard delivery, and proof of receipt.
Distribution name alone is insufficient: session, compositor, portal backend,
application toolkit, sandbox, and package version determine capabilities.

GNOME/KDE provide the strongest permissioned Wayland delivery building blocks.
X11 supports the existing native paste approach without a RemoteDesktop portal.
Sway/Hyprland/Cinnamon Wayland need additional adapter feasibility work.
Steam Deck Desktop Mode is a required hardware target; Gaming Mode is a
separate unresolved integration target, not covered by KDE compatibility.

Do not claim all major distros or Steam Deck modes work based on upstream
interfaces or compilation alone. Do not respond to missing RemoteDesktop by
attempting unrestricted synthetic input on Wayland.

## Requirement retained

One setup and delivery workflow, no source-specific paste toggle, distro
selector, compositor selector, keyboard-device selector, or instructions to
disable a read-only OS. Backend selection is automatic. OS-native permission
requests and honest unavailable-capability errors remain necessary.

This investigation does not recommend removing privacy controls. Insertion
permission, minimal local delivery verification, microphone access, and model
context capture are separate capabilities and consents.

## Repository-specific gaps

The table below records the pre-implementation baseline. Current development has
added exact-field metadata adapters, permissioned GlobalShortcuts on Wayland, and
removed raw keyboard monitoring. Generated clipboard text now remains available
until an explicit restore action; supported previous content is retained only in
process memory. Native Linux clipboard ownership, portal, compositor and Steam
Deck certification remain unresolved. See the
[implementation status](./UNIVERSAL-INSERTION-PLAN.md) for current validation.

| Surface | Current implementation | Gap to universal behavior |
|---|---|---|
| Destination identity | Window handle + PID; KWin focus script on KDE Wayland, x-win elsewhere | No exact field identity; GNOME extension dependency; no general Gamescope/wlroots adapter |
| Input delivery | Native clipboard chord; Wayland RemoteDesktop keysym session | No positive target receipt, capability negotiation, or EIS transport |
| Clipboard | arboard with `wayland-data-control`; fixed 350 ms restoration | Clipboard ownership/protocol compatibility and delayed consumption need platform tests; formats and restoration errors need handling |
| Activation | Tauri global shortcuts; optional physical evdev modifier monitor on Wayland | No explicit GlobalShortcuts portal adapter in app code; physical keyboard workflow does not satisfy controller-only Deck |
| Focused text | UIA on Windows; non-Windows returns no focused text | No AX/AT-SPI destination inspection or verification in this interface |
| Linux packaging | Fedora/Bazzite source-build instructions; installer script requests NSIS | No reviewed Flatpak manifest or distro-independent Linux release pipeline |
| Device resources | Vulkan-backed local models and default microphone via CPAL | No Deck memory/latency/audio/suspend evidence; desktop GPU tests are not Deck evidence |

The code already treats a failed portal reply as ambiguous and does not
automatically repeat the paste. Preserve that safety property.

## Compatibility matrix: candidates, not certifications

| Required target | Candidate session/backend route | What must be demonstrated |
|---|---|---|
| Ubuntu and Debian GNOME | Native GNOME Wayland portals; X11 where available | Minimum supported backend version, activation, non-extension focus evidence, clipboard, actual IDE/browser insertion |
| Fedora Workstation | GNOME Wayland | Same, including permission revocation and native Wayland versus Xwayland targets |
| Fedora KDE and openSUSE KDE | Plasma Wayland; X11 only when installed/supported | KWin and sandbox focus access, portal permissions, clipboard, actual insertion |
| Arch KDE | Plasma route, detected at runtime | Rolling-version regression coverage; no assumption packages are installed |
| Linux Mint Cinnamon/Xfce/MATE | X11 native paste where that is the running session | Activation, accessibility evidence, selections and clipboard; Cinnamon Wayland is a separate capability target |
| Arch Sway/wlroots | Compositor IPC + accessibility candidates | Standard RemoteDesktop is absent from wlr backend; find a safe packaged activation and delivery route without root or system workarounds |
| Arch Hyprland | GlobalShortcuts portal + compositor/accessibility candidates | RemoteDesktop absence does not remove shortcut support; safe delivery still needs proof |
| Bazzite KDE/GNOME | Appropriate desktop backend, immutable-host packaging | Flatpak permissions, sidecar/GPU/audio, focus access and persistence without host modifications |
| SteamOS Steam Deck Desktop | Detect actual installed session; Valve documents KDE desktop | LCD/OLED native tests, desktop activation without physical keyboard, default mic, memory/GPU and suspend/resume |
| SteamOS/Bazzite Deck Gaming Mode | Gamescope, distinct from desktop compositor | No verified standard portal route for remote-input plus global activation/focus; dedicated feasibility gate |

Pin actual OS/backend versions in the eventual test results. Do not infer a
SteamOS session's X11/Wayland default from a generic KDE page or secondary
"2026 guide"; primary evidence reviewed does not establish that default.

## Upstream mechanisms and limits

### RemoteDesktop and EIS

The RemoteDesktop portal provides session/device selection, user authorization,
keyboard notifications, and on supporting versions ConnectToEIS. Probe the
actual D-Bus interface version and methods at runtime. A portal service or
package being installed does not prove its selected backend implements the
needed interface or that the user authorized it.

Legacy keysym notifications and EIS are alternative internal transports, not
user settings. Retain legacy delivery only on an explicitly negotiated path.
EIS adds an authorized input channel; it does not locate an AI composer or
prove the application consumed the text.

InputCapture is not interchangeable with RemoteDesktop: capture/forwarding
capability is not permission to inject arbitrary text into another input.

### Active window and exact control

There is no general focused-widget/caret API in the desktop portals reviewed.
GNOME Shell introspection has access restrictions. KWin scripting provides
window metadata, not field identity, and sandbox access to its D-Bus/script
loading must be assessed separately.

Use accessibility metadata when available; validate ownership and fresh focus.
AT-SPI is a candidate for editable-text/selection inspection on Linux, not
proof all Chromium/Electron/GTK/Qt controls expose usable semantics.
Unavailable accessibility must remain an explicit capability state.

AT-SPI's `EditableText` interface also exposes `InsertText` and
`SetTextContents`: it is a candidate direct insertion transport, not merely
inspection. Prefer selection-aware insertion; `SetTextContents` must not
silently replace an existing value. Its Registry supports focus events, not a
universal synchronous `GetFocus`. Combine events with fresh state/ownership
checks and invalidate stale objects after app or accessibility-bus restart.

Flatpak provides accessibility-bus plumbing, but permission to act on another
application's arbitrary editable object remains unproven for the intended
manifest. Do not equate bus visibility with cross-application write access.
Electron, browser, Qt and GTK provider behavior needs exact-field tests.

For compositor IPC, use authenticated, bounded internal adapters and narrowly
scoped package access. A cached active-window event is insufficient: confirm
fresh destination state before the write. No title-based guess can safely
distinguish chat input, editor, and terminal in one window.

### Clipboard

Keyboard delivery and clipboard delivery are independent requirements.
`wayland-data-control` support in a library does not prove the running
compositor advertises a compatible protocol. GNOME, KDE, wlroots and Xwayland
bridges need distinct interoperability evidence.

The Clipboard portal is associated with authorized sessions; it is not a
universal focused-field insertion API. Detect backend support rather than
assuming every RemoteDesktop implementation also supports clipboard.

The research follow-up confirms KDE's current source advertises Clipboard
and implements selection transfer, restricted to supported active sessions
and Wayland. This does not prove availability in any particular stable distro
package. Ordinary GTK/Qt clipboard APIs also exist without a portal; however,
being a windowed app does not prove an unfocused background owner can write at
the moment Promptify needs it.

The upstream wlr data-control protocol is deprecated in favor of
`ext-data-control-v1`. Audit arboard's actual negotiated protocol support rather
than infer it from the feature name. GNOME does not expose the same reviewed
data-control path; toolkit/authorized portal routes require a separate
feasibility test. Xwayland clipboard bridging remains a native-test requirement,
not an established solution in this report.

Test clipboard ownership/lifetime, delayed reads, clipboard managers, mixed
native/Xwayland applications, non-ASCII text, selection replacement, original
format preservation, and concurrent user copies. Clipboard restoration cannot
be timed solely from completion of a synthetic keystroke.

### Activation and controller-only operation

Use the GlobalShortcuts portal where implemented and an internal native
activation adapter elsewhere. Support activation/deactivation signals,
conflicts, rebinding, cancellation, session closure and permission revocation.
Registration is not proof a hotkey reaches Promptify in another application's
focus, particularly when running under Xwayland.

The underlying `global-hotkey` project documents Linux support as X11-only;
the Tauri plugin's broad Linux support label is not evidence of a Wayland
portal backend. Audit the resolved plugin implementation before reuse and add
an explicit permissioned portal adapter where needed. Do not rely on a hidden
Xwayland shortcut connection to capture native Wayland application's input.

Steam Input remapping is not a verified background global activation API.
A controller emitting a keyboard chord also does not establish that Promptify
can receive it without a physical keyboard. The current evdev selection filters
out virtual keyboards and limited gamepad keyboard devices.

A controller-only path must capture the destination before a touch/controller
action gives Promptify focus, then restore only through a supported deliberate
user flow. Opening an overlay and guessing the prior input is not acceptable.
Validate the entire activation-to-delivery cycle, not just the paste transport.

## Steam Deck: distinct feasibility gates

### Desktop Mode

Valve describes a stock KDE Plasma experience. This makes existing desktop
mechanisms plausible, not certified. Required tests:

- Stock stable SteamOS LCD and OLED, recording exact image and session versions.
- No external keyboard; activation, stop, cancel, permission dialogs and recovery
  navigable with controller/touch. Repeat with a docked keyboard.
- Browser textareas/contenteditable and VS Code chat, including native/Xwayland
  variants actually available on the device.
- Default onboard mic, headset/Bluetooth transition, silence and mic denial.
- Fit setup/overlay/actions on Deck's 1280x800 display and actual scaling.
- Model memory together with desktop applications; GPU sharing, inference
  latency, thermals, battery and CPU fallback.
- Suspend/resume, desktop/game-mode switching, portal restart and OS update:
  discard stale destination tokens and establish fresh permission/session state.

### Gaming Mode

Gamescope and its portal backend must not be treated as KDE. The reviewed
gamescope portal implementation advertises Access/ScreenCast/Screenshot rather
than the needed RemoteDesktop/GlobalShortcuts path.

That is evidence of a missing standard route, not proof every possible native
or accessibility adapter is impossible. Before claiming support, establish:

1. A sanctioned background activation mechanism while another app is active.
2. Fresh focused window/control identity across Steam app switches.
3. Permissioned delivery and clipboard availability for actual target apps.
4. Visible, controller-navigable permission/recovery UI without stealing input.
5. No reliance on root, disabling SteamOS read-only mode, broad input access,
   unreviewed plugins, or changing game security behavior.

Desktop Mode working does not close these gates. If the requirement includes
Gaming Mode, unresolved gates block that support claim and must be presented
as an outstanding engineering constraint, not silently scoped away.

## Packaging decision

Flatpak is the preferred packaging feasibility spike for immutable systems,
including Deck/Bazzite, not an already proven solution. AppImage or native
packages may be useful, but do not solve missing permissions/compositor APIs.

Evaluate together:

- WebKitGTK/Tauri runtime and native worker/CLI placement.
- Vulkan driver extension/runtime compatibility and real Deck acceleration.
- CPAL/default microphone routing through the shipped audio environment.
- Portal app identity, desktop entry, autostart/background lifecycle.
- Accessibility bus access, compositor metadata access and sandbox PID mapping.
- Model download/storage, permissions, offline startup and upgrade preservation.
- Minimum filesystem/network/D-Bus/device permissions; no `--device=all` or
  unrestricted session-bus access as a blanket workaround.

Portal D-Bus access is normally exposed to Flatpak applications; do not add
broad permissions redundantly. Host compositor scripts may require access
Flatpak does not grant by default. A package that launches but cannot safely
identify or reach a field does not meet the requirement.

No end-user compiler/SDK installs, rpm-ostree layering, or mutable SteamOS root
should be necessary for the shipped runtime.

## Evidence and validation deliverables

### Implementation feasibility spikes, in dependency order

1. Prove a common capability/destination/receipt contract and test fake backend
   failure states without binding it to prompt classification.
2. Prove native X11 and GNOME/KDE Wayland activation -> destination capture ->
   clipboard -> delivery -> receipt using known text, no speech/model dependency.
   Include sandboxed and unsandboxed packages.
3. Prove AT-SPI editable-field identity, selection-aware insertion and minimal
   verification across actual GTK/Qt/Chromium/Electron inputs. If a provider or
   sandbox blocks it, keep that capability unavailable rather than substituting
   a guessed target.
4. Resolve wlr/Hyprland/Cinnamon Wayland delivery with supported internal
   adapters; no raw-input/root workaround. Missing standard portals are gaps,
   not proof accessibility or every other transport is impossible.
5. Run stock Steam Deck Desktop/controller and Gaming Mode feasibility gates
   separately, followed by voice/model/resource tests on hardware.
6. Only then migrate workaround settings and ship the common UX; native
   platform acceptance remains a release requirement.

Implementation now includes bounded AT-SPI active-window discovery, permissioned
GlobalShortcuts activation, and one common enable/disable action acquiring both
input and shortcut sessions. Raw evdev access and physical-keyboard selection
are removed; old settings remain readable but are not written again. These
changes are source and supplementary host-test evidence, not native certification.

Blocking unknowns: Flatpak cross-app accessibility policy, native provider coverage
for the new focus path, GNOME background clipboard path, installed
backend/version coverage, Xwayland bridging, controller-only activation,
Gamescope destination/delivery, and Deck resource/suspend behavior.

Build a content-free capability report with session/backend versions,
activation, target evidence, clipboard, transport, permission and verification
status. Do not log transcripts, clipboard text, window captions or URLs.

For each matrix row, record package hash/version, architecture, OS image,
desktop/session, portal versions, sandbox, target app versions and display
scaling. Run the common exact-output/duplicate/focus/cancellation acceptance
suite plus platform-specific failures. Actual destination observations are
required; mocks and cross-compilation are supplementary only.

Test Debian/Ubuntu, Fedora GNOME/KDE, Mint X11, openSUSE KDE, Arch KDE,
Sway/Hyprland, Bazzite and stock Deck separately. A desktop-family result can
guide test reuse but does not automatically certify every distribution.

This session has a Windows host and has not demonstrated an authorized
interactive Linux or Steam Deck test device. No Linux or Deck paste success,
controller activation, or package compatibility is claimed.

## Primary-source index

- [RemoteDesktop specification](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html)
- [Clipboard specification](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Clipboard.html)
- [GlobalShortcuts specification](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.GlobalShortcuts.html)
- [Portal backend selection](https://flatpak.github.io/xdg-desktop-portal/docs/portals.conf.html)
- [GNOME portal implementation](https://gitlab.gnome.org/GNOME/xdg-desktop-portal-gnome)
- [GNOME Shell introspection](https://gitlab.gnome.org/GNOME/gnome-shell/-/blob/main/js/misc/introspect.js)
- [KDE portal implementation](https://invent.kde.org/plasma/xdg-desktop-portal-kde)
- [KWin implementation](https://invent.kde.org/plasma/kwin)
- [Xapp portal implementation](https://github.com/linuxmint/xdg-desktop-portal-xapp)
- [wlroots portal scope](https://github.com/emersion/xdg-desktop-portal-wlr)
- [Hyprland portal implementation](https://github.com/hyprwm/xdg-desktop-portal-hyprland)
- [Gamescope implementation](https://github.com/ValveSoftware/gamescope)
- [Gamescope portal implementation](https://github.com/evlaV/xdg-desktop-portal-gamescope)
- [Valve desktop FAQ](https://help.steampowered.com/en/faqs/view/671A-4453-E8D2-323C)
- [Steam Input documentation](https://partner.steamgames.com/doc/features/steam_controller)
- [Bazzite implementation and desktop variants](https://github.com/ublue-os/bazzite)
- [Flatpak permission boundaries](https://docs.flatpak.org/en/latest/sandbox-permissions.html)
- [AT-SPI editable-text interface](https://gitlab.gnome.org/GNOME/at-spi2-core/-/blob/main/xml/EditableText.xml)
- [AT-SPI text interface](https://gitlab.gnome.org/GNOME/at-spi2-core/-/blob/main/xml/Text.xml)
- [AT-SPI registry interface](https://gitlab.gnome.org/GNOME/at-spi2-core/-/blob/main/xml/Registry.xml)
- [KDE Clipboard implementation](https://invent.kde.org/plasma/xdg-desktop-portal-kde/-/blob/master/src/clipboard.cpp)
- [Flatpak accessibility-bus setup](https://github.com/flatpak/flatpak/blob/main/common/flatpak-run-dbus.c)
- [Native GTK clipboard API](https://docs.gtk.org/gdk4/class.Clipboard.html)
- [Native Qt clipboard API](https://doc.qt.io/qt-6/qclipboard.html)
- [Underlying global-hotkey platform scope](https://github.com/tauri-apps/global-hotkey)

Recheck backend source and installed versions when implementing. Main-branch
capabilities are not proof a distro's stable package ships those capabilities.
