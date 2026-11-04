import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.mem.Memory;

public class DumpVtable extends GhidraScript {
    @Override
    public void run() throws Exception {
        long base = Long.decode(getScriptArgs()[0]);
        int count = Integer.parseInt(getScriptArgs()[1]);
        Memory mem = currentProgram.getMemory();
        for (int i = 0; i < count; i++) {
            Address a = toAddr(base + i * 8L);
            long v = mem.getLong(a);
            Function f = getFunctionAt(toAddr(v));
            println(String.format("%02d %s -> %#x %s", i, a, v,
                f == null ? "" : f.getName()));
        }
    }
}
