# /// script
# requires-python = ">=3.10"
# ///
"""Resolve the SDK source directory Cargo selected for this mod."""

import json
import subprocess
import sys
from pathlib import Path

metadata = json.loads(
    subprocess.run(
        [
            "cargo",
            "metadata",
            "--format-version",
            "1",
            "--manifest-path",
            sys.argv[1],
        ],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
)
package = next(
    (item for item in metadata["packages"] if item["name"] == "mpc-sample-sdk"),
    None,
)
if package is None:
    sys.exit("mpc-sample-sdk is not a dependency")
print(Path(package["manifest_path"]).parent)
