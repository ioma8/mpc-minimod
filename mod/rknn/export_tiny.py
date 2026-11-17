# /// script
# requires-python = ">=3.10,<3.13"
# dependencies = [
#   "tiktoken==0.7.0",
#   "torch==2.4.0",
#   "onnx==1.18.0",
#   "numpy==1.26.4",
#   "numba==0.60.0",
#   "tqdm>=4.41",
#   "more-itertools",
# ]
# ///

import argparse
import os
import sys
from pathlib import Path

import torch
import torch.nn.functional as F

parser = argparse.ArgumentParser()
parser.add_argument("--model-dir", type=Path, required=True)
args = parser.parse_args()

source = Path(os.environ["MINIMOD_WHISPER_SOURCE"])
sys.path.insert(0, str(source))
import whisper


def short_encoder_forward(self, x):
    x = F.gelu(self.conv1(x))
    x = F.gelu(self.conv2(x))
    x = x.permute(0, 2, 1)
    x = (x + self.positional_embedding[: x.shape[1]]).to(x.dtype)
    for block in self.blocks:
        x = block(x)
    return self.ln_post(x)


model = whisper.load_model("tiny.en", device="cpu")
model.eval()
model.requires_grad_(False)
model.encoder.forward = short_encoder_forward.__get__(model.encoder)
args.model_dir.mkdir(parents=True, exist_ok=True)
mel = torch.randn(1, 80, 2000)
tokens = torch.randint(0, 100, (1, 12), dtype=torch.long)
with torch.no_grad():
    encoded = model.encoder(mel)
for name, module, inputs, names in (
    ("encoder", model.encoder, (mel,), (["x"], ["out"])),
    ("decoder", model.decoder, (tokens, encoded), (["tokens", "audio"], ["out"])),
):
    input_names, output_names = names
    destination = args.model_dir / f"whisper_{name}_tiny.en_20s.onnx"
    torch.onnx.export(
        module,
        inputs,
        destination,
        input_names=input_names,
        output_names=output_names,
        opset_version=12,
    )
    print(f"wrote {destination} ({destination.stat().st_size} bytes)")
