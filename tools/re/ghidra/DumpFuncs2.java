// Decompile a fixed set of functions without relying on full auto-analysis:
// disassemble a window around each address, create the function if missing,
// then decompile. Run with -noanalysis so the import is fast.
//   DUMP_OUT=/tmp/ghidra-out.c analyzeHeadless ... -noanalysis -postScript DumpFuncs2.java
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.app.cmd.disassemble.DisassembleCommand;
import ghidra.app.cmd.function.CreateFunctionCmd;
import ghidra.program.model.address.Address;
import ghidra.program.model.address.AddressSet;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.Instruction;

import java.io.FileWriter;
import java.io.PrintWriter;

public class DumpFuncs2 extends GhidraScript {
    /** Ghidra imports this PIE at image base 0x100000, so file VA + 0x100000. */
    private static final long BASE = 0x100000L;
    private static final long[] ADDRS = {
        0x15e70c0L, 0x1660370L, 0x16b6060L, 0x14d0890L, 0x1d35a50L,
        0x2941f80L, 0x2803b90L, 0x1679d60L, 0x1688c80L, 0x1149880L,
    };

    private void prepare(Address addr) {
        if (getFunctionAt(addr) != null) return;
        Address lo = addr.subtract(0x40);
        Address hi = addr.add(0x20000);
        DisassembleCommand dis = new DisassembleCommand(addr, new AddressSet(lo, hi), true);
        dis.applyTo(currentProgram, monitor);
        CreateFunctionCmd cmd = new CreateFunctionCmd(addr);
        cmd.applyTo(currentProgram, monitor);
    }

    @Override
    public void run() throws Exception {
        String out = System.getenv("DUMP_OUT");
        if (out == null) out = "/tmp/ghidra-out.c";
        println("image base = " + currentProgram.getImageBase());
        DecompInterface ifc = new DecompInterface();
        ifc.toggleCCode(true);
        ifc.toggleSyntaxTree(true);
        ifc.openProgram(currentProgram);
        try (PrintWriter w = new PrintWriter(new FileWriter(out))) {
            for (long a : ADDRS) {
                Address addr = toAddr(BASE + a);
                prepare(addr);
                Function f = getFunctionAt(addr);
                int n = 0;
                for (Instruction i = getInstructionAt(addr); i != null && n < 4000; i = i.getNext(), n++) {
                    // count the instructions we can see from the entry
                }
                w.printf("%n/* ===== %#x %s (%d insns) ===== */%n", a,
                        f == null ? "(no function)" : f.getName(), n);
                println("decompiling " + Long.toHexString(a) + " fn=" + (f != null));
                if (f == null) {
                    w.println("/* not defined as a function */");
                    continue;
                }
                DecompileResults res = ifc.decompileFunction(f, 120, monitor);
                if (!res.decompileCompleted()) {
                    w.println("/* decompile failed: " + res.getErrorMessage() + " */");
                } else {
                    w.println(res.getDecompiledFunction().getC());
                }
            }
        } finally {
            ifc.dispose();
        }
        println("wrote " + out);
    }
}
