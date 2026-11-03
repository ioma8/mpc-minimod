# Bluetooth on the MPC Sample

Findings about the device's Bluetooth support, gathered from the shipped firmware
images (`firmware/extracted/part1`, `part2`), the live lab VM, and the application
binary (`/usr/bin/MPC Sample`, sha256 `b6584cba…3b508`, firmware 1.3.0.12).

**Verdict: the device has Bluetooth** — a Broadcom BCM43456 combo (BT part
BCM4345C5), BlueZ enabled, and a full pairing UI inside the app. It is **not
available in the lab emulator**, which exposes no Bluetooth controller.

Everything below is measured; open questions are listed at the end.

---

## 1. Hardware

### 1.1 The chip

The stock boot image contains both device-tree bindings of a combo part, as
entries of a `compatible` list (the raw strings include the leading comma):

```
,brcm,bcm4345c5        ← Bluetooth
,brcm,bcm43456-fmac    ← WiFi (FullMAC)
```

and the matching firmware files in the root filesystem:

```
/lib/firmware/brcm/BCM4345C5.hcd            Bluetooth patchram
/lib/firmware/brcm/brcmfmac43456-sdio.bin   WiFi firmware
/lib/firmware/brcm/brcmfmac43456-sdio.txt   WiFi NVRAM
```

So the Bluetooth controller is a **Broadcom BCM4345C5**, part of a BCM43456
combo, attached over UART (see the drivers below). The exact UART instance is not
established from the strings alone.

### 1.2 Kernel drivers shipped

```
/lib/modules/6.12.60-imb-2025-12-03-rt13/kernel/drivers/bluetooth/
    btbcm.ko      Broadcom protocol (patchram download, baudrate setup)
    hci_uart.ko   HCI over UART (H4)
```

Only those two — no USB Bluetooth, no `hci_vhci`. The firmware is built for the
real chip, not for a virtual controller.

### 1.3 Userspace (BlueZ)

Installed and **enabled** on the device:

```
/usr/lib/systemd/system/bluetooth.service        (enabled, inactive in the lab)
/usr/lib/systemd/system/bluetooth.service.d/     CapabilityBoundingSet=CAP_NET_RAW
/usr/lib/systemd/system/bluetooth.target
/usr/libexec/bluetooth/bluetoothd
/usr/bin/btattach, /usr/bin/btmon, /usr/bin/bluemoon
/etc/bluetooth/{main,input,network}.conf
```

`/etc/bluetooth/main.conf`:

```
[General]
SecureConnections = false
```

`btattach` + `bluemoon` are the Broadcom-specific tools: the daemon is started by
attaching the BT UART to the HCI stack, rather than by the kernel probing a
device. `bluetooth.service` carries
`ConditionPathIsDirectory=/sys/class/bluetooth`, so it stays inactive until a
controller exists.

---

## 2. The application

The app ships a complete Bluetooth client. Symbols extracted from the binary:

```
LinuxBluetooth              LinuxBluetoothAdapter        LinuxBluetoothDevice
LinuxBluetoothConfirmation  BluetoothConfirmationRequest BluetoothListener
TouchBluetoothButton        TouchBluetoothImageComponent BluetoothItem
TouchPreferences::BluetoothTab
```

`TouchPreferences::BluetoothTab` is the settings screen. Its methods describe the
whole flow:

```
OnBluetoothToggle        power the adapter on/off
CreateUIForDevice        build one row per discovered device
DisplayPasskey           show a numeric passkey
DisplayPinCode           show a PIN
RequestPinCode           ask the user for a PIN
```

### 2.1 How it talks to BlueZ

D-Bus, through an internal `DBusClient` (no libbluetooth linkage implied by the
symbols). Interfaces it uses:

```
org.bluez.Adapter1          SetPowered
org.bluez.Device1           Pair, Connect, Disconnect, Remove, CancelPairing
org.bluez.Agent1            its own pairing agent
org.bluez.AgentManager1     registers that agent
```

Error paths handled explicitly: `org.bluez.Error.AlreadyConnected`,
`…Busy`, `…PairingNotSupported`, `…Rejected`.

Its agent object path is:

```
/com/inmusicbrands/bluetooth_agent
```

(alongside `/com/inmusicbrands/wifi_agent` for WiFi).

### 2.2 User-facing strings

```
BLUETOOTH                     BLUETOOTH IS OFF
Bluetooth Button              Bluetooth failed to connect
DEVICESETUP                   PAIRED DEVICES
PAIR                          Paired / Pairing / Pairable
Devices
```

The help text is split across two literals in the binary —

```
"When Bluetooth is on, your "
" can detect and communicate with other nearby Bluetooth devices."
```

— and reads as one sentence at runtime: *"When Bluetooth is on, your MPC Sample
can detect and communicate with other nearby Bluetooth devices."* That is the
clearest statement of intent: pairing with nearby devices from the Preferences
screen.

### 2.3 What the Bluetooth screen is for — decoded from its service UUIDs

Immediately after the status strings the binary holds a list of Bluetooth service
UUIDs. They are decisive:

| UUID | profile |
|---|---|
| `0000110a-0000-1000-8000-00805f9b34fb` | **A2DP Source** |
| `0000110b-0000-1000-8000-00805f9b34fb` | **A2DP Sink** |
| `00001124-0000-1000-8000-00805f9b34fb` | **HID** (Human Interface Device) |
| `00001812-0000-1000-8000-00805f9b34fb` | **HID over GATT** (HOGP) |
| `03b80e5a-ede8-4b33-a751-6ce34ec4c700` | **Apple BLE-MIDI** |

and the help text next to them says it plainly:

```
"The currently supported peripherals are MIDI devices and QWERTY keyboards."
```

So Bluetooth is for **wireless MIDI controllers (BLE-MIDI) and Bluetooth
keyboards (HID/HOGP)**, with the A2DP profiles also enumerated. Earlier revisions
of this document said the purpose was "not established" because a search for the
*literal* strings `GATT`/`BLE`/`BleMidi` returned nothing — that search was
misleading: the app identifies profiles by **UUID**, not by name. Corrected here.

### 2.4 Are the Bluetooth options hidden?

**No — the UI is compiled in and user-facing.** The complete screen is present:
status (`BLUETOOTH IS OFF`), two lists (`PAIRED DEVICES`, `AVAILABLE DEVICES`),
actions (`CONNECT`, `REMOVE`, `PAIR`), states (`Pairing`, `Not Connected`), the
PIN flows (`requires a PIN to connect.`, "Please enter the following code on your
device and then press the Enter/Return key:") and the error path
(`Bluetooth failed to connect`, `Failed to connect to … Please check your device
and try again.`). None of that is developer-facing text.

Two things that *could* look like gating, both resolved:

* the string `"This tab only exists in non-release builds"` is **not Bluetooth**.
  Its constructor is `FUN_03827f20` (1300 bytes, Ghidra did not auto-create it;
  `create_function` was needed). Decompiled, it builds a **developer diagnostics
  tab**: an info label carrying that text, a `Custom tags` header with a
  `Refresh tags` button, a `CRASH REASON` pane, and a **`Crash application`**
  button (`"Crash Button"` / `"Crash application"`). It is a dev-only crash/tags
  tool, so there is no Bluetooth gate there and nothing to "ungate" for Bluetooth;
* `Embedded::UnsupportedDeviceDetector` exists (`…UnsupportedDeviceDetector17UnsupportedDevice…`)
  and can disable features per device model, but every message reachable from it
  is about project compatibility ("project uses unsupported number of …") and
  none concerns Bluetooth.

**Conclusion:** the Bluetooth screen exists in the shipped binary as normal
production UI; nothing in the strings disables it. Whether it is *reachable* on a
given unit depends on the Preferences tab being built and on a Bluetooth adapter
being present (`BLUETOOTH IS OFF` is what it shows when there is none — which is
exactly the emulator's state).

**It cannot be seen in the emulator at all**, for a reason unrelated to gating:
the app **ignores tablet/touch input**, so the Preferences screen cannot be
opened. The lab documents this itself in `panel/server.py:414` — *"Screen capture
through QEMU's QMP socket (the app ignores tablet/touch input)"* — and it was
verified by injecting an absolute pointer tap over QMP `input-send-event` onto
the `virtio-tablet`: the screen changed by 0 px. The app opens
`/dev/input/event0..2` (tablet, keyboard, gpio-keys) but does not act on the
tablet's pointer events; its touchscreen path is not emulated.

Three ways to actually see the tab, in order of cost:

1. **Open Preferences programmatically from the mod** — call the entry that
   raises the Preferences screen once the UI is up, then screenshot the tab
   strip. Smallest change, and it answers whether the Bluetooth tab is in the
   list. Not yet identified: the candidates that reference the `"Preferences"`
   string are `FUN_01d0f5b0` (a mode-name lookup switch, returns strings like
   "Main Mode" — not it) and `FUN_02f85de0` (1272 lines, no Bluetooth reference —
   also not the tab list).
2. **Make the emulator deliver touch** — the app reads a touch controller the lab
   does not emulate; virtio-tablet is ignored. A lab-side device change, much
   bigger.
3. **Check on real hardware** — if the tab is missing there too, the gate is a
   per-model feature flag in the tab list, which is where the search would then
   focus.

### 2.5 Enabling the developer diagnostics tab (analysis only, not implemented)

The tab is **compiled out of the call graph**, not gated at runtime. Proven three
independent ways (Ghidra VA = file offset + 0x100000):

| check | result |
|---|---|
| direct `BL` callers of `0x3827f20` | **0** (scan validated: the same scan finds 27 387 callers for the `juce::String` ctor at `0x10535c0`) |
| relocated pointers to `0x3827f20` in all 490 322 `R_AARCH64_RELATIVE` relocations | **0** |
| functions called from inside the tab-code region `0x381e000–0x3824000` | the dev ctor is absent, while sibling tab factories are present (`FUN_03820220` at `0x3820220`, called twice) |

So in this release build nothing allocates or constructs it; the class body
survives only because the linker kept it. **There is no flag to flip.**

A live tab factory shows the pattern that the missing one would have had
(`FUN_03820220`):

```c
pvVar6 = operator_new(0x328);      // allocate the tab object
FUN_01026b10(auStack..., ...);     // ctor
```

To enable the dev tab, the mod would have to reproduce its factory:

1. **allocate** — the ctor writes `param_1[0x92]`, i.e. **offset 0x490**, so the
   object must be at least 0x498 bytes (siblings allocate exact sizes, e.g.
   `0x328`; over-allocating is safe for `operator_new`);
2. **construct** — call `FUN_03827f20(obj)` on the UI thread; it builds the info
   label, the `Custom tags` section with `Refresh tags`, the `CRASH REASON` pane,
   the `Crash application` button and the `Set User ID` button, adding children
   to the container at `obj + 0x98` via `FUN_03832480`;
3. **attach** — insert the object into the Preferences tab strip. *This is the
   unsolved step*: the strip's owner and its add-tab virtual have not been
   identified. The path to them is the live factories in `0x381e000–0x3824000`
   (they call `operator_new` + ctor and then register), or the user of the
   `BLUETOOTH` title setter `FUN_03583900` through its vtable;
4. **select** — the tab must also be made current. This one is not optional in
   the lab: the app ignores touch input (§2.4), so a tab that is merely added
   can never be clicked into view. Showing it means calling the strip's
   *set current tab* path with its index.

Cautions if this is ever implemented:

* the tab contains a **`Crash application`** button and a `Set User ID` action —
  it must live behind `MINIMOD_DEBUG` and never ship;
* `Custom tags` / `Refresh tags` touch the app's tags database, so constructing
  it on a user's unit risks mutating real data;
* it is a developer diagnostics tool, unrelated to Bluetooth despite sitting in
  the same string pool.

---

## 3. Why Bluetooth does not work in the lab emulator

The lab VM (kernel `6.12.60-mpclab`) has the Bluetooth stack compiled in:

```
Bluetooth: Core ver 2.22
NET: Registered PF_BLUETOOTH protocol family
Bluetooth: HCI device and connection manager initialized
Bluetooth: HCI socket layer initialized
Bluetooth: L2CAP socket layer initialized
Bluetooth: SCO socket layer initialized
Bluetooth: HCI UART driver ver 2.3
Bluetooth: HCI UART protocol H4 registered
```

…but there is **no controller**:

* `/sys/class/bluetooth` is empty,
* `rfkill list` prints nothing,
* `bluetooth.service` is `enabled` but `inactive` (its
  `ConditionPathIsDirectory=/sys/class/bluetooth` fails),
* there is no `/dev/vhci`, and no `hci_vhci.ko` exists to provide one.

The reason is in `lab.py`: the device tree is generated from QEMU's `virt`
machine and then patched by `patch_dtb()` with only what the app reads. There is
no Bluetooth/UART node, so no HCI device is ever created. `/dev/ttyS0..3` exist
as plain serial ports with nothing attached.

**To make Bluetooth testable in the lab** one of these is needed:

1. build/load `hci_vhci` (not shipped; would need a kernel rebuild) and attach a
   host-side HCI — the app's UI would then see an adapter;
2. a `virtio-serial` port wired to `btattach`, with a host process emulating an
   HCI controller;
3. only then does `bluetoothd` start and the app's `BluetoothTab` leave the
   "BLUETOOTH IS OFF" state.

Until then the Bluetooth screen can only be inspected statically, not exercised.

---

## 4. Open questions

1. **Which UART** carries the BT HCI on the real device, and is it exposed in the
   shipped device tree (the compatible string is present, the node's `reg` is not
   established here).
2. **Whether the BLE-MIDI path feeds the sequencer/pads the same way the rear
   UART MIDI does.** The BLE-MIDI UUID is present (`03b80e5a-…`) and the screen
   says MIDI devices are supported, but the routing from a paired BLE-MIDI device
   into the app's MIDI inputs was not traced.
3. **Whether `bluetoothd` is started at boot on the device** — the unit is
   `enabled`, but it needs the controller to appear first; the start order
   between `btattach`/`bluemoon` and `bluetooth.service` was not inspected.
4. Whether any of this is reachable from the mod: nothing in the mod touches
   Bluetooth today; its MIDI support uses the **rear UART** node
   (`/dev/snd/midiC*D0`), not Bluetooth.

---

## 5. How each finding was obtained

```sh
# device-tree bindings inside the stock boot partition (ext2 image)
strings -a firmware/extracted/part1 | grep -iE "brcm,bcm4345|brcm,bcm43456"

# firmware + drivers (live VM rootfs == part2)
lab.py ssh 'ls /lib/firmware/brcm/'
lab.py ssh 'ls /lib/modules/*/kernel/drivers/bluetooth/'

# BlueZ presence and state
lab.py ssh 'systemctl cat bluetooth.service; systemctl is-enabled bluetooth.service;
            systemctl is-active bluetooth.service; ls /etc/bluetooth/'

# kernel side (stack loads, no controller)
lab.py ssh 'dmesg | grep -iE "bluetooth|hci"; ls /sys/class/bluetooth; rfkill list'

# device tree in the lab VM (no BT node)
lab.py ssh 'ls /proc/device-tree/; find /proc/device-tree -name "*bluetooth*"'

# application symbols and UI strings
strings -a "/usr/bin/MPC Sample" | grep -iE "bluetooth|org\.bluez|inmusicbrands"

# the Bluetooth screen's string block, with offsets (the UUIDs live here)
strings -a -t x "/usr/bin/MPC Sample" | sed -n '/BLUETOOTH IS OFF/,+14p'

# is anything gating it?
strings -a "/usr/bin/MPC Sample" | grep -aiE "non-release|hasBluetooth|bluetoothEnabled|disabled on"

# what references the dev-only-tab string (Ghidra VA = file offset + 0x100000)
curl -s "http://127.0.0.1:8089/get_xrefs_to?address=0x47198d0&program=knob.bin"
```

Related: `AUTOTRIM-HANDOFF.md` (mod status), `docs/EMULATOR.md` (lab),
`docs/MOD-REFERENCE.md` (the app's MIDI path, which is the rear UART, not BT).
