#!/usr/bin/env python3
"""Capture one audio file per pad from the virtual MPC Sample.

Stock firmware exports a mixdown only, so per-pad audio is obtained by handing the
device a project that contains one pad (`mpc2stems solo-projects`), playing it, and
recording what the app sends to its jacks -- here the lab's `mpclab.pcm` socket,
which is the same signal the Song export would capture.

Each capture pins the project's song to one sequence that contains the pad
(sequence 3 covers 11 of the 12 pads, sequence 4 covers pad 27), so the take is
~9 s and the model can render exactly the same material.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from mpc2stems import xpj  # noqa: E402

SSH_BASE = [
    "ssh",
    "-p",
    "2122",
    "-i",
    "out/keys/id_ed25519",
    "-o",
    "StrictHostKeyChecking=no",
    "-o",
    "UserKnownHostsFile=/dev/null",
    "-o",
    "ConnectTimeout=8",
    "root@127.0.0.1",
]


def sh(cmd: str, *, timeout: int = 60) -> str:
    result = subprocess.run(SSH_BASE + [cmd], capture_output=True, text=True, timeout=timeout)
    if result.returncode != 0:
        raise RuntimeError(f"ssh failed: {cmd}\n{result.stderr[-400:]}")
    return result.stdout


def panel(port: int, name: str) -> None:
    for action in ("press", "release"):
        body = json.dumps({"name": name, "action": action}).encode()
        request = urllib.request.Request(
            f"http://127.0.0.1:{port}/control",
            data=body,
            headers={"Content-Type": "application/json"},
        )
        urllib.request.urlopen(request, timeout=10).read()


def app_pid() -> str:
    try:
        return sh('pidof "MPC Sample"').strip()
    except Exception:
        return ""


def wait_ready(old_pid: str, deadline_s: float = 90.0) -> None:
    """Wait for a *new* app process whose playback stream is running.

    Polling the ALSA status alone is not enough: right after
    ``systemctl restart`` the old process is still alive and its stream is still
    RUNNING, so the take would begin on the previous project and be cut off by
    the restart.  The PID has to change first.
    """
    end = time.monotonic() + deadline_s
    while time.monotonic() < end:
        pid = app_pid()
        if pid and pid != old_pid:
            try:
                status = sh("cat /proc/asound/Loopback/pcm0p/sub0/status")
            except Exception:
                status = ""
            if "RUNNING" in status:
                # The loopback stream is kept alive by the lab's audio bridge, so
                # RUNNING does not mean "the app is ready" -- the app needs its
                # first screen and project load.  The lab itself allows 27 s after
                # a restart; 25 s here is the measured-safe margin.
                time.sleep(25.0)
                return
        time.sleep(1.0)
    raise RuntimeError("the app never came back with a running audio stream")


def pin_song(document: dict, sequence: int) -> None:
    """Reduce the song to one sequence so the take is short and reproducible.

    `songs[0].items[]` is a list of {item.sequenceIndex, item.repeats}; the app
    plays that arrangement, and the model renders the same list.
    """
    data = document["data"]
    data["songs"][0]["items"] = [{"item.sequenceIndex": sequence, "item.repeats": 1}]
    data["currentSequence"] = sequence


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--solo", type=Path, default=Path("out-solo"))
    ap.add_argument("--out", type=Path, default=Path("out-capture"))
    ap.add_argument("--panel-port", type=int, default=8865)
    ap.add_argument("--pcm-port", type=int, default=5657)
    ap.add_argument("--seconds", type=float, default=13.0)
    ap.add_argument("--only", type=str, default="", help="comma-separated pad indices")
    args = ap.parse_args(argv[1:])

    wanted = {int(x) for x in args.only.split(",") if x.strip()} if args.only else None
    projects = sorted(args.solo.glob("*.xpj"))
    if not projects:
        print(f"no solo projects in {args.solo}", file=sys.stderr)
        return 1

    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "projects").mkdir(exist_ok=True)
    audios = args.out / "audio"
    audios.mkdir(exist_ok=True)

    for project in projects:
        pad = int(project.name.split("-")[1])
        if wanted is not None and pad not in wanted:
            continue
        sequence = 4 if pad == 27 else 3
        container = xpj.read_project(project)
        document = container.document
        pin_song(document, sequence)

        local = args.out / "projects" / f"{project.stem}-seq{sequence}.xpj"
        xpj.write_project(local, container, document)

        sh("rm -rf /data/TempProjects/Temp-Project_[ProjectData] /data/TempProjects/Temp-Project.xpj")
        subprocess.run(
            ["scp", "-P", "2122", "-i", "out/keys/id_ed25519",
             "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null",
             "-q", str(local), "root@127.0.0.1:/data/TempProjects/Temp-Project.xpj"],
            check=True, timeout=60,
        )
        data_dir = project.with_name(f"{project.stem}_[ProjectData]")
        subprocess.run(
            ["scp", "-P", "2122", "-i", "out/keys/id_ed25519",
             "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null",
             "-q", "-r", str(data_dir), "root@127.0.0.1:/data/TempProjects/"],
            check=True, timeout=120,
        )
        sh(
            f"mv '/data/TempProjects/{data_dir.name}' "
            "'/data/TempProjects/Temp-Project_[ProjectData]'"
        )
        old_pid = app_pid()
        sh("systemctl restart ac50")
        wait_ready(old_pid)

        # a freshly restarted app is stopped at the sequence start; pressing STOP
        # here would only risk desynchronising it
        panel(args.panel_port, "PLAY")
        target = audios / f"{project.stem}.wav"
        subprocess.run(
            [sys.executable, str(Path(__file__).with_name("ws_pcm.py")),
             str(target), str(args.seconds), str(args.pcm_port)],
            check=True, timeout=120,
        )
        panel(args.panel_port, "STOP")
        shots = args.out / "screens"
        shots.mkdir(exist_ok=True)
        try:
            with urllib.request.urlopen(
                f"http://127.0.0.1:{args.panel_port}/screen.png", timeout=10
            ) as response:
                (shots / f"{project.stem}.png").write_bytes(response.read())
        except Exception as exc:  # a screenshot is a nicety, not the result
            print(f"  (no screenshot: {exc})", file=sys.stderr)
        print(f"pad {pad:2d} -> {target.name}", flush=True)

    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
