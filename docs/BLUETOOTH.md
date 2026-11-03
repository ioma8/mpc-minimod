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

### 2.3 What it is *not* (as far as the strings show)

Searched explicitly, all zero matches in the binary:

| term | hits | term | hits |
|---|---|---|---|
| `a2dp` / `A2DP` | 0 / 0 | `gatt` / `GATT` | 0 / 0 |
| `avrcp` / `AVRCP` | 0 / 0 | `BleMidi`, `MidiOverBle` | 0 |
| `hfp` / `HFP` | 0 / 0 | `AudioSink`, `MediaControl` | 0 / 0 |

(the single `hsp` hit is the unrelated symbol fragment `hspf`).

So the strings establish **generic device pairing** — adapter power, discovery,
pair/connect/disconnect/remove, passkey and PIN confirmation — and nothing about
audio profiles or BLE MIDI. Whether a paired device is used for MIDI, file
transfer or something else is **not** established here; see §4.

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
2. **What paired devices are for.** The UI is generic pairing. No BLE-MIDI or
   audio-profile strings were found. If the product uses Bluetooth MIDI, the
   profile handling would be in the paired-device path, not in the strings seen.
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
strings -a "/usr/bin/MPC Sample" | grep -aE "DEVICESETUP|PAIRED DEVICES|Pair"
```

Related: `AUTOTRIM-HANDOFF.md` (mod status), `docs/EMULATOR.md` (lab),
`docs/MOD-REFERENCE.md` (the app's MIDI path, which is the rear UART, not BT).
