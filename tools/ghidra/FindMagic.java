// Find functions that compare against a 4-character file magic and decompile them.
// Args: <outDir> <MAGIC> [<MAGIC> ...]   e.g. out ANIX SKEL MMSH
//@category SoM

import java.io.File;
import java.io.PrintWriter;
import java.util.LinkedHashSet;
import java.util.Set;

import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.Instruction;
import ghidra.program.model.listing.InstructionIterator;
import ghidra.program.model.scalar.Scalar;

public class FindMagic extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        File out = new File(args[0]);
        out.mkdirs();
        DecompInterface decomp = new DecompInterface();
        decomp.openProgram(currentProgram);

        for (int i = 1; i < args.length; i++) {
            String magic = args[i];
            // Files store the tag as bytes in order, so the u32 compare constant is little endian.
            long value = 0;
            for (int k = 3; k >= 0; k--) {
                value = (value << 8) | (magic.charAt(k) & 0xff);
            }

            Set<Function> functions = new LinkedHashSet<>();
            InstructionIterator it = currentProgram.getListing().getInstructions(true);
            while (it.hasNext()) {
                Instruction ins = it.next();
                for (int op = 0; op < ins.getNumOperands(); op++) {
                    for (Object o : ins.getOpObjects(op)) {
                        if (o instanceof Scalar && (((Scalar) o).getValue() & 0xffffffffL) == value) {
                            Function f = getFunctionContaining(ins.getAddress());
                            if (f != null) {
                                functions.add(f);
                            }
                        }
                    }
                }
            }

            try (PrintWriter w = new PrintWriter(new File(out, magic + ".c"))) {
                w.println("// " + functions.size() + " functions reference " + magic);
                for (Function f : functions) {
                    w.println("// ==== " + f.getName() + " @ " + f.getEntryPoint());
                    DecompileResults r = decomp.decompileFunction(f, 90, monitor);
                    if (r.decompileCompleted()) {
                        w.println(r.getDecompiledFunction().getC());
                    } else {
                        w.println("// decompile failed: " + r.getErrorMessage());
                    }
                }
            }
            println(magic + ": " + functions.size() + " functions");
        }
    }
}
