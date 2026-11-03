#!/bin/sh
# Pack build/minimod.so into an SD autorun package (build/minimod.zip).
set -eu
cd "$(dirname "$0")"
[ -f build/minimod.so ] || ./build.sh

STAGE=build/pkg
rm -rf "$STAGE"
mkdir -p "$STAGE/autorun"

cat > "$STAGE/autorun/autorun.json" <<'JSON'
{
  "value0": {
    "scripts": [
      {
        "path": "autorun.sh",
        "askQuestion": false,
        "message": "Start minimod?",
        "yesButtonText": "Start",
        "noButtonText": "Cancel"
      }
    ],
    "files": []
  }
}
JSON

cat > "$STAGE/autorun/autorun.sh" <<'SH'
#!/bin/sh
# minimod: load the stock app with the mod preloaded. Nothing on disk changes.
DIR="$(cd "$(dirname "$0")" && pwd)"
STOCK="/usr/bin/MPC Sample"
FW=44507c7a7d3029c411b2e3780ea286ca3e08b7e3ffd7b5047d52f73bf5299d1f

[ "$(dd if="$STOCK" bs=4096 count=1 2>/dev/null | sha256sum | cut -d' ' -f1)" = "$FW" ] || exit 0
cp "$DIR/minimod.so" /run/minimod.so || exit 0
chmod 755 /run/minimod.so

mkdir -p /run/systemd/system/ac50.service.d
printf '[Service]\nEnvironment=LD_PRELOAD=/run/minimod.so\nEnvironment=MINIMOD_DEBUG=0\n' \
    > /run/systemd/system/ac50.service.d/minimod.conf
systemctl daemon-reload
systemctl restart ac50
SH
chmod 755 "$STAGE/autorun/autorun.sh"
cp build/minimod.so "$STAGE/autorun/minimod.so"

# autorun.check = sha256(autorun.json + autorun.sh + salt)
python3 - "$STAGE/autorun" <<'PY'
import hashlib, pathlib, sys
d = pathlib.Path(sys.argv[1])
blob = (d / "autorun.json").read_bytes() + (d / "autorun.sh").read_bytes()
blob += b"f7bed1c9-7e90-4731-9194-c996666b4331"
(d / "autorun.check").write_text(hashlib.sha256(blob).hexdigest())
PY

rm -f build/minimod.zip
(cd "$STAGE" && zip -qr ../../build/minimod.zip autorun)
echo "built build/minimod.zip"
