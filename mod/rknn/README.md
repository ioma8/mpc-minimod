# RK3566 dictation backend

This backend exports the existing OpenAI Whisper `tiny.en` weights to fixed 20-second ONNX graphs, converts those graphs to RK3566 RKNN models with Rockchip Toolkit2 2.5.0, then builds a persistent ARM64 inference worker against the matching Model Zoo runtime.

Run `./build.sh dictation-rknn` from `mod/`. It needs `uv`, Docker with ARM64 and AMD64 emulation, and network access for the pinned upstream tools and weights. Generated models and toolchains stay under `mod/build/`; the final ZIP carries only the runtime, models, and helper.

The RKNN model zoo demo uses a 512-wide decoder output by default. The build adjusts that width to Whisper tiny.en's 384 channels. NPU performance and runtime compatibility still need a physical MPC test.
