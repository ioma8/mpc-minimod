#!/bin/sh
# Provisioning applied to the vendor rootfs before systemd starts.
#
# Sourced by the initramfs /init after the AC50 root filesystem is mounted at
# /newroot. Everything here exists because a real board has hardware this
# virtual board does not; each block says which hardware and what breaks
# without it. Nothing is patched inside the MPC binary itself.

# ---------------------------------------------------------------------------
# 1. Skip the control-surface firmware flash.
#
# /usr/bin/launch-ac50 runs az0x-firmware-tool first, which flashes the MCU over
# MIDI and blocks forever waiting for an ALSA port called "Control Surface".
# There is no MCU here, so the app would never be reached. The ulimit and
# MALLOC_ARENA_MAX lines are kept: the app relies on the 1 MiB thread stack.
# ---------------------------------------------------------------------------
cat > /newroot/usr/bin/launch-ac50 <<'LAUNCH'
#!/bin/sh
ulimit -S -s 1024
export MALLOC_ARENA_MAX=1
exec setarch -R -- "/usr/bin/MPC Sample" "$@"
LAUNCH
chmod 755 /newroot/usr/bin/launch-ac50

# ---------------------------------------------------------------------------
# 2. DHCP on the emulated NIC.
#
# QEMU user-mode networking only forwards host ports to an address it leased
# itself, and the vendor image leaves an unknown NIC to connman, which ignores
# it. Without a lease the web front end on :80 is unreachable from the host.
# ---------------------------------------------------------------------------
cat > /newroot/etc/systemd/system/emu-dhcp.service <<'UNIT'
[Unit]
Description=Emulator DHCP lease on the virtual NIC
After=network-pre.target
Before=az0x-webserver.service ac50.service

[Service]
Type=simple
ExecStart=/bin/busybox udhcpc -i enp0s5 -f -p /run/udhcpc.pid
Restart=always

[Install]
WantedBy=multi-user.target
UNIT
mkdir -p /newroot/etc/systemd/system/multi-user.target.wants
ln -sf /etc/systemd/system/emu-dhcp.service \
       /newroot/etc/systemd/system/multi-user.target.wants/emu-dhcp.service

# ---------------------------------------------------------------------------
# 3. Console access.
#
# The serial console is the only input path that works: the app wants a touch
# panel and the MCU, neither of which is emulated. /var/lib lives on tmpfs, so
# ssh host keys are regenerated on every boot and sshd is slow to come up --
# the serial console is the reliable way in.
# ---------------------------------------------------------------------------
mkdir -p /newroot/etc/systemd/system/serial-getty@ttyAMA0.service.d
cat > /newroot/etc/systemd/system/serial-getty@ttyAMA0.service.d/autologin.conf <<'CONF'
[Service]
ExecStart=
ExecStart=-/sbin/agetty --autologin root --noclear %I $TERM
CONF
ln -sf /usr/lib/systemd/system/serial-getty@.service \
       /newroot/etc/systemd/system/serial-getty@ttyAMA0.service

mkdir -p /newroot/etc/ssh/sshd_config.d
cat > /newroot/etc/ssh/sshd_config.d/90-emu.conf <<'CONF'
PermitRootLogin yes
PasswordAuthentication yes
PermitEmptyPasswords yes
CONF

# ---------------------------------------------------------------------------
# 4. ALSA test utilities (aplay/amidi/aconnect), for checking the audio path.
# ---------------------------------------------------------------------------
if [ -d /emu/extra ]; then
    mkdir -p /newroot/usr/local/bin
    cp /emu/extra/* /newroot/usr/local/bin/ 2>/dev/null
    chmod 755 /newroot/usr/local/bin/* 2>/dev/null
    say "installed $(ls /emu/extra | wc -l) audio tools"
fi

say "vendor rootfs provisioned"
