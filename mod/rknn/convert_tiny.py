# /// script
# requires-python = ">=3.8,<3.9"
# dependencies = [
#   "rknn-toolkit2 @ https://raw.githubusercontent.com/airockchip/rknn-toolkit2/v2.5.0/rknn-toolkit2/packages/x86_64/rknn_toolkit2-2.5.0-cp38-cp38-manylinux_2_17_x86_64.manylinux2014_x86_64.whl",
#   "protobuf>=4.21.6,<=4.25.4",
#   "setuptools<81",
#   "psutil>=5.9.0",
#   "ruamel.yaml>=0.17.4",
#   "scipy>=1.5.4",
#   "tqdm>=4.64.0",
#   "opencv-python>=4.5.5.64",
#   "fast-histogram>=0.11",
#   "numpy<=1.26.4",
#   "onnx>=1.16.1,<=1.18.0",
#   "onnxruntime>=1.17.0",
#   "torch @ https://download.pytorch.org/whl/cpu/torch-2.4.0%2Bcpu-cp38-cp38-linux_x86_64.whl",
# ]
# ///

import argparse
from rknn.api import RKNN

parser = argparse.ArgumentParser()
parser.add_argument("onnx")
parser.add_argument("output")
args = parser.parse_args()
rknn = RKNN(verbose=False)
try:
    if rknn.config(target_platform="rk3566") != 0:
        raise SystemExit("RKNN config failed")
    if rknn.load_onnx(model=args.onnx) != 0:
        raise SystemExit("ONNX load failed")
    if rknn.build(do_quantization=False) != 0:
        raise SystemExit("RKNN build failed")
    if rknn.export_rknn(args.output) != 0:
        raise SystemExit("RKNN export failed")
finally:
    rknn.release()
