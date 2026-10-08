// Ad-hoc queries against the analysed program. Output goes to a file.
// Args:
//   <outFile> dec <depth> <addr> [<addr> ...]   decompile functions and their direct callees to <depth>
//   <outFile> sym <regex>                      list symbols whose name or namespace matches
//   <outFile> xref <addr> [<addr> ...]         list the functions that reference each address
//   <outFile> str <regex>                      list defined strings matching the regex, with referrers
//@category SoM

import java.io.File;
import java.io.PrintWriter;
import java.util.ArrayDeque;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.Set;
import java.util.regex.Pattern;

import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Data;
import ghidra.program.model.listing.Function;
import ghidra.program.model.symbol.Reference;
import ghidra.program.model.symbol.Symbol;
import ghidra.program.model.symbol.SymbolIterator;

public class Query extends GhidraScript {
    private static final int MAX_FUNCTIONS = 60;

    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        try (PrintWriter w = new PrintWriter(new File(a[0]))) {
            switch (a[1]) {
                case "dec": decompile(w, Integer.parseInt(a[2]), a, 3); break;
                // analyzeHeadless.bat mangles '|', so ',' separates alternatives in regexes.
                case "sym": symbols(w, a[2].replace(',', '|')); break;
                case "xref": xrefs(w, a, 2); break;
                case "ptrs": pointers(w, addr(a[2]), Integer.parseInt(a[3])); break;
                case "f32": floatConstants(w, a, 2); break;
                case "funcs": functionList(w, addr(a[2]), addr(a[3])); break;
                case "str": strings(w, a[2].replace(',', '|')); break;
                case "vtables": vtables(w, Integer.parseInt(a[2])); break;
                case "dis": disassemble(w, addr(a[2]), Integer.parseInt(a[3])); break;
                case "mem": memory(w, addr(a[2]), Integer.parseInt(a[3])); break;
                case "disp": displacements(w, a[2], Integer.parseInt(a[3])); break;
                default: w.println("unknown mode " + a[1]);
            }
        }
    }

    private Address addr(String s) {
        return toAddr(s.startsWith("0x") ? s.substring(2) : s);
    }

    private void decompile(PrintWriter w, int depth, String[] a, int from) throws Exception {
        DecompInterface di = new DecompInterface();
        di.openProgram(currentProgram);
        Set<Address> seen = new HashSet<>();
        ArrayDeque<Object[]> queue = new ArrayDeque<>();
        for (int i = from; i < a.length; i++) {
            Function f = getFunctionAt(addr(a[i]));
            if (f == null) f = getFunctionContaining(addr(a[i]));
            if (f != null) queue.add(new Object[] { f, 0 });
            else w.println("// no function at " + a[i]);
        }
        int count = 0;
        while (!queue.isEmpty() && count < MAX_FUNCTIONS) {
            Object[] item = queue.poll();
            Function f = (Function) item[0];
            int d = (Integer) item[1];
            if (!seen.add(f.getEntryPoint())) continue;
            count++;
            w.println("// ==== " + f.getName() + " @ " + f.getEntryPoint() + "  depth " + d
                    + "  size " + f.getBody().getNumAddresses());
            DecompileResults r = di.decompileFunction(f, 90, monitor);
            w.println(r.decompileCompleted() ? r.getDecompiledFunction().getC() : "// decompile failed");
            if (d < depth) {
                for (Function callee : f.getCalledFunctions(monitor)) {
                    if (!callee.isThunk() && !callee.isExternal()) queue.add(new Object[] { callee, d + 1 });
                }
            }
        }
        if (!queue.isEmpty()) w.println("// stopped at " + MAX_FUNCTIONS + " functions");
    }

    /**
     * For each 32-bit constant (hex, e.g. 3A802008), find where it sits in initialised memory and
     * list the functions that reference that spot or the 16-byte vector containing it.
     */
    private void floatConstants(PrintWriter w, String[] a, int from) throws Exception {
        for (int i = from; i < a.length; i++) {
            long v = Long.parseLong(a[i], 16);
            byte[] pattern = new byte[] { (byte) v, (byte) (v >> 8), (byte) (v >> 16), (byte) (v >> 24) };
            w.println("// constant 0x" + a[i]);
            Address at = currentProgram.getMinAddress();
            int matches = 0;
            while (at != null && matches < 40) {
                at = currentProgram.getMemory().findBytes(at, pattern, null, true, monitor);
                if (at == null) break;
                matches++;
                Set<String> users = new java.util.TreeSet<>();
                for (int back = 0; back < 16; back += 4) {
                    Address base = at.subtract(back);
                    for (Reference r : getReferencesTo(base)) {
                        Function f = getFunctionContaining(r.getFromAddress());
                        if (f != null) users.add(f.getEntryPoint().toString());
                    }
                }
                w.println("  at " + at + " used by " + users);
                at = at.next();
            }
        }
    }

    /** List functions in [start, end] with size and the number of distinct calling functions. */
    private void functionList(PrintWriter w, Address start, Address end) {
        for (Function f : currentProgram.getFunctionManager().getFunctions(start, true)) {
            if (f.getEntryPoint().compareTo(end) > 0) break;
            Set<Address> callers = new HashSet<>();
            for (Reference r : getReferencesTo(f.getEntryPoint())) {
                Function c = getFunctionContaining(r.getFromAddress());
                if (c != null) callers.add(c.getEntryPoint());
            }
            w.println(f.getEntryPoint() + "  size " + f.getBody().getNumAddresses() + "  callers " + callers.size()
                    + (callers.size() <= 3 ? " " + callers : ""));
        }
    }

    /** Print <count> qwords from <start>, naming the function each one points at (vtable dump). */


    /** Runs of at least `min` consecutive 8-byte pointers to function entry points in initialised data (candidate vtables). */
    private void vtables(PrintWriter w, int min) throws Exception {
        ghidra.program.model.mem.Memory mem = currentProgram.getMemory();
        ghidra.program.model.listing.FunctionManager fm = currentProgram.getFunctionManager();
        for (ghidra.program.model.mem.MemoryBlock b : mem.getBlocks()) {
            if (!b.isInitialized() || b.isExecute()) continue;
            Address at = b.getStart();
            long end = b.getEnd().getOffset();
            long runStart = -1;
            int run = 0;
            while (at.getOffset() + 8 <= end) {
                long v = mem.getLong(at);
                boolean fn = v != 0 && fm.getFunctionAt(toAddr(v)) != null;
                if (fn) {
                    if (run == 0) runStart = at.getOffset();
                    run++;
                } else {
                    if (run >= min) w.println(Long.toHexString(runStart) + "  entries " + run);
                    run = 0;
                }
                at = at.add(8);
            }
            if (run >= min) w.println(Long.toHexString(runStart) + "  entries " + run);
        }
    }

    /** Print `count` instructions from `start`. */
    /** `mem <addr> <bytes>`: raw bytes of the loaded image as little-endian qwords, with the pointer target when it is one. */
    private void memory(PrintWriter w, Address start, int bytes) throws Exception {
        for (int o = 0; o < bytes; o += 8) {
            Address at = start.add(o);
            long v = currentProgram.getMemory().getLong(at);
            w.println(String.format("%s  %016x  %08x %08x  f32 %g %g", at, v, (int) v, (int) (v >>> 32), Float.intBitsToFloat((int) v), Float.intBitsToFloat((int) (v >>> 32))));
        }
    }

    private void disassemble(PrintWriter w, Address start, int count) {
        ghidra.program.model.listing.Listing l = currentProgram.getListing();
        ghidra.program.model.listing.Instruction ins = l.getInstructionAt(start);
        if (ins == null) {
            ghidra.app.cmd.disassemble.DisassembleCommand cmd = new ghidra.app.cmd.disassemble.DisassembleCommand(start, null, true);
            cmd.applyTo(currentProgram);
            ins = l.getInstructionAt(start);
        }
        for (int i = 0; i < count && ins != null; i++) {
            w.println(ins.getAddress() + "  " + ins);
            ins = ins.getNext();
        }
    }

    /** Functions whose memory operands use at least `min` of the given displacements (comma separated hex). */
    private void displacements(PrintWriter w, String list, int min) {
        Set<Long> want = new HashSet<>();
        for (String t : list.split(",")) want.add(Long.parseLong(t.replace("0x", ""), 16));
        for (Function f : currentProgram.getFunctionManager().getFunctions(true)) {
            Set<Long> hit = new HashSet<>();
            ghidra.program.model.listing.InstructionIterator it = currentProgram.getListing().getInstructions(f.getBody(), true);
            while (it.hasNext()) {
                ghidra.program.model.listing.Instruction ins = it.next();
                for (int op = 0; op < ins.getNumOperands(); op++) {
                    for (Object o : ins.getOpObjects(op)) {
                        if (o instanceof ghidra.program.model.scalar.Scalar sc && want.contains(sc.getUnsignedValue())) hit.add(sc.getUnsignedValue());
                    }
                }
            }
            if (hit.size() >= min) w.println(f.getEntryPoint() + "  " + f.getName() + "  size " + f.getBody().getNumAddresses() + "  hits " + hit.size());
        }
    }

    private void pointers(PrintWriter w, Address start, int count) throws Exception {
        for (int i = 0; i < count; i++) {
            Address at = start.add(8L * i);
            long v = currentProgram.getMemory().getLong(at);
            Address target = toAddr(v);
            Function f = target == null ? null : getFunctionAt(target);
            w.println(at + "  [" + i + "]  " + Long.toHexString(v) + "  " + (f == null ? "-" : f.getName()));
        }
    }

    private void symbols(PrintWriter w, String regex) {
        Pattern p = Pattern.compile(regex, Pattern.CASE_INSENSITIVE);
        SymbolIterator it = currentProgram.getSymbolTable().getAllSymbols(true);
        while (it.hasNext()) {
            Symbol s = it.next();
            String full = s.getName(true);
            if (p.matcher(full).find()) w.println(s.getAddress() + "  " + s.getSymbolType() + "  " + full);
        }
    }

    private void xrefs(PrintWriter w, String[] a, int from) {
        for (int i = from; i < a.length; i++) {
            Address target = addr(a[i]);
            w.println("// refs to " + target);
            Map<String, String> out = new LinkedHashMap<>();
            for (Reference r : getReferencesTo(target)) {
                Function f = getFunctionContaining(r.getFromAddress());
                w.println("  " + r.getFromAddress() + "  " + r.getReferenceType() + "  in "
                        + (f == null ? "(none)" : f.getName() + " @ " + f.getEntryPoint()));
            }
        }
    }

    private void strings(PrintWriter w, String regex) {
        Pattern p = Pattern.compile(regex, Pattern.CASE_INSENSITIVE);
        for (Data d : currentProgram.getListing().getDefinedData(true)) {
            if (!d.hasStringValue()) continue;
            String v = String.valueOf(d.getValue());
            if (!p.matcher(v).find()) continue;
            w.println(d.getAddress() + "  \"" + v + "\"");
            for (Reference r : getReferencesTo(d.getAddress())) {
                Function f = getFunctionContaining(r.getFromAddress());
                w.println("    <- " + r.getFromAddress() + " in " + (f == null ? "(none)" : f.getName() + " @ " + f.getEntryPoint()));
            }
        }
    }
}
