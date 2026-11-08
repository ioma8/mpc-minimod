# Device notes — hardware, Bluetooth, WiFi, hidden settings

Facts about the MPC Sample as a Linux device, from the shipped images
(`part1` boot, `part2` rootfs), the live lab VM and the app binary. Product
`AC50`, firmware 1.3.0.12 (`/usr/bin/MPC Sample`, sha256 `b6584cba…`).

Everything here is measured unless marked otherwise. The mechanisms the mod
itself uses are in [`MECHANISMS.md`](MECHANISMS.md); this file is the rest of the
platform.

---

## 1. Hardware and OS

| item | value |
|---|---|
| board | inMusic `AC50`, product string `AC50` |
| display | 320 × 240 |
| panel | 45 physical controls (`PAD 1…16`, `SHIFT`, `DATA`, `K1..K3`, `FADER`, transport, `SCREEN 1..3`); control-surface UART, exposed as an ALSA rawmidi device |
| connectivity chip | Broadcom **BCM43456** combo: Bluetooth **BCM4345C5** (UART) + WiFi **BCM43456 FullMAC** (SDIO) |
| kernel | `6.12.60-imb-2025-12-03-rt13` (PREEMPT_RT), Bluetooth and brcmfmac modules ship |
| launcher | `/usr/bin/launch-ac50` → `setarch -R -- "/usr/bin/MPC Sample"`, 1 MiB stack, `MALLOC_ARENA_MAX=1` |
| network managers | ConnMan (WiFi/Ethernet), BlueZ (Bluetooth), both enabled; D-Bus only |
| project inserts | four insert slots per channel, one instance of a plugin per project; edited only by the rack addin (claimed; not verified here) |
| update image | `AZ0x` container: `BOOT` raw, other partitions xz; partitions `part1`/`part2`/`parr` |

The device tree carries both combo bindings as `compatible` entries:
`,brcm,bcm4345c5` (Bluetooth) and `,brcm,bcm43456-fmac` (WiFi). Firmware in the
rootfs: `/lib/firmware/brcm/BCM4345C5.hcd` (BT patchram),
`brcmfmac43456-sdio.bin` and `-sdio.txt` (WiFi firmware + NVRAM).

---

## 2. Bluetooth

**The device has Bluetooth**: a full pairing UI inside the app, BlueZ enabled,
and the Broadcom protocol modules for UART-attached HCI. It is **not available in
the lab emulator**, which exposes no controller.

### Kernel and userspace

```
/lib/modules/6.12.60-imb-2025-12-03-rt13/kernel/drivers/bluetooth/
    btbcm.ko      Broadcom protocol (patchram, baudrate)
    hci_uart.ko   HCI over UART (H4)
```

Only those two — no USB Bluetooth, no `hci_vhci`. The BT UART is attached to the
HCI stack with `btattach` (Broadcom: `btmon`, `bluemoon`), not by kernel probing.

```
/usr/lib/systemd/system/bluetooth.service           enabled, inactive in the lab
/usr/lib/systemd/system/bluetooth.service.d/        CapabilityBoundingSet=CAP_NET_RAW
/usr/lib/systemd/system/bluetooth.target
/usr/libexec/bluetooth/bluetoothd
/etc/bluetooth/{main,input,network}.conf
```

`bluetooth.service` carries `ConditionPathIsDirectory=/sys/class/bluetooth`, so
it stays inactive until a controller exists. `/etc/bluetooth/main.conf` sets
`SecureConnections = false`.

### Application side

Classes found in the binary: `LinuxBluetooth`, `LinuxBluetoothAdapter`,
`LinuxBluetoothDevice`, `LinuxBluetoothConfirmation`,
`BluetoothConfirmationRequest`, `BluetoothListener`, `TouchBluetoothButton`,
`TouchBluetoothImageComponent`, `BluetoothItem`,
`TouchPreferences::BluetoothTab`.

`BluetoothTab` methods describe the flow: `OnBluetoothToggle`,
`CreateUIForDevice`, `DisplayPasskey`, `DisplayPinCode`, `RequestPinCode`. It
talks to BlueZ over D-Bus through an internal `DBusClient` (no libbluetooth
linkage implied):

```
org.bluez.Adapter1     SetPowered
org.bluez.Device1      Pair, Connect, Disconnect, Remove, CancelPairing
org.bluez.Agent1       its own pairing agent
org.bluez.AgentManager1  registers that agent
```

There is also a developer diagnostics tab that is compiled out of the call graph
entirely; even a working touchscreen would not show it. Nothing string-level
gates the user-facing Bluetooth tab.

### Open questions

* Which UART instance the controller sits on (the strings alone do not say).
* Whether Bluetooth is enabled per SKU (no gate found, but no controller was
  exercised).

---

## 3. WiFi

**The device has WiFi**: BCM43456 FullMAC on SDIO (`brcmfmac`), ConnMan +
wpa_supplicant in userspace, and a full `WiFiTab` in the app. It is **not
available in the lab emulator**, which exposes no wireless interface.

### Kernel and userspace

```
brcmfmac/brcmfmac.ko, cyw/, bca/, wcc/ variants, brcmutil.ko, cfg80211.ko
```

```
connman.service            enabled, active
wpa_supplicant.service     disabled (present, plus @/nl80211/wired templates)
systemd-networkd.service
```

D-Bus policy grants exactly two names: `net.connman` (with
`.Agent`/`.Counter`/`.Notification`) and `fi.w1.wpa_supplicant1`.

`/etc/connman/main.conf`:

```ini
[General]
OnlineCheckIPv4URL = http://ipv4.connman.inmusicbrands.com/
NetworkInterfaceBlacklist = vmnet,vboxnet,virbr,ifb,ve-,vb-,enp
```

The inMusic-branded connectivity-check URL is the strongest sign WiFi is a
shipping feature rather than leftover code.

### Application side

Classes: `LinuxNetworking`, `LinuxNetworking::AccessPoint`
(Connect/Disconnect/Remove), `LinuxNetworking::CheckAgentRegistration`,
`LinuxNetworkingInput`, `NetworkUnavailablePage`. It registers its own D-Bus agent
at `/com/inmusicbrands/wifi_agent` and talks to ConnMan (`net.connman`,
`net.connman.Agent`, handling `Agent.Error.{Canceled,Rejected,LaunchBrowser}`) —
the app never touches `wpa_supplicant` directly.

`TouchPreferences` tabs (from RTTI): `InfoTab`, `WiFiTab`, `EthernetTab`,
`BluetoothTab`, `AudioTab`, `LegalTab`, `BackupRestoreTab`,
`SoftwareUnlockTab`, `CrashReportsTab`, `NotImplementedTab`. `WiFiTab` and
`EthernetTab` are ordinary siblings of `BluetoothTab`; no dev-only gate.

### Why it cannot be exercised in the lab

The guest has no wireless interface:

```
lo sit0 enp0s8 enu1 usb0      <- no wlan0
rfkill list                   (empty)
/var/lib/connman/             ethernet_…_cable only, no wifi_* entry
```

ConnMan is active only because the lab's own connectivity is the USB ethernet
gadget. QEMU provides no SDIO WiFi device, so `brcmfmac` never binds.

### Open questions

* Which SDIO instance, and whether the shipped device tree enables it
  unconditionally (the binding is present; the node's `reg` was not established).
* Antenna/regulatory: `regulatory.db` ships; where the domain is set (ConnMan,
  kernel `cfg80211`, or NVRAM) was not traced.
* Whether WiFi is enabled per SKU — the app has both `WiFiTab` and `EthernetTab`,
  and ConnMan blacklists `enp*`, which hints at variants.
* The mod touches no networking.

---

## 4. Preferences — the hidden touch UI

The settings UI exists and is complete, but **no panel control opens it**.

* The lab ships a map of every button and SHIFT combination (56 entries, built by
  pressing them all on stock firmware): none opens a settings/preferences screen.
* `panel/controls.json` has 45 physical controls; `SCREEN 1–3` are the
  *sample-page* tabs (Trim / Tune / Filter), not Preferences.
* `TouchPreferences` is a Component with its own tab strip, reachable only by
  the digitizer. The app ignores injected touch: the lab states it
  (`panel/server.py`: *"the app ignores tablet/touch input"*), and a QMP
  absolute-pointer tap over `virtio-tablet` produced 0 px change and no state
  change. The app opens `/dev/input/event0..2` (tablet, keyboard, gpio-keys) but
  acts on none of the tablet events.
* The settings table is shared MPC code: it includes rows that cannot apply here
  (`WALL POWER DIMMING / BRIGHTNESS`, `FOOTSWITCH 1/2`, `TOUCH STRIP BEHAVIOUR`,
  `CLIP WORKFLOW / MAIN MODE`, `STEP SEQUENCER BUTTON MODES`, `TRACK MUTE STATE
  PER SEQUENCER`) alongside ones that do (MIDI INPUT/OUTPUT, PADS, DISPLAY
  RESOLUTION, DATE & TIME). Which tabs a model builds is decided in code, not by
  a string.

### What is known about wiring it up

`TouchPreferencesOverlay`: RTTI `0x070f1470`, primary vtable `0x070f14e8`,
constructor `FUN_03804fe0`; a factory path in `FUN_03519cb0` allocates `0x5e8`
bytes, sets the title `Preferences` and calls the constructor with the app's
existing service/model objects. The older `FUN_03337be0` note describes a
*different* component and must not be used as the overlay constructor.

Layout and navigation: sidebar 114 px, content `W-114`, footer 64 px wide at the
bottom right, all `H-30`. Tab switch `FUN_03336d10` (clears the content
container, validates the index, calls the tab's `+0x168`, adds the component);
Apply `FUN_03336c70`; close `FUN_03336c10` / `FUN_03336c20`; host removal
`FUN_02f8ee00` walks parents and closes through the native screen host.

**A possible way in.** The remote addin reaches a device's touchscreen by
writing `struct input_event` records into the touchscreen's evdev node (see
[`ADDIN-ECOSYSTEM.md`](ADDIN-ECOSYSTEM.md#remote--drm-capture-evdev-touch-injection-mcp)).
If the Sample's panel accepts injected events, that is a hardware route to this
overlay; it is untested here.

**Activation is not implemented.** The constructor and factory identify a
construction path, not a safe runtime call site: ownership of the constructed
overlay, which code presents it, whether that path is live in the emulator, and
how to avoid a second instance or break native close behaviour are all unknown.
Passing arbitrary/null dependency pointers is unsafe — the overlay registers
observers and dereferences the dependency graph.

The category list the factory builds is: Info, WiFi, Ethernet, Sequencer,
MIDI/Sync, Hardware, AutoLoadSave, Other, RecordingExport, Project, Crash
Reports. Earlier class-name inventories (WiFiTab, EthernetTab, BluetoothTab,
AudioTab, InfoTab, LegalTab, BackupRestoreTab, SoftwareUnlockTab,
CrashReportsTab, NotImplementedTab) came from an older component and do not
prove which tabs the current overlay builds.

A compact adaptation would need: a tab selector operated by DATA, the selected
tab given the full width below a small header, a viewport for overflowing
content, and native tab activation plus Apply/close retained. That is a design,
not code.

---

## 5. What the lab does not have

| capability | lab | consequence |
|---|---|---|
| Bluetooth controller | absent | `BluetoothTab` cannot be exercised; `bluetooth.service` stays inactive |
| WiFi interface | absent | `WiFiTab` cannot be exercised; ConnMan has no WiFi technology |
| touch digitizer | ignored by the app | Preferences cannot be opened in the emulator at all |
| audio capture accuracy | loopback only | per-pad ground truth needs the app's own export (see converter docs) |
| real-time audio | QEMU + bridge | timing-sensitive claims are measured, not assumed |

Emulator results are recorded as emulator results. Only the 16-Levels pitch shift
and external MIDI chromatic playback are hardware-verified.

---

## 6. How these facts were obtained

```sh
# boot image: device-tree bindings, kernel modules
strings -a firmware/extracted/part1 | grep -aiE "brcm,bcm4345|brcm,bcm43456|sdio|bluetooth"

# rootfs: userspace, policies, configs
strings -a firmware/extracted/part2 | grep -aiE "net\.connman|fi\.w1\.wpa_supplicant1|brcmfmac|bluez"

# app surface
strings -a "/usr/bin/MPC Sample" | grep -aiE "bluetooth|org\.bluez|LinuxBluetooth|inmusicbrands"
strings -a -t x "/usr/bin/MPC Sample" | grep -aE "TouchPreferences[0-9]+[A-Za-z]+Tab"

# live VM
python3 lab.py ssh 'ls /sys/class/bluetooth; rfkill list; ls /sys/class/net'
python3 lab.py ssh 'systemctl is-enabled connman wpa_supplicant bluetooth; cat /etc/connman/main.conf'
```

The Preferences findings came from Ghidra (constructor, vtable, factory, layout
and close routines), the panel map for "no button opens it", and a controlled
QMP touch injection that changed nothing.
