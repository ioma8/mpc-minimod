"""Resolve ADRP+ADD references in the MPC Sample ARM64 binary.

The binary is stripped, so we recover xrefs to interesting .rodata addresses by
decoding the instruction stream ourselves.
"""
from __future__ import annotations
import sys, struct, pickle
import capstone
from capstone import arm64 as ca

PATH = "/tmp/mpcwork/fw/x/usr/bin/MPC Sample"
TEXT_OFF, TEXT_VADDR, TEXT_SIZE = 0xB55B00, 0xB55B00, 0x3A28904

def main(targets: dict[str, int], out_path: str) -> None:
    data = open(PATH, "rb").read()
    code = data[TEXT_OFF : TEXT_OFF + TEXT_SIZE]
    md = capstone.Cs(capstone.CS_ARCH_ARM64, capstone.CS_MODE_ARM)
    md.detail = True
    hits: dict[str, list[int]] = {k: [] for k in targets}
    want = {v: k for k, v in targets.items()}
    adrp: dict[int, int] = {}
    for ins in md.disasm(code, TEXT_VADDR):
        if ins.id == ca.ARM64_INS_ADRP:
            rd = ins.operands[0].reg
            adrp[rd] = ins.operands[1].imm
        elif ins.id in (ca.ARM64_INS_ADD, ca.ARM64_INS_LDR):
            ops = ins.operands
            if len(ops) >= 3 and ops[1].type == ca.ARM64_OP_REG and ops[2].type == ca.ARM64_OP_IMM:
                rd, rn, imm = ops[0].reg, ops[1].reg, ops[2].imm
                if rn in adrp:
                    addr = (adrp[rn] + imm) & 0xFFFFFFFFFFFFFFFF
                    if addr in want:
                        hits[want[addr]].append(ins.address)
    pickle.dump(hits, open(out_path, "wb"))
    for k, v in hits.items():
        print(f"{k}: {len(v)} xrefs {[hex(x) for x in v[:6]]}")

if __name__ == "__main__":
    import json
    targets = json.load(open(sys.argv[1]))
    main(targets, sys.argv[2])
