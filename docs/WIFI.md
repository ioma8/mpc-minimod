# WiFi on the MPC Sample

Findings about the device's WiFi support, from the shipped firmware images
(`firmware/extracted/part1` boot, `part2` rootfs), the live lab VM and the
application binary. Counterpart to `docs/BLUETOOTH.md`.

**Verdict: the device has WiFi** — a Broadcom BCM43456 (SDIO, `brcmfmac`), with
ConnMan + wpa_supplicant in userspace and a full `WiFiTab` in the app. It is
**not available in the lab emulator**, which exposes no wireless interface.

---

## 1. Hardware

### 1.1 The chip

Same combo part as the Bluetooth side (`docs/BLUETOOTH.md` §1). The boot image
carries the FullMAC binding and the SDIO stack:

```
,brcm,bcm43456-fmac        ← device-tree compatible
/drivers/mmc/core/sdio.c   ← SDIO bus driver
/drivers/mmc/core/sdio_cis.c
/drivers/mmc/core/sdio_irq.c
```

Firmware in the root filesystem:

```
/lib/firmware/brcm/brcmfmac43456-sdio.bin    WiFi firmware
/lib/firmware/brcm/brcmfmac43456-sdio.txt    NVRAM / board parameters
```

So the WiFi side is a **BCM43456 FullMAC on SDIO**, driven by `brcmfmac`.

### 1.2 Kernel drivers

```
/lib/modules/6.12.60-imb-2025-12-03-rt13/kernel/drivers/net/wireless/broadcom/brcm80211/
    brcmfmac/brcmfmac.ko
    brcmfmac/cyw/brcmfmac-cyw.ko
    brcmfmac/bca/brcmfmac-bca.ko
    brcmfmac/wcc/brcmfmac-wcc.ko
    brcmutil/brcmutil.ko
/lib/modules/…/kernel/net/wireless/cfg80211.ko
```

(the `cyw`/`bca`/`wcc` modules are the per-vendor firmware-variant shims; only one
is normally loaded.)

---

## 2. Userspace

ConnMan is the network manager, wpa_supplicant the supplicant:

```
connman.service            enabled, active
wpa_supplicant.service     disabled (but present, plus the @/nl80211/wired templates)
systemd-networkd.service
```

D-Bus policy in the rootfs grants exactly these two:

```
<allow own="net.connman"/>                 net.connman.Agent / .Counter / .Notification
<allow own="fi.w1.wpa_supplicant1"/>       fi.w1.wpa_supplicant1
```

`/etc/connman/main.conf`:

```ini
[General]
OnlineCheckIPv4URL = http://ipv4.connman.inmusicbrands.com/
NetworkInterfaceBlacklist = vmnet,vboxnet,virbr,ifb,ve-,vb-,enp
```

The **inMusic-branded connectivity-check URL** is the strongest sign that WiFi is
a shipping product feature rather than leftover code: that endpoint only makes
sense for a device that comes up on arbitrary networks.

---

## 3. The application

### 3.1 Classes

```
LinuxNetworking                      DBusClient-based network manager
LinuxNetworking::AccessPoint         Connect / Disconnect / Remove
LinuxNetworking::CheckAgentRegistration
LinuxNetworkingInput                 (anonymous namespace)
NetworkUnavailablePage               (anonymous namespace)
```

Its own D-Bus agent object, registered alongside the Bluetooth one:

```
/com/inmusicbrands/wifi_agent
```

It talks to ConnMan over D-Bus (`net.connman`, `net.connman.Agent`, with
`Agent.Error.{Canceled,Rejected,LaunchBrowser}` handled), i.e. the same pattern as
the Bluetooth stack — the app never touches `wpa_supplicant` directly.

### 3.2 The Preferences tabs (complete list, from RTTI)

```
TouchPreferences11EthernetTab        TouchPreferences12BluetoothTab
TouchPreferences15CrashReportsTab    TouchPreferences16BackupRestoreTab
TouchPreferences17NotImplementedTab  TouchPreferences17SoftwareUnlockTab
TouchPreferences7InfoTab             TouchPreferences7WiFiTab
TouchPreferences8AudioTab            TouchPreferences8LegalTab
```

`WiFiTab` and `EthernetTab` are ordinary siblings of `BluetoothTab` — there is no
separate "dev-only" gate on them, unlike the developer diagnostics tab analysed in
`docs/BLUETOOTH.md` §2.5.

### 3.3 UI text

```
"…can detect and connect with a wireless network."
```

(the Bluetooth counterpart reads "…can detect and communicate with other nearby
Bluetooth devices", so the two screens are deliberately parallel.)

---

## 4. Status in the lab emulator: not available

The guest has **no wireless interface**:

```
lo  sit0  enp0s8  enu1  usb0        ← no wlan0
```

`connman` is `active` only because the lab's own connectivity uses the USB
ethernet gadget (`usb0`/`enu1`), and `wpa_supplicant` is `disabled`. Measured:

```
/var/lib/connman/            ethernet_d676753b85a1_cable   settings
                             ^ only an ethernet technology, no wifi_* entry
/sys/class/net               enp0s8 enu1 lo sit0 usb0      (no wlan0)
rfkill list                  (empty)
/lib/firmware/regulatory.db  present
```

With no `wlan0` there is no `net.connman` WiFi technology to enumerate, so the
app's `WiFiTab` would show nothing — exactly like the Bluetooth tab.

The lab does not emulate a WiFi chip (the `brcmfmac` driver and firmware are
present for the real hardware, but QEMU provides no SDIO WiFi device), so this
cannot be exercised in the emulator as it stands.

---

## 5. Open questions

1. **Which SDIO instance** the chip sits on, and whether the shipped device tree
   enables it unconditionally (the binding is present; the node's `reg` was not
   established here).
2. **Antenna / regulatory**: `regulatory.db` and `regulatory.db.p7s` ship in
   `/lib/firmware`, so the device expects a regulatory domain; where the domain
   is set (connman, kernel `cfg80211`, or the NVRAM `.txt`) was not traced.
3. **Whether WiFi is enabled per SKU** — the app has both `WiFiTab` and
   `EthernetTab`, and ConnMan blacklists `enp*` interfaces, which hints at
   variants; nothing found that gates the tab at the string level.
4. Interaction with the mod: nothing in the mod touches networking.

---

## 6. How each finding was obtained

```sh
# boot image: WiFi device-tree binding + SDIO stack
strings -a firmware/extracted/part1 | grep -aiE "brcm,bcm43456|sdio|wlan"

# root filesystem image: userspace, policy, config
strings -a firmware/extracted/part2 | grep -aiE "net\.connman|fi\.w1\.wpa_supplicant1|brcmfmac"

# live VM: drivers, services, interfaces, config
lab.py ssh 'find /lib/modules/*/kernel/drivers/net/wireless -name "*.ko"'
lab.py ssh 'systemctl is-enabled connman; systemctl is-active connman; ls /sys/class/net'
lab.py ssh 'cat /etc/connman/main.conf; ls /lib/firmware/brcm/'

# app surface
strings -a firmware/extracted/part2 | grep -aoE "TouchPreferences[0-9]+[A-Za-z]+Tab"
strings -a firmware/extracted/part2 | grep -aE "LinuxNetworking|wifi_agent|wireless network"
```

Related: `docs/BLUETOOTH.md` (the combo chip, the Bluetooth tab, and why no tab can
be reached in the emulator).
