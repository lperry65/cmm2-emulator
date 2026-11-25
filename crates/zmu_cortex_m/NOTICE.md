# zmu_cortex_m

This directory is a vendored copy of the `zmu_cortex_m` crate from zmu
(https://github.com/jjkt/zmu), taken from upstream commit 72feb75. zmu is
licensed under the Apache License 2.0; the original `LICENSE` is in this
directory and still applies to the whole crate, including my changes.

Two editor project files from the upstream tree were left out. Everything else
was imported unchanged first, and my changes went in as a separate commit.

## My changes

The firmware needs a few things zmu does not do, so I changed the following.

- Flash base address: the flash memory can now sit at a base address (`FlashMemory::new_at`,
  `Processor::flash_memory_at`), and instruction caching and fetching use that base, so the ITCM
  region is reachable.
- Interrupts: the interrupt count is raised from 32 to 240. The NVIC register decode used the wrong
  shift for the word index (it only worked for word 0), and the pending-exception and clear scans
  used the wrong interrupt number for words above 0.
- Core register: the CCR register (0xE000ED14) is now writable.
- Core registers: AIRCR, DFSR, MMFAR, BFAR and AFSR can now be written (they were
  read-only). AIRCR only accepts a write that carries the VECTKEY, and reads 0xFA05_0000
  before it is written.
- System reset: a write to AIRCR with the VECTKEY and SYSRESETREQ sets
  `Processor::system_reset_requested`, and the owner of the processor does the reset. Reset now
  also clears the interrupt controller, SysTick and the system-control registers (SHPR, SCR,
  CCR, AIRCR, DWT control) to the values `Processor::new` gives them. The cycle counter and
  the instruction counts are kept.
- CCR and zero divide: SDIV and UDIV trap on division by zero only when
  CCR.DIV_0_TRP is set; before, they always trapped.
- Flash writes: stores to flash are allowed, and the cached decoded
  instructions covering the written bytes are decoded again.
- LDR literal: the decoder now treats `Rn = PC` in the LDR immediate (T4) and LDR register (T2)
  encodings as the literal form, which is how `U = 0` is encoded. LDR literal with `Rt = PC` now
  reports a branch instead of a normal instruction.
- New instructions: REV.W, REV16.W and REVSH.W, RBIT, SSAT and USAT, which were
  decoded as undefined before. SSAT and USAT set the Q flag on saturation.
- ORN with an immediate (T1) was decoded as undefined; it now runs. The firmware's
  tokeniser uses it.
- Floating-point rounding instructions: VRINTA, VRINTN, VRINTP, VRINTM, VRINTR and VRINTX were
  decoded as undefined (only VRINTZ ran), and now run. The firmware's maths library uses VRINTA,
  VRINTM and VRINTP, and a plain BASIC `FOR` loop stopped at the first VRINTM.F64.
- VMAXNM and VMINNM, VFNMA, and VCVTA, VCVTN, VCVTP and VCVTM (convert to an integer with a named
  rounding) were decoded as undefined; they now run. A quiet NaN gives way to a number in VMAXNM
  and VMINNM, and +0 counts as larger than -0.
- VCVT with fraction bits (float to and from 16 and 32-bit fixed point) was decoded as the plain
  integer conversion, with the wrong source register and no fraction bits, so it ran and gave wrong
  answers. It now has its own decoder and executor. The firmware has about thirty of them.
- DSP extension: UXTAH, SXTAH and SXTAB (UXTAB already ran), and PKHBT and PKHTB, were decoded as
  undefined. The extend-and-add forms share one executor. The firmware has about thirty-five UXTAH
  and SXTAH. UXTAB, SXTAB, UXTAH, SXTAH and PKH are decoded only when the DSP extension is built in
  (they were in the general list before), so an armv7m build without it treats them as undefined,
  like SEL and UADD8. PKHTB's disassembly dropped its shift (`asr #n`, `asr #32` for an immediate of
  0); it shows it now.
- DSP extension, the 36 parallel adds and subtracts: only UADD8 ran, and the other 35 (SADD8,
  SADD16, SASX, SSAX, SSUB8, SSUB16, the Q, SH, UQ and UH forms of the same six, UADD16, UASX, USAX,
  USUB8 and USUB16) were decoded as undefined. Galactic Raiders stops at UQSUB8 (`fac0 f051`; an
  earlier reading took it as UQADD8, but op1 = 100 is the subtract). They are one decoder and one
  executor now, with the sign, the kind (plain, saturating, halving) and the lane arrangement as
  data (`Instruction::PARALLEL`, `decoder/parallel.rs`, `executor/parallel_add.rs`), and UADD8 is
  one of them. Only the S and U forms write GE (a byte lane one bit, a halfword lane two; the flag
  is `>= 0`, or for an unsigned add the carry out of the lane); none touches Q. Rd, Rn or Rm as SP
  or PC is UNPREDICTABLE and decodes as UDF; without the DSP extension they are all undefined.
  Checked against the pseudocode on each of the A7.7 pages, and against llvm-mc over 2 million
  opcodes of the 0xFA00 to 0xFBFF range.
- MRS read the GE bits for SYSm 5, 6 and 7 (IPSR, EPSR, IEPSR); the manual reads the APSR bits,
  GE included, only when SYSm<2> is 0.
- MSR with the mask 01 (`apsr_g`) or 11 (`apsr_nzcvqg`) was decoded as a branch: the table only
  matched mask 10. They decode now, so a program can set GE for SEL. A mask of 00, or any mask but
  10 for a SYSm above 3, is UNPREDICTABLE and decodes as UDF.
- DSP extension, QADD, QSUB, QDADD and QDSUB were decoded as undefined. They are one decoder and one
  executor (`Instruction::QSAT`). The manual's operands are Rd, Rm, Rn and the result is Rm plus or
  minus Rn, with Rn doubled first (and saturated) in the two D forms. Q is set when either step
  saturates, and never cleared. An earlier reading expected Galactic Raiders to need them (its
  text code has two QADD).
- DSP extension, SMMUL, SMMLA and SMMLS with their R forms were decoded as undefined (the game's
  drawing code has twelve SMMUL). One decoder and one executor (`Instruction::SMM`): the high word
  of `(Ra << 32) +- Rn * Rm` in 64 bits, with 0x8000_0000 added first for the R forms. The sum can
  pass 2^63 and only bits 63:32 are kept, so it wraps. SMMLA with Ra = PC is SMMUL; SMMLS with Ra =
  PC or SP, and SMMLA with Ra = SP, are UNPREDICTABLE and decode as UDF. None of them sets Q.
- DSP extension, USAD8 and USADA8 (A7.7.211, .212) were decoded as undefined. One decoder and one
  executor (`Instruction::USAD8`, `decoder/usad8.rs`, `executor/sum_abs_diff.rs`): the sum of the
  four absolute differences of the unsigned bytes, with Ra added for USADA8 (Ra = PC is USAD8),
  wrapped to 32 bits. Neither GE nor Q is touched. Table A5-28 has the two Ra columns of this row
  the wrong way round; I followed the instruction pages, and llvm and Unicorn agree with them.
- DSP extension, SXTB16, SXTAB16, UXTB16 and UXTAB16 (A7.7.183, .180, .222, .219) were decoded as
  undefined. One decoder and one executor (`Instruction::EXTEND16`, `executor/extend16.rs`): the
  bytes 0 and 2 of Rm rotated by 0, 8, 16 or 24, extended to two halfwords, and for the add forms
  added to the halfwords of Rn, each wrapping on its own with no carry into the other. Rn = PC is
  the plain extend. The existing SXTAB, SXTAH, UXTAB and UXTAH were checked in the same run (all
  four rotations, a sum that wraps, Rd = Rn and Rd = Rm), and were right; what they lacked was the
  register rule of their pages (Rd or Rm SP or PC, and Rn = SP, are UNPREDICTABLE), which now
  decodes as UDF for them too (`decoder/unpredictable_dsp.rs`). The bit hw2<6> that the diagrams
  mark (0) must be 0 here, as it is for the other extends; llvm-mc also decodes it set (see below).
- SSAT16 and USAT16 (A7.7.153, .214) were decoded as SSAT and USAT with `ASR #32`, so they ran with
  a silently wrong result. They are `Instruction::SAT16` now (`decoder/sat16.rs`,
  `executor/saturate16.rs`): the two halfwords are saturated on their own to 1 to 16 bits
  (SSAT16) or 0 to 15 (USAT16), and Q is set if either saturated. The table entry comes before
  SSAT and USAT, and the SSAT and USAT decoders refuse `sh = 1` with a shift amount of 0 (what is
  left of that space, sat_imm bit 4 or 5 set, the (0) bits of SSAT16; and all of it without the DSP
  extension, where A7.7.152 says UNDEFINED), so none of it is an ASR #32 any more. The shift forms
  of SSAT and USAT themselves (LSL 0 to 31, ASR 1 to 31, 1 to 32 and 0 to 31 bits, Q) were
  checked and were right. Their disassembly now shows the shift, and Rd or Rn as SP or PC is
  UNPREDICTABLE and decodes as UDF (it was only PC for Rd). The clamp is one `saturate()` in
  `core/operation.rs` for SSAT, USAT, SSAT16 and USAT16.
- DSP extension, SMULWB, SMULWT, SMLAWB and SMLAWT (A7.7.150, .141) were decoded as undefined
  (`Instruction::SMULW`, `executor/word_multiply.rs`): bits 47:16 of Rn times the chosen signed
  halfword of Rm, with Ra << 16 added first for SMLAW, which sets Q (never clears it) when that
  sum does not fit 32 bits. SMULW cannot overflow and never sets Q.
- DSP extension, SMUAD, SMUSD, SMLAD and SMLSD with their X forms (A7.7.147, .151, .137, .142) and
  SMLALD, SMLALDX, SMLSLD and SMLSLDX (A7.7.140, .143) were decoded as undefined
  (`Instruction::SMDUAL` and `SMDUAL_LONG`, `executor/dual_multiply.rs`). The two signed halfword
  products (Rm's halfwords exchanged for X) are added or subtracted, and Ra, or the 64-bit
  RdHi:RdLo, added, in a wide integer. For the 32-bit ones Q is set when the whole sum does not fit
  (SMUSD cannot overflow); for the long ones the sum wraps at 2^64 and Q is not set.
- DSP extension, SMLALBB, SMLALBT, SMLALTB and SMLALTT (A7.7.139) and UMAAL (A7.7.202, which is
  Armv7E-M, so it needs the DSP extension too) were decoded as undefined
  (`Instruction::SMLAL_half` and `UMAAL`, `executor/long_multiply.rs`). The signed halfword
  product wraps into the 64-bit accumulate without Q; UMAAL is Rn * Rm + RdHi + RdLo unsigned,
  which is at most 2^64 - 1. RdHi = RdLo, and SP or PC in any register, are UNPREDICTABLE for all
  the long ones here and decode as UDF; without the DSP extension all of these are undefined.
  (SMLAL, UMLAL, SMULL and UMULL still lack those two rules; they are not part of this batch.)
- Checked against Unicorn 2.1.5 (QEMU), a Cortex-M7 in Thumb mode, with random vectors mixed with
  boundary values and random NZCVQ and GE, registers chosen to overlap: two runs of 600,000
  vectors (1,200,000 in all) over USAD8, USADA8, SXTB16, SXTAB16, UXTB16, UXTAB16, SXTAB, UXTAB,
  SXTAH, UXTAH, SXTB, UXTH (10,000 to 20,000 each), SSAT16, USAT16 (20,000), SSAT, USAT (60,000),
  SMULW, SMLAW, SMUAD, SMLAD, SMUSD, SMLSD, SMLALD, SMLSLD, UMAAL (20,000 each) and SMLAL<x><y>
  (40,000), per run. One difference, 26 vectors of the second run, all SMLAD: Unicorn sets Q when
  the partial sum of the two products is 2^31 (both halfwords of both operands -32768) even when
  the accumulate then brings the total back in range. The pseudocode of A7.7.137 sets Q only if
  the whole sum differs from its low 32 bits, and I follow it; real silicon was not available to
  decide. Every other register, flag, Q and GE matched. A harness with four planted bugs showed
  mismatches in each of the instructions it touched, so it does see these differences. What the
  check could not cover: encodings with an UNPREDICTABLE register (the runner never makes one) and
  the bits that must be zero.
- The decode was swept against llvm-mc over 7.6 million opcodes (hw1 in the SSAT, USAT, extend,
  multiply and long multiply ranges with Rn 0, 2, 13 or 15, hw2 all 65,536): the 226,192 opcodes
  this tree decodes as one of these families (and SMLAL) all assemble with llvm-mc to the same
  opcode from their disassembly text, but for 9,304 existing SMLAL ones llvm calls UNPREDICTABLE;
  and every opcode llvm decodes as one of them that this tree does not (351,408) is SP or PC as a
  register, RdLo = RdHi, SMLAL, or the extends with the bit hw2<6> set, which the manual marks
  (0) and this tree, as for the existing extends, treats as undefined.
- LDMDB (LDMEA is the same encoding) was hardwired to UDF in the decoder. It runs now
  (`exec_ldmdb`). All the words are read before anything is written, so a fault, which is precise,
  leaves the base and every other register as they were. Write back comes before the load of PC,
  so an exception return through `ldmdb sp!, {..., pc}` unstacks from the new SP, as POP does.
  The base in the list with write back, fewer than two registers, PC and LR together, and PC as
  the base are UNPREDICTABLE and decode as UDF. An unaligned base is a UsageFault, as for LDM.
  A PC load inside an IT block that is not its last instruction is also UNPREDICTABLE in the
  manual; it is not checked here, as for LDM and POP.
  Its write back deliberately comes before the PC load. The literal pseudocode of A7.7.42 runs
  LoadWritePC (BXWritePC, and so ExceptionReturn) first and writes back afterwards; but the write
  back has to reach the SP bank in use before the return, and the frame has to be read from the SP
  the instruction leaves, as POP's pseudocode (A7.7.99) has it and as real cores and QEMU do.
  `exec_ldm` used to write back after the return; see the entry below (issue 80), which fixed it.
  LDMDB counts 1 + N cycles, and 4 + N with a PC load, as POP does; the rule is the one below.
- `executor/saturate.rs` is empty and nothing refers to it. It is empty in the upstream import
  too, so I left it. `executor/saturating_add_sub.rs` was empty upstream and is now the executor
  of QADD, QSUB, QDADD and QDSUB.
- An exception return from the last instruction of an IT block lost the ITSTATE it restored
  (issue 79): `execute()` advanced the ITSTATE after any branch, and after an exception return
  that was the ITSTATE just unstacked for the interrupted code, so the thread's next instruction
  ran unconditionally instead of under the condition the frame said. It does not advance after an
  exception return now (the flag `exception_returned`, which the tail-chaining already had). It
  holds for BX, POP, LDM, LDMDB and LDR with PC. `MOV PC, LR` is not an exception return (it is
  BranchWritePC), so it advances the block as any branch does. The manual makes BX, POP and LDM
  with PC UNPREDICTABLE inside an IT block unless last; zmu runs the return, and the ITSTATE is
  the frame's whatever the handler's own block said.
- `exec_ldm` wrote back after the exception return (issue 80): `ldm.w r2!, {r4, pc}` in Handler
  mode, loading EXC_RETURN, left r2 as the stacked value plus 8. LDM and LDMDB are one routine
  now: every word is read first (so a fault leaves every register as it was, where LDM used to
  leave the registers it had loaded), the write back comes next, and the PC load is last, as POP
  and LDR with write back already did. The same ordering was checked in LDR (immediate; the
  register form has no write back in Thumb) and in POP: both were right.
- Cycles of a load of PC: LDM counted 1 + N and POP 4 + N. One rule now, for LDM, LDMDB and POP:
  1 + N, and a pipeline refill of P = 3 on top for a load of PC (4 + N). ARM publishes no
  Cortex-M7 per-instruction timings; the M4 TRM gives 1 + N for LDM and POP and adds P, 1 to 3, for
  a PC load, and POP's 3 was the value already in use. What moves is only an LDM that loads PC (a
  `pop`-shaped `ldm sp!, {..., pc}` is decoded as POP and did not change), by 3 cycles. LDR with PC
  keeps its 2 cycles.
- SP bits 1:0 are RAZ/WI (issue 78). B1.4.1 (page B1-516) says "Armv7-M implementations treat SP
  bits[1:0] as RAZ/WI"; A5.1.3 (page A5-127) makes them SBZP (a non-zero write is UNPREDICTABLE, a
  read returns zero); and the pseudocode of `R[]` in B1.4.7 reads `SP<31:2>:'00'` (page B1-521) and
  writes `SP<31:2>:'00'` (page B1-522). The pseudocode of MSR itself writes `SP_main = R[n]` with no
  mask, so the mask on the MSR path comes from the B1.4.1 RAZ/WI rule. `mov sp, r1` with non-zero
  low bits is UNPREDICTABLE under A5.1.3 but write-ignored under B1.4.1, so dropping the bits is a
  permitted choice. zmu kept bits 1:0 of a value written to SP. Every write to `SP_main` and
  `SP_process` now drops them: `set_r(SP)`, `set_msp`, `set_psp`, the SP arithmetic, MSR MSP and
  PSP (which went round `set_msp`), the stack pointer loaded at reset (already masked) and the
  exception return (the pop adds a multiple of 4 to an aligned SP, and bit 2 comes back as
  STKALIGN says). The effect is that SP is never misaligned, so a PUSH or POP cannot raise the
  alignment UsageFault from SP any more; four tests in `executor/mod.rs` that built that fault with
  an odd SP were changed to use another base register or to say that it cannot happen, and the
  tests of MRS MSP and PSP now expect the dropped bits. The STKALIGN logic reads bit 2 only and is
  unchanged. Unicorn (QEMU) does not model this in either of its Thumb modes, so it cannot check
  it.
- `instruction_count` (also from issue 78: should a fetch fault add to it?) stays as it is. The
  manual has no counter of instructions executed: the DWT counts cycles (CYCCNT), the extra cycles
  of instructions (CPICNT), exceptions, sleep, load/store cycles and folded instructions, none of
  them instructions. So it is zmu's own bookkeeping: one per instruction that was fetched and
  decoded and begun in `execute()` (one that then faults counts), and a fetch that faults never
  became an instruction. For cmm2-core it is the budget of a run (`while count < limit`) and the
  figure in the report; a fetch fault takes an exception without advancing it, but that cannot
  spin: the handler runs counted instructions, or fetch-faults again, which escalates and then
  locks the core up, and the run loop stops on a lockup. A lockstep comparison or the JIT's oracle
  that compares by `step()` calls must not use `instruction_count` as the number of steps.
- Checked against Unicorn 2.1.5 (QEMU), a Cortex-M7 in Thumb mode: 75,800 random and boundary
  inputs, the 36 parallel adds and subtracts, the four saturating ones, the six multiplies and
  LDMDB, with registers, flags, Q, GE and memory compared: no differences. Unicorn is a local
  check only; nothing of it is in this tree.
- Rounding to an integer (FPRoundInt, FPToFixed): they now round on the bits of the value, not on
  the `BigFloat` approximation, which keeps 40 digits and so does not hold a double with a long
  mantissa exactly (a floor of such a whole number could come back one low). The rounding modes,
  ties to even and ties away included, are one `FPRounding` that the instruction or the FPSCR
  names. A tie in the FPSCR nearest mode went to the odd neighbour; it goes to the even one.
  A result that saturates now signals Invalid Operation, not Overflow. Fixed point to float scales
  by multiplying by 2^-n, because dividing by `BigFloat`'s 2^n lost digits.
- Floating-point arithmetic: add, subtract, multiply, divide, square root, fused multiply-add,
  VCMP, and the conversions between single and double and from integers now run on integers, in
  `executor/fp_real.rs`, and no longer on `BigFloat`, which I dropped. `BigFloat` has a decimal
  exponent of about 127 and 40 digits, so `1E300*10` never returned (the host looped on the
  infinity it made), `1E-130/10` was 0, and a tie or a long mantissa was rounded wrong. A value
  is kept as `sig * 2^exp` with a 128-bit `sig`; a sum, a quotient and a root drop what they have
  to into a sticky bit, and one rounding, FPRound, does the rest: all four FPSCR rounding modes,
  overflow to the largest number or infinity by mode, underflow decided before rounding, and the
  IOC, DZC, OFC, UFC, IXC and IDC flags as the manual has them. Flush-to-zero and default NaN work
  on inputs and results. VCVT between single and double used the host's `as`, with no flags, no
  NaN rules and no FZ or DN; it goes through the same unpack and round now. VSUB had its sign wrong
  for an infinite second operand (`1 - inf` was +inf). VFMS negated the second operand, and the
  manual negates the first, which shows in the sign of a NaN. The six VMOV forms between the core
  and the FPU registers ignored an IT condition that failed; the firmware's strtod has one in an
  ITEE block, and that is what made `1E-50` loop.
- FPSCR: only the bits a Cortex-M7 implements can be set (N Z C V, AHP, DN, FZ, RMode, and the
  cumulative flags IDC, IXC, UFC, OFC, DZC, IOC: mask `0xf7c0_009f`). The trap enables (bits 8-12
  and 15) are reserved and read as zero, so every write goes through `Processor::write_fpscr`, and
  exception return restores the FPSCR through it. A trap enable restored from a stack frame used to
  reach a `todo!()` in the FP exception code; that branch is gone and the cumulative flag is always
  set. Checked against DDI0403E.e A2.5.3 (pp. A2-37, A2-38): bits 31:28, 26:22, 7 and 4:0 are the
  defined ones, bits 27, 21:8 and 6:5 are reserved, which gives `0xf7c0_009f`; it was right.
- VMSR (write the FPSCR from a core register) was decoded as undefined; it runs now. VMRS with
  Rt = SP is undefined, and a VMRS in an IT block whose condition fails reports not taken.
- The three VMOV forms that stopped the host with `unimplemented!()` run now: two core registers
  to and from two consecutive single registers, and a core register to and from one 32-bit word of
  a double register (`vmov.32 d3[1], r2`). The unpredictable encodings (SP or PC as Rt or Rt2, the
  same core register twice into the core, the pair s31:s32, a register above d15) are undefined.
- `Real::add` refuses (debug assert) an addend wider than 125 bits, and the comment on `Real::mul`
  states the real limit: products of two 53-bit significands, 106 bits.
- Guest code must not panic the host. The decoder, the executor, the bus and the NVIC had `todo!()`,
  `unreachable!()`, `unwrap()` and arithmetic that overflowed on values the guest picks. Each is now
  defined; the notes below are what the guest gets. A test runs every 16-bit opcode and a million
  random ones, half of them 32-bit, through the decoder and executor on awkward register values
  (`executor/guest_never_panics_tests.rs`), and another reads and writes every address of the system
  space at every width.
  - The encodings the manual calls UNPREDICTABLE decode as UDF, as BFI with `msbit < lsbit` did.
    These are UBFX and SBFX whose field runs past bit 31, BFC with `msbit < lsbit`, and every
    instruction that would put PC in a plain destination register or write a base register back to
    PC (`decoder/unpredictable.rs`; MOV, ADD, LDR, LDM and POP, which branch, are left out).
    `Processor::set_r(PC)` used to panic; it is a branch now, which the decoder makes unreachable.
    `add_r`, `sub_r`, `add_pc` and the PC read wrap. SBFX also sign-extended from the wrong bit for
    an lsb above 0, and BFC or BFI over all 32 bits overflowed `set_bits`.
  - Addresses are modulo 2^32: an access past the top continues at 0, and a literal, branch, table
    branch, ADR, VLDR or VSTR offset that goes under 0 or over 2^32 wraps, and the vector table
    address too. A multi-byte access that starts in RAM or flash and runs past its end is a precise
    bus fault (PRECISERR, with BFAR holding the address; it used to slice out of range). Every data
    access sets BFAR this way, the multiple loads and stores, the FP ones and the table branches
    too; they called the bus directly. A lazy FP state save is not one. The alignment faults come
    first (below).
  - An unknown semihosting operation, an open with a name that is not UTF-8, and a write or a file
    name of more than 1 MiB, set r0 to -1 and carry on; the callback is not called.
    `decode_semihostcmd` returns `Ok(None)` for them. With no callback set, BKPT #0xab does not read
    the guest's arguments at all.
  - An exception return that stacks an IPSR that is not an exception the core has (Reset, a
    reserved number, or one above interrupt 239) is an invalid return (UsageFault INVPC), as one
    with a zero IPSR for a handler already was. The checks on EXC_RETURN and on the nesting come
    before the pop. The ones on the popped xPSR come after it: first the IPSR against the mode
    (the check of the return itself), then the Thumb bit (INVSTATE, which strictly only faults when
    the next instruction would execute). The pop is not undone: the registers, PC, SP and the FP
    state are the frame's when the fault is taken. The frame the fault pushes holds the popped LR
    and the popped xPSR exactly, bad IPSR included, and the handler's LR is the EXC_RETURN that was
    refused. That IPSR is never made the current one (so it never holds an exception that does not
    exist, and handing the same EXC_RETURN back is refused again), and what the refused return
    gives the entry travels with the fault, so a lockup or a reset leaves nothing behind. Whether
    the core locks up is decided from the execution priority, not from an IPSR. The exceptions the
    core lacks have no state: the registers that name them read as zero and ignore writes.
    Checked against ExceptionReturn() in DDI0403E.e B1.5.8 (p. B1-541), PopStack() and DeActivate()
    (p. B1-542), PushStack() and ExceptionTaken() in B1.5.6 (pp. B1-532, B1-533). What was wrong or
    missing, and is changed:
    - A return refused before the pop (the handler is not active, EXC_RETURN is not one of 0x1,
      0x9, 0xd in its low nibble, or a Thread return with other exceptions active) enters the
      fault with no PushStack: the stack pointer is unchanged and nothing is stacked, and LR =
      '1111':EXC_RETURN. I used to push a frame there. That reading was right.
    - After the pop, a bad IPSR is a PushStack to negate the PopStack, then LR = '1111':EXC_RETURN
      and ExceptionTaken, so the handler's LR is the refused EXC_RETURN and the frame holds the
      popped registers and xPSR. That is what was implemented; my other recollection (LR set before
      ExceptionTaken, so that the frame stacks it) was wrong.
    - A return to Handler mode has no nesting check in the manual (only Thread mode does:
      `NestedActivation != 1 && CCR.NONBASETHRDENA == '0'`). I refused it when only one exception
      was active; now the popped IPSR is the only check (zero is refused, after the pop). On
      silicon this case may be refused anyway, but the pseudocode does not say so.
    - CCR.NONBASETHRDENA (bit 0, which the CCR already stored) is honoured: it allows the Thread
      return with other exceptions active. ARMv6-M has no such bit, so on an armv6m build the CCR
      does not store it and it is never honoured.
    - ExceptionTaken() clears EPSR.IT (p. B1-533); only PushStack did, so a return refused before
      the pop (no frame) left the IT state live in the fault handler. It is cleared on entry now.
    - PushStack stacks `frameptralign = SP<2> AND forcealign` for the main stack as well; I had
      left out the AND for MSP, so with CCR.STKALIGN clear bit 9 of the stacked xPSR could be set.
    - Kept, and not in the manual: an IPSR above interrupt 239, Reset (1) or a reserved number
      (7-10, 13) in the popped xPSR for a return to Handler mode is refused as INVPC, because the
      core has no state for such an exception. The manual lets any value through. The INVSTATE check
      (EPSR.T = 0) is made at the return, where the manual makes it when the next instruction
      executes. Sleep-on-exit tests the nesting after DeActivate (the manual's text tests
      NestedActivation, counted before it, against 0, which can never be true).
  - A synchronous fault is taken as itself when it is enabled and its priority is higher than the
    execution priority (so PRIMASK, BASEPRI and FAULTMASK count), and is otherwise escalated to
    HardFault. A HardFault at an execution priority below 0 (HardFault or NMI running, or FAULTMASK)
    is a lockup. Checked against ExecutionPriority() (DDI0403E.e B1.5.4, pp. B1-528, B1-529:
    PRIGROUP applied to the active exceptions and to BASEPRI, PRIMASK 0, FAULTMASK -1), Priority
    escalation (p. B1-529: a synchronous fault or SVC whose group priority is not higher than the
    execution priority, or a disabled configurable fault) and B1.5.15 (pp. B1-555, B1-556: lockup is
    a fault at a priority below 0, HardFault, NMI or FAULTMASK); the code agrees with them. A
    priority that is not a group priority compares the same, since the execution priority is always
    a multiple of the group size. It used to be escalated only when disabled, and a lockup only when
    the IPSR said HardFault or NMI. The exception reported as running is the one in the IPSR when it
    is active, and otherwise the active one of the highest group priority, the lower number among
    equals.
  - `decode_imm_shift` reads the two type bits (it read three and had a `todo!()`); no decoder
    passed a third bit.
  - A D register number above 15 is UNDEFINED, because the FPU has 16 doubles (FPv5-D16), and used
    to alias onto D0-D15. `DoubleReg::new` returns `None` for it, and the VFP decoders share their
    operand reading (`decoder/vfp_operands.rs`). A VLDM, VSTM, VPUSH or VPOP register list that is
    empty or runs past D15 (or S31) is UDF; the single-register forms overflowed a `u8` for a long
    list. VPUSH and VPOP of single registers took the first register as D:Vd and not Vd:D, so `vpush
    {s16-s31}` was undefined.
  - NVIC: the registers have room for 512 interrupts and the core has 240. The ones past it read
    zero and ignore writes (STIR too). Reading an IPR byte used the word index and not the byte
    address.
  - A failing ITM trace file is reported once through `log::warn!`, and stops the tracing; the guest
    runs on. It used to unwrap.
  - Not reachable by the guest, left as they are: the `expect`s in `exec_vcvt` (the decoder makes
    the register kinds match the size), `satq`'s size assert, the shift asserts in `operation.rs`
    (`shift_c` has dealt with 0 and RRX before them), `exception_return`'s mode assert (only
    `bx_write_pc` calls it, in handler mode), the `unwrap`s in `exception_taken` and `deactivate`
    (the exception is one of the table's, or was checked when it was stacked), and the GDB server's
    `unwrap`s (the debugger, not the guest).
- UsageFault stacked the faulting address less 4 (upstream's `return_address - 4`), so a handler
  that returned went back four bytes too early. Every fault stacks the address of the instruction
  that caused it, SVC stacks the next one, and an interrupt taken between two instructions stacks
  the one that has not run; the caller of `exception_entry` says which, and `push_stack` stacks
  what it is given.
- ADR.W towards lower addresses (`adr.w r1, #-8`, encoding T2) was decoded as undefined; it runs
  now. VLDM did not check that the FPU is enabled (CPACR), so it ran with the FPU off and skipped
  the lazy state save; it faults with NOCP like the other FP loads and stores. The disassembly
  printed `s10` to `s16` for D10 to D15, and ADR printed its offset with a stray `#`.
- Memory hints: LDRH and LDRSH with Rt = PC in the immediate (positive or negative), register and
  literal forms are the unallocated hints of the manual and run as no-ops, and so does PLI in all
  its forms (it decoded as undefined). The forms with write back stay undefined. PLD with PC as the
  base is the literal form (it decoded as the register form with Rn = PC).
- Literal loads: with Rn = PC the byte and halfword loads (LDRB, LDRSB, LDRH, LDRSH) in the register
  and unprivileged encodings (and LDRT) are the literal forms too, U is bit 23 and the offset is
  bits 11:0, where they decoded as the register form with PC as the base, or as undefined. LDRH,
  LDRSH, LDRB and LDRSB literal were undefined altogether; they run now (as the immediate form with
  PC as the base, which is Align(PC, 4)).
- The 8-bit offset forms of LDR, LDRB, LDRH, LDRSB, LDRSH, STR, STRB, STRH, LDRD and STRD with P = 0
  and W = 0 are undefined; they ran as an offset-less access.
- Registers the manual calls UNPREDICTABLE are undefined, per encoding: SP as Rt of LDRB, LDRH,
  LDRSB and LDRSH (every addressing form, the literal one too); SP or PC as Rt of STRB and STRH; PC
  as Rt of STR (SP may be stored, and LDR may load SP or PC); SP or PC as Rt or Rt2 of LDRD and
  STRD, and one register twice in LDRD; SP as Rt of LDREX, LDREXB and LDREXH; SP or PC as Rd or Rt
  of STREX, STREXB and STREXH. LDRD literal (it was undefined) is LDRD with PC as the base, for P =
  1 and W = 0.
- The decoder's pattern table is ordered by the number of fixed bits and then by the pattern, no
  longer by the order a hash map gave. Patterns with as many fixed bits as each other that overlap
  have to be listed in `RESOLVED_OVERLAPS` in `build.rs`, winner first, or the build stops. There
  are three: MCRR over VSTM (T2, P = U = W = 0), LDC literal over VLDM (T2, Rn = PC), and
  ADD SP, SP, SP as T1 and not T2. The LDRB register pattern took 00000 in bits 11:6 and not
  000000, so it decoded undefined encodings as LDRB.
- DebugMonitor is exception 12 (vector offset 0x30) and 10 is reserved; upstream had them the
  other way round.
- `get_r`, `set_r`, `add_r` and `sub_r` keep upstream's shape (r0-r12 by index first), because a
  match on the enum cost about 5% of the firmware run; the last arm is PC, so they cannot panic.
- More encodings the manual calls UNDEFINED or UNPREDICTABLE are undefined here: VLDM and VSTM
  with P = U (any W); STREX, STREXB and STREXH with Rd = Rt or Rd = Rn; the exclusives with Rn = PC;
  the stores (immediate and register) with Rn = PC, and STRD with Rn = PC; write back with the base
  also a destination for LDRD and STRD, and for the immediate loads and stores (Rt = Rn).
- The pattern tables are checked at build time: a pattern listed twice (same name or not) stops the
  build, and the order that the resolved overlaps ask for is made by a topological sort over all
  of them at once, which is asserted after it. (SBFX was in the table twice.)
- SVC is taken as SVCall only when SVCall can preempt (its priority is higher than the execution
  priority, so PRIMASK, BASEPRI, FAULTMASK and a running handler count); otherwise it is a forced
  HardFault, and a lockup below priority 0 (B1.5.4 Priority escalation, p. B1-529, whose example is
  an SVC with PRIMASK set). An SVC escalated to HardFault keeps the return address of SVCall, the
  next instruction (ReturnAddress(), p. B1-534: "a fault that is escalated ... retains the
  ReturnAddress() value of the original fault"; the note is repeated at p. B1-529); I stacked the
  SVC itself. The IT state an SVC stacks (escalated or not) is the one after the SVC, to match
  that return address (the note in ReturnAddress(), p. B1-534); it was the state before. If the
  entry of an exception fails while stacking, it is a derived exception on exception entry and
  follows DerivedLateArrival() (B1.5.11, pp. B1-547, B1-548) and PushStack (B1.5.6, p. B1-532):
  - the derived exception reuses the one PushStack: SP is lowered once, the frame is the original's
    (its return address, and its IT state, which ExceptionTaken() clears and PushStack does not),
    and no second frame is pushed. Before, the derived fault pushed a frame of its own below the
    first, with the wrong IT state and, for SVC, the SVC's own address.
  - the derived exception is the stacking fault (BusFault STKERR, MemManage MSTKERR) delivered by
    the escalation rules: it is escalated to HardFault when it is disabled or when the preempted
    priority is not lower than its own. If it is escalated, or its GROUP priority (PRIGROUP
    applied, as DerivedLateArrival() compares OEpriority and DEpriority) is higher than the
    original's, it is taken and the original stays pending; otherwise (the same group or lower)
    the original is taken on the frame and the derived exception is left pending.
  - which exception is left pending: an original that came from the pending exceptions
    (`check_exceptions`, so also an interrupt, a fault pended through SHCSR, a MON_PEND
    DebugMonitor) is always left pending, as the manual has it. A synchronous original that is
    entered now (an SVC, a BKPT, a fault from `handle_fault`) is left pending only when it
    returns to the next instruction (SVCall, and the like): a fault or a BKPT returns to the
    instruction itself, which runs again, so the manual's unconditional SetPending(OEnumber) would
    run it twice. That is a deliberate departure from the pseudocode. The ignoring of a derived
    DebugMonitor (a watchpoint) does not arise, there are no watchpoints.
  - a derived HardFault at an execution priority below 0 is a lockup (B1.5.15, p. B1-556): the
    core stops with a Lockup trap.
  - a vector read error (of the original, or of the derived exception that is taken) is always a
    HardFault (B1-547), on the same frame, with the fault VectorTable (HFSR.VECTTBL, also when the
    core locks up: B1-555 has the Fault Status Registers consistent with the fault that caused
    the lockup). The exception whose vector failed is left pending by the rules above. It is a
    lockup only where the manual has one: for NMI and HardFault (B1-556), when the execution
    priority is below 0 (B1.5.15), or when the HardFault's own vector cannot be read either; the
    lockup trap names the vector read (fault VectorTable). The NMI and HardFault cases come out
    of the priority rule and are not special-cased: the derived HardFault (-1) is not higher than
    the original (-2, -1), so the original is taken first and its vector fails, and the HardFault
    is left pending at the lockup. (An earlier version made every vector failure after a stacking
    failure a lockup.) Before, the frame was pushed and the exception cleared and never taken, or a
    second frame was pushed.
  - the trap is for the exception that was taken, by the fault trap mode, with the fault that
    made it (the original's, or the stacking fault when the derived one is taken), or else for the
    derived one by the mode. It reports the PC of the faulting instruction (the SVC itself for an
    SVC), not the return address. Nothing from one entry is kept for the next one: the entry
    returns how it ended.
  - an exception selected by `check_exceptions` counts as that step's exception entry, whether the
    entry worked or failed; the GDB step after a halt (a BKPT, or a breakpoint or watchpoint from
    the debugger's list: C1-695 applies whenever the core leaves Debug state) takes it instead of an
    instruction. Every path that enters an exception counts the 12 cycles of an entry
    (`EXCEPTION_ENTRY_CYCLES`) in the cycle counter, DWT and SysTick: SVC and faults as before, and
    now an exception taken after an instruction or from sleep (it counted 0), and the after-halt
    entry. 12 is the interrupt latency ARM gives for the Cortex-M7 with zero-wait-state memory (the
    same as the Cortex-M3 and M4); wait states in the real memories add to it. An exception taken
    straight after an exception return in the same step is tail-chained and counts 6 cycles
    (`EXCEPTION_TAIL_CHAIN_CYCLES`). 6 is the figure ARM gives for the Cortex-M3 and M4 (their TRMs
    and the Generic User Guides); I have not found an authoritative figure for the Cortex-M7 and do
    not have its TRM, so it is the M3/M4 one, unverified for the M7. The emulator still does the
    unstacking and stacking, only the count is the tail-chain's. A lockup enters nothing, and is a
    state of the core (`Processor::locked_up`, `is_locked_up()`, set by `lock_up()`; it is not
    inferred from `running`, which a semihosting exit also clears). On every path that finds one (a
    fault, the entry of an SVC or BKPT, and an exception taken after an instruction, from sleep,
    after a halt or after an exception return, where it is not a tail-chain either) the step counts
    the one cycle of the fetch of the lockup address (B1.5.15) and the core stops, with the priority
    of the lockup (-1 at least). While it is locked up `step()` executes nothing and takes no
    exception except an NMI, or a pending reset (B1-555): each further step counts the 1 cycle, and
    re-queues no trap; an NMI whose entry works leaves the lockup (this was broken before: an NMI in
    a HardFault lockup was never taken) and counts a full entry. A lockup from a failed stacking or
    vector read sets PC = 0xFFFFFFFE, the lockup address of B1-556; one from an instruction keeps
    the PC of that instruction. A pending Reset whose initial SP and PC cannot be read is a lockup
    at priority -1 at that address (B1-556), reported as one, with HFSR.VECTTBL and no FORCED, and
    counts 1. (`Processor:: entry_cycles` is the one place that decides what a step counts.) Lockup
    priorities follow B1-556 to B1-558: a vector read error at NMI entry locks up at -2 whatever the
    running priority, at HardFault entry and at reset at -1; INVPC on a return from the NMI handler
    to an execution priority of -1 at -2; a stacking failure on NMI entry at -1 or -2
    (IMPLEMENTATION DEFINED, -2 here, so that the NMI is not entered again); the rest at the
    priority the core runs at. `lock_up(priority)` sets it. What leaves a lockup is what can preempt
    its priority: an NMI from one at -1 (B1-555 names the HardFault handler; FAULTMASK and the reset
    handler are at -1 as well), a reset from either, nothing from one at -2 but a reset
    (`lockup_can_end()` says so; that an NMI ends any lockup at -1 is my extrapolation from the
    priorities: B1-555 itself names only the HardFault-handler case; cmm2-core's `run_loop` steps a
    lockup that can end and stops with a fault ("core locked up") otherwise; the machine pends no
    NMI today). The lockup address is 0xFFFFFFFE for a failed stacking or vector read, INVPC and a
    bus or MemManage fault on unstacking (the PC is set to it: `bx lr` is where it was before), and
    the PC of the instruction otherwise. A reset (`reset()`) leaves the lockup and sets `running`
    again, so the GDB `monitor reset` recovers a locked core. A pending Reset taken from lockup (or
    at all) is charged the 12 cycles of an entry, also straight after an exception return (a reset
    clears the tail-chain flag): it is not an exception entry, and I have no figure for the reset
    sequence, so this is an approximation. A pending Reset resets the core only, not the peripherals
    and interrupt lines around it, which an embedder's reset (cmm2's `Cpu::reset`) does; nothing
    pends Reset today. VTOR is reset to `Processor::boot_address` (zero by default; INITVTOR on the
    Cortex-M7, set from the BOOT_ADD pins on the STM32H7) by every `reset()`, as ResetSCSRegs resets
    it; cmm2-core sets it to the flash base instead of writing VTOR before each reset. A reset that
    cannot read its vectors has still reset the exceptions first (TakeReset reads them after), so
    nothing stays active. A sleeping step counts its cycle in `cycle_count` and SysTick (which has
    to wake the core), and not in the DWT cycle counter unless `Processor::dwt_runs_in_sleep` is set
    (default off): on the STM32H7 the CPU clock is stopped in CSleep (RM0433 6.7.5) and rcc_c_ck,
    which clocks the DWT, runs only in CRun (8.5.12), so CYCCNT does not count through a WFI sleep
    unless DBGMCU_CR.DBGSLEEP_D1 is set; the flag is that bit. The ARMv7-M manual says nothing about
    the cycle counter in sleep. cmm2 does not call `step_sleep`: a WFI runs there as a 1-cycle
    instruction (a separate issue), so this changes nothing for it. A GDB `continue` on a locked
    core spins, stepping the 1-cycle fetch and reporting nothing further, as the hardware core does;
    it used to report Finalized. The GDB simulation steps a core with running and sleeping both set
    (what a WFI leaves, as in `system/simulation.rs`) with `step_sleep`; it used to run its
    instructions.
  - the HardFault-vector-fails-too branch is reached when the vector words of the exception to take
    and of HardFault are both unreadable; the memory between them may be mapped (with VTOR at
    0x1fff_ff80, vectors 3 and 11, at 0x1fff_ff8c and 0x1fff_ffac, are unmapped, and vector 32 at
    0x2000_0000 is not). The test `test_an_original_and_a_hardfault_with_unreadable_vectors_lock_up`
    does that.
    The `is_lockup(HardFault)` test next to it is the manual's rule (B1.5.15), but no exception
    but NMI can be taken at a priority below 0, and NMI is handled by the first part of the
    condition, so no test reaches it.
  - CFSR.DIVBYZERO (bit 25), UNALIGNED (24) and NOCP (19) are recorded by `record_fault_status`
    (issue 67; the UFSR bits of B3.2.15), also when UsageFault is disabled and the fault is
    escalated to HardFault (FORCED): B1.5.14 (p. B1-551) has the status bit set in every case.
    DIVBYZERO is raised by SDIV and UDIV with CCR.DIV_0_TRP and NOCP by an FP instruction with
    the coprocessors off. The bits are set in that one place, from the fault: `check_vfp_enabled`
    used to set NOCP itself as well, and does not now; every FP instruction that raises
    `Fault::Nocp` (the VLDM and VSTM forms too, and those entered from exception entry and
    return) reaches it through the fault. UNALIGNED is raised by the alignment checks below.
  - with FPCA set, a failed stacking leaves an extended frame whose FP registers, and
    FPCCR.LSPACT, were never written. The manual allows the stacking to be abandoned (the note to
    PushStack, p. B1-532), and the contents of what was not stacked are UNKNOWN; the handler does
    not get them from the emulator either.
  - under `FaultTrapMode::none()` a failed entry no longer reports a trap, as it used to
    (always, and it stopped the core): the core carries on in the derived handler, and a trap is
    queued only by the mode, or for a lockup, like any other fault.
  - VTOR keeps bits 31:7 (TBLOFF, B3.2.5), and the vector table address is VTOR<31:7>:'0000000'
    (B1.5.6, p. B1-533): a write masks the low seven bits. The tests that used unaligned VTORs use
    a short vector table in flash instead, so that the wanted vectors cannot be read.
  - the test `test_failing_stacks_and_unreadable_vectors_never_panic_and_lower_sp_by_one_frame` runs
    random stacks (MSP, PSP, SPSEL, FPCA) near the end of SRAM, vector tables of random length,
    aligned VTORs, priorities, PRIGROUP, fault enables and pending exceptions, and checks that no
    entry lowers the stack it uses by more than its own frame (0x24, or 0x6c with FPCA). BKPT with
    no semihosting host is a debug event (C1.5, p. C1-694; B1.5.14, HardFault on breakpoint
    escalation): DFSR.BKPT is set, the DebugMonitor is taken if DEMCR.MON_EN is set and its group
    priority is higher than the execution priority, and otherwise it is a HardFault with
    HFSR.DEBUGEVT, a lockup below priority 0. A debug event that generates the DebugMonitor sets
    DEMCR.MON_PEND (C1-694), and MON_PEND stays set until the exception is taken or a write clears
    it (C1-689); the BKPT takes the DebugMonitor at once, so the code does not set the bit for it
    and it reads 0 afterwards. A software write of MON_PEND = 1 to DEMCR pends the DebugMonitor
    whatever MON_EN is (C1-689): it is taken by the usual priority rules, taking it clears MON_PEND
    and the pending state (also when a BKPT takes it directly), and a write of 0 removes the pending
    exception. Whether a write pends is decided by the pending state of the exception, not by the
    stored bit. The return address is the BKPT itself for both (ReturnAddress(), synchronous). DFSR
    bits are set when the event halts or generates an exception (C1-694), so DFSR.BKPT is set on the
    HardFault path too; only whether they are set for an event that is ignored or causes lockup is
    IMPLEMENTATION DEFINED. BKPT #0xab with no host is the same. ARMv6-M has no DebugMonitor
    (exception 12) and no DEMCR, but the core stores every bit of DEMCR, so on an armv6m build BKPT
    never enters exception 12 now, and a MON_PEND write pends nothing. With a GDB server attached, a
    BKPT (not #0xab when a semihosting function is installed, which it is for the servers built by
    `GdbServer::new`; with none, #0xab halts too) halts into the debugger: the simulation reports a
    breakpoint stop and DFSR.BKPT is set, and the PC stays on the BKPT, as it does when silicon
    halts in Debug state. A plain continue or step from there hits the same BKPT again, as it does
    on hardware; the debugger has to move the PC past it (GDB's `set $pc += 2`, which the server
    supports as a register write), and then continue and step go on. A BKPT that halts does not
    advance the IT state, as it does not move the PC, and takes no pending exception in that step
    (the debugger sees the PC on the BKPT; the exception stays pending). The halt is consumed when
    the simulation reports it. The step after a halt starts by taking a pending exception of
    sufficient priority, and does not execute an instruction in that step (Halting debug stepping,
    C1.5, p. C1-695), so a continue with the PC still on the BKPT takes it instead of halting again,
    and with PendSV pending the step after `set $pc` takes it before the next instruction runs. It
    used to be a HardFault under the server. A stacking failure on the entry of an exception is a
    derived exception (above), a lockup when the execution priority is below 0, not by the IPSR. The
    xPSR that a refused return stacks holds only the fields the xPSR has (reserved bits 23:20, and
    GE without the DSP extension, are zero).
- CCR.STKALIGN (B3.2, p. B3-605; B1.5.7, p. B1-535): whether the bit is RW or RO (RAO) and its reset
  value are IMPLEMENTATION DEFINED, and Arm recommends a reset of 1. On the armv7em build (the
  Cortex-M7) the CCR now resets with STKALIGN = 1; it stays writable. That the real M7 resets it
  to 1 (and whether it is RW or RAO) is from my memory of the Cortex-M7 TRM, which I do not have:
  unverified. Other builds still reset the CCR to 0.
- The 16-bit STM decoded only seven bits of its register list (`0b111_1111`), dropping r7 from every
  `stmia Rn!, {..., r7}` (`{r4-r7}` stored three words and wrote back 12). A7.7.159 T1 (p. A7-383)
  has an 8-bit list; fixed. The other list decoders (LDM T1, PUSH T1 with LR, POP T1 with PC, the
  32-bit forms) were checked and are right. LDM.W with fewer than two registers, or with write back
  and the base in the list (A7.7.41 T2), is UDF in `decode_LDM_t2`, and has a test. STM.W and STMDB
  cannot have SP or PC in the list (the decode patterns have 0 in bits 15 and 13).
- VCVT between floating-point and fixed-point: checked against A7.7.229 (p. A7-463, A7-464) and
  FixedToFP() (A2.5, FP conversions, pp. A2-58, A2-59): fixed to float rounds to nearest whatever
  the FPSCR says (`round_nearest = TRUE`), float to fixed rounds towards zero; the integer VCVT
  (A7.7.228) rounds by the FPSCR for integer to float (`round_nearest = FALSE`) and towards zero
  unless R is given. The code did that already.
- Checked against the encoding diagrams and decode pseudocode, per row of `decoder/unpredictable.rs`
  (LDR A7.7.43 and .45, LDRB .46 to .48, LDRD .50 and .51, LDREX* .52 to .54, LDRH .55 to .57,
  LDRSB .59 to .61, LDRSH .63 to .65, STR .161 and .162, STRB .163 and .164, STRD .166, STREX* .167
  to .169, STRH .170 and .171, pp. A7-246 to A7-399): the rows I had were right, and these were
  missing and are now undefined (UNPREDICTABLE in the manual): SP or PC as the offset register Rm
  of the register forms of LDR, LDRB, LDRH, LDRSB, LDRSH, STR, STRB and STRH; PUSH of a single
  register Rt = SP or PC and POP of Rt = SP (A7.7.101 T3, A7.7.99 T3; POP {PC} stays a branch);
  TBB and TBH with Rn = SP, or Rm = SP or PC (A7.7.185, p. A7-416; Rn = PC is the usual form); the
  32-bit PUSH and POP of fewer than two registers (T2) and POP of PC and LR together; LDM and POP
  with PC and LR together, LDM, STM and STMDB with Rn = PC, STM and STMDB with fewer than two
  registers, and STM and STMDB with write back and the base in the list (A7.7.41, .99, .101, .159,
  .160, pp. A7-242, A7-319, A7-322, A7-383, A7-385); and the 16-bit PUSH, POP, LDM and STM with an
  empty list. The manual makes SP as a destination UNPREDICTABLE for many data-processing encodings
  too (for example UXTB.W, MUL); those still execute as a write to SP, which cannot hurt the host.
  The UNPREDICTABLE conditions that depend on the IT state (PC in a load or POP outside the last
  instruction of an IT block, TBB in an IT block) are not checked. LDMDB is still decoded as
  undefined, as upstream has it.
- SMULL and SDIV (`executor/signed_multiply.rs`, `executor/divide.rs`): SMULL zero-extended its two
  operands, so it was an unsigned multiply (SMLAL already sign-extended); it now sign-extends both,
  as A7.7.136 says (issue 64). SDIV of i32::MIN by -1 panicked on the host; it now wraps to
  i32::MIN, as the manual's result for that case is. Tests added for both.
- GDB: `GdbServer`, `ZmuTarget` and `Simulation` can be built from a processor that has already
  been set up and reset (`from_processor`). Both mark the processor as having a debugger attached.
- IT state: the IF-THEN state is now saved in the stacked xPSR on exception entry and restored on
  return. It was dropped, so an interrupt inside an IT block corrupted the handler.
- Default features: `default` is now `["armv7em", "fpv5-d16"]`, the Cortex-M7 with the
  double-precision FPU. All other features are unchanged.

The armv7m build needs `--no-default-features --features armv7m`: with the default features
also on, `build.rs` stops.

- Alignment faults (issue 68; A3.2.1, B3.2.8, B2.3.5, B1.5.14): the core did not check alignment at
  all. The data accesses that always fault on an address that is not aligned, whatever
  CCR.UNALIGN_TRP says, now do: LDM, STM and STMDB, PUSH and POP (except the single-register 32-bit
  forms, which are STR and LDR with write back, A7.7.99 T3 and A7.7.101 T3), LDRD, STRD, LDREX and
  STREX (word), LDREXH and STREXH (halfword), VLDR, VSTR, VLDM, VSTM, VPUSH and VPOP (word). LDR,
  STR, LDRH, LDRSH, STRH and TBH fault only with CCR.UNALIGN_TRP set (bit 3; always on an armv6m
  build, which has no unaligned accesses); the byte accesses never do. The fault is a UsageFault
  with UFSR.UNALIGNED, raised before any access or write back (a refused POP leaves SP alone), and
  escalated to HardFault (FORCED) when UsageFault is disabled, the status bit being set either
  way. The check is in `read16_data`, `read32_data`, `write16_data` and `write32_data` (the
  UNALIGN_TRP rule: one AND and a branch on the address) and, for the always-aligned instructions,
  `require_aligned` at the start of each. The unprivileged forms (LDRT, STRT and so on) are decoded
  as the plain ones here and follow the same rules. Accesses to Device and Strongly-ordered memory
  must be aligned, and are UNPREDICTABLE otherwise (A3.2.1, A3-83); the core has no memory types
  (no MPU attributes), so that is not modelled. The test that checks BFAR for every kind of data
  access now runs the aligned-only instructions off the end of RAM (a MemManage fault, MMFAR)
  instead of straddling it, which an aligned word cannot do. Cost: the real firmware's 50
  million instruction BASIC run took 17.8 s with the checks and 17.3 s without (three runs each,
  ±1 s of noise), so none that shows. The real-firmware tests all pass: nothing in the firmware
  faults on alignment.

- Batch 4 of issue 77, the base instructions that were missing or decoded wrongly. Each entry says
  what was checked independently and what that could not cover.
- CLREX (A7.7.23) was hardwired to UDF; it is `Instruction::CLREX` now, and it opens the local
  exclusive monitor. The monitor had no state at all (`exclusive_monitors_pass` always said yes, so
  every STREX stored and returned 0). It is now Figure A3-6 of the manual (`core/monitor.rs`,
  `Processor::exclusive_tag`): a load-exclusive tags the word of its address and goes to Exclusive
  Access; a store-exclusive stores and returns 0 only if the monitor is in Exclusive Access and its
  address is in the tagged word, otherwise it stores nothing and returns 1, and either way the
  monitor ends Open Access (Table A3-2). CLREX, exception entry (`ExceptionTaken`, B1.5.6),
  exception return (B1.5.8) and reset (B1.5.5) open it. What the manual leaves IMPLEMENTATION
  DEFINED and I chose: the tagged block is one word (a = 2, A3.4.3), whatever the size of the load,
  so a STREXB to another byte of the tagged word passes; a store-exclusive to another word fails and
  closes the monitor; an ordinary store, to the tagged word or not, does not touch the monitor
  (Table A3-2, Store(t)); a size that does not match the load's is not tracked. The Cortex-M7 TRM
  was not available to say which options the chip takes. Unicorn (QEMU, Cortex-M7, Thumb mode)
  agreed on 24,000 random sequences of 2 to 10 load-exclusives, store-exclusives and CLREX on
  matched widths (8,000 each of byte, halfword, word, no mismatch). It differs in the choices above:
  it tags the exact address and also compares the memory value at the store, so it fails a
  STREXB/STREXH to another byte or halfword of the tagged word (502 of 3,000 byte and 527 of 3,000
  halfword sequences that had an alias) and fails a STREX after an ordinary store of a different
  value to the tagged word (about 1% of 3,000 sequences per width). It cannot check exception entry
  and return or reset (the plain Thumb mode has no exceptions of the M-profile), which the unit
  tests check. The firmware has 68 exclusive instructions (the atomic bit set and clear loops of its
  HAL); a failing STREX there only makes the loop retry.
- The hint space (A5.3.4, Table A5-14, A7.7.31, A7.7.32): CSDB and every unallocated hint decoded as
  B T3 with a condition of 111x (a taken branch with a garbage offset), and `dbg #0` as UDF. One
  decoder now (`decoder/hint.rs`) reads op1 (hw2<10:8>, not 000 is UNDEFINED) and op2: NOP, YIELD,
  WFE, WFI, SEV, CSDB (`Instruction::CSDB`), DBG #0 to #15 (`Instruction::DBG`), and every other op2
  executes as a NOP. CSDB only orders speculation of data values and of the flags, which this core
  does not do, so it has no effect (A7.7.31 makes it UNPREDICTABLE inside an IT block; it executes
  as a NOP there too). B T3 refuses a condition of 111x (Table A5-13: a conditional branch has op =
  not x111xxx), so no hint, MSR, MRS or barrier encoding can decode as a branch, and a hw1 with that
  condition that is none of them is UDF. The (0) bits hw2<13> and hw2<11> of the diagrams are not
  tested (the hint space of the table has them free); the hint space with them set is 768 opcodes,
  of which llvm 18 and 21 reject 756 outright and soft-fail 12. hw1 0xF3A0 to 0xF3AE (the (1) bits
  of hw1 clear) are UDF through the B T3 guard while hw2 with the (0) bits set execute as hints;
  both are UNPREDICTABLE, so both are allowed. llvm decodes op2 0x0d, 0x0f, 0x1d, 0x2d as PACBTI,
  BTI, PAC and AUT (Armv8.1-M), which are unallocated hints, NOPs, on Armv7-M. Unicorn: 10,120
  vectors (every op2 but YIELD, WFE and WFI, which stop its execution loop; 40 each) with random
  registers and flags, no mismatch. Swept against llvm-mc: hw1 0xF3AF with all 65,536 hw2, and
  0xF3A0 to 0xF3AE with hw2 0x8000 to 0x80FF: 69,376 opcodes; no difference but the ones above, the
  opcodes llvm calls UNPREDICTABLE (hw1 with the should-be-one bits clear, which are UDF here), and
  hw2 with bit 15 clear, which are other instructions (USAT and USAT16 with Rn = PC).
- LDRT, LDRBT, LDRHT, LDRSBT, LDRSHT (UDF unless Rn = PC) and STRT, STRBT, STRHT (decoded as STR
  immediate T4 and its byte and halfword forms, which lost the unprivileged access) are
  `Instruction::UNPRIVILEGED` (`decoder/unprivileged.rs`, `executor/load_and_store.rs`): address =
  Rn + imm8, no write back. Rn = PC is the literal form of the load of that width (the pages say SEE
  it) and UNDEFINED for a store; Rt = SP or PC is UNPREDICTABLE and decodes as UDF. zmu has no MPU
  model: `peripheral/mpu.rs` is a stub whose `validate_address` accepts every address. The access
  goes through it with `AccType::UnPriv` (as the FP state saving already does) and is then the
  ordinary access (a real MPU needs the access type passed down to the memory access, as
  `fp_generic.rs` does for lazy stacking, because the UNALIGN_TRP UsageFault comes before the MPU
  check and an access crossing a region is checked in each part; that is for when it exists), which
  is what the architecture gives in Thread mode with nPRIV set and everywhere without an MPU. The
  rule the MPU must follow when it exists: an access made by these eight instructions is checked
  against the permissions of an unprivileged access whatever the mode (CheckPermission, ispriv =
  acctype != AccType_UNPRIV && FindPriv(), B2.3.8), and the MPU fault is MemManage; the default
  memory map applies the same (B3.5, B3.5.4). Unicorn (plain Thumb mode has no privilege split, so
  these are the ordinary accesses): 160,000 vectors, 20,000 of each of the eight, random base
  (aligned or not), offset (0 and 255 weighted), registers (SP as base too), flags and memory;
  registers, flags and a checksum of the memory matched in all. Swept against llvm-mc: hw1 0xF800 to
  0xF9FF with hw2 bits 11:8 = 1110, all Rt and imm8: 2,097,152 opcodes. All 430,080 decodes as one
  of the eight agree with llvm on name, Rt, Rn and offset; the rest of the space is the same
  instructions as before (the other forms of the loads and stores, checked by comparing the decode
  to the build before the change: only these eight forms and the 3,840 STRT encodings with Rt = SP,
  which were STR, changed).
- Coprocessor instructions (A5.3.18, Table A5-30). The table had MCR with hw2<4> clear, which is
  CDP, so CDP and CDP2 decoded as a no-op MCR, real MCR as UDF, and VMLA and VMLS (not VNMLA and
  VNMLS, which already fell into CDP and were UDF) as a silent no-op MCR; LDC and LDC2 (immediate)
  were silent no-ops; MRC, MRRC, MCRR, STC, LDC (literal) and CDP were UDF. All 14 kinds
  (`decoder/coproc.rs`, `Instruction::COPROC`) now raise `Fault::Nocp` (UsageFault with UFSR.NOCP,
  recorded in `record_fault_status` as before) for any coprocessor number but 10 and 11. The manual:
  only CP10 and CP11 are implemented on Armv7-M (B3.2.20), the CPACR here keeps only those two
  fields (the rest would read back zero, which is how software tests for a coprocessor), and the
  Cortex-M7 has no other. A coprocessor instruction for a coprocessor that does not exist or is not
  enabled takes NOCP (A5.3.18 note); I did not find a Cortex-M7 statement about CP0 to CP7 beyond
  that, and the TRM was not available. CP10 and CP11 with the FPU built in exist: the FP
  instructions are decoded first, and any other encoding for those numbers decodes as `COPROC` too,
  and the executor looks at CPACR (A2.6, A5.3.18): UFSR.UNDEFINSTR with the FPU enabled, NOCP with
  it disabled (`exec_coprocessor`). VMLA, VMLS, VNMLA and VNMLS were such encodings until FP batch 5
  added them (the VMLA and VMLS half had been a silent no-op). An encoding
  inside a pattern that an FP decoder owns and refuses (for example VLDM with P = 0, U = 0, W = 1)
  is still UDF whatever CPACR says, as before. Without the FPU (the armv7m and armv6m builds) CP10
  and CP11 are absent coprocessors like the rest and take NOCP; one existing test
  (`test_resolved_overlaps_decode_as_resolved`) expected MCRR p10 to be UDF and now expects a
  coprocessor instruction. The encodings the pages call UNDEFINED or UNPREDICTABLE decode as UDF:
  LDC and STC with P = U = W = 0, op1 11xxxx, MCR with Rt = SP or PC, MRC with Rt = SP, MCRR and
  MRRC with SP or PC in either (and MRRC with Rt = Rt2), STC with Rn = PC, LDC literal with W = 1 or
  P = 0 (the architecture raises NOCP before anything else for an absent coprocessor, but
  UNPREDICTABLE allows UDF). Swept against llvm-mc: all 2,048 hw1 of 0xEC00 to 0xEFFF and 0xFC00 to
  0xFFFF, all 16 coprocessor numbers, Rt of 0, 1, 13 and 15 and 8 values of hw2<7:0>: 1,048,576
  opcodes. All 567,616 decodes as a coprocessor instruction agree with llvm on kind and coprocessor
  number; the 91,840 that llvm accepts and this tree refuses are all the UNPREDICTABLE shapes above
  (checked by class); llvm also decodes 2,048 `stc2l p10` (unindexed 0xFC.. forms) generically.
  FLDMX and FSTMX (VLDM and VSTM of doubles with an odd imm8, deprecated but valid, "SEE FLDMX" at
  VLDM in A7.7, D2.3.1) were a silent no-op (FLDMX, as LDC) or UDF (FSTMX); they are now coprocessor
  instructions that give UNDEFINSTR (FPU on) or NOCP (off). The firmware has both as real code
  (libgcc's `__gnu_Unwind_Restore_VFP` and `__gnu_Unwind_Save_VFP`), reached only by EHABI
  unwinding, which MMBasic does not do. FP batch 5 (below) implements them as the odd word count
  forms of VLDM and VSTM. Batch 4 left the FP decode alone (its FP tests passed unchanged);
  the batch-5 block below changes it deliberately. 12 FP encodings that llvm calls invalid decode
  as FP here, as they did before. Executed on the core with CPACR at reset: every kind x 14
  numbers gives CFSR.NOCP and the UsageFault with registers and memory untouched.
- MSR and MRS (B5.2.2, B5.2.3): MSR now refuses Rn = SP or PC and a SYSm outside 0 to 3, 5 to 9 and
  16 to 20 (for any mask), and MRS refuses Rd = SP or PC and the same SYSm set; all UDF. The MRS
  decoder also read Rd from five bits (8..13); it is four now (cosmetic: the table pattern fixes
  hw2<12> = 0). Rd = PC is refused in `decoder/mrs.rs` only. Armv6-M has SYSm {0..3, 5..9, 16, 20}
  and the armv6m build now uses that set (it used the v7-M one before). The SYSm 8 to 20 execution
  (MSP, PSP, PRIMASK, BASEPRI, BASEPRI_MAX, FAULTMASK, CONTROL) was re-read against the pseudocode
  and is right. Swept against llvm-mc (hw1 0xF380 to 0xF38F and 0xF3EF, hw2 0x8000 to 0x8FFF;
  139,264 opcodes): every MSR and MRS this tree decodes is accepted by llvm without a warning; llvm
  calls the remainder UNPREDICTABLE or invalid.
- Other encodings the audit marked wrong: SSAT16 and USAT16 (batch 3), MRS and GE (batch 1a) were
  already fixed, CSDB, STRT and the MCR family are above. I found nothing else wrong in the sweeps
  apart from what is listed.
- Firmware: the real firmware tests (boot to the prompt at 611,000,000 instructions and
  1,036,007,876 cycles, the BASIC lines, the USB, SD and RTC runs) print the same instruction and
  cycle counts (28 figures compared) before and after batch 4, and all pass; no STREX there changed
  its result in a way that alters the flow (a failing one would loop and move the counts).
- Mutation checks: 31 mutations of the new code (monitor, CLREX, hints, B T3, the unprivileged
  accesses, the coprocessor decode and executor, MSR and MRS) were run against the whole unit test
  suite; 27 were caught at once and the entry-clears-the-monitor one after a test was added. Three
  are equivalent: the STRT Rn = PC rule in `unpredictable.rs` (the decoder already refuses it), the
  unprivileged executor's own condition check (the ordinary executor it calls checks it, and the MPU
  hook is a stub), and the hint executor's condition check (it only changes the result kind of a
  skipped hint). Five planted bugs in the Unicorn comparison were all seen.
 - FP batch 5 (issue 77, the last batch): VMLA, VMLS, VNMLA and VNMLS in F32 and F64 (A7.7.238,
   A7.7.250); VCVTB and VCVTT between half precision and single or double precision, both directions
   (A7.7.231); and FLDMX and FSTMX, the odd-word-count forms of VLDM and VSTM (D2.3.1, the `imm8<0>
   = 1` case the VLDM page calls SEE FLDMX). The patterns are 16 added table rows — 4
   multiply-accumulate, 4 F16 convert, and 8 word-list, of the latter 4 the `P == U` reject rows —
   replacing the 2 wildcard word-list rows the 8 split, net 14; confirmed word for word against
   llvm-mc. `decoder/vmla.rs` and `decoder/vcvt.rs` wrappers decode the operands (Vd and Vm with the
   D and M extension bits); destinations outside the register bank of the built profile decode as
   UDF.
 - The multiply-accumulate executes the legacy pseudocode, not the FMA one: the product is
   rounded first, and the negation of VMLS, VNMLA and VNMLS is a sign-bit flip (FPNeg, A2.5) of
   the rounded product, of the accumulator, or of both — never a fused multiply-add and never a
   multiply by −1, because both differ when the accumulator and the two factors are the same
   register. Inf × 0 and a sum of two infinities of opposite sign (which is what adding an
   accumulator to its negation is) give the default NaN and set IOC; 0 × −finite is −0. The add
   takes the accumulator as its first operand, which is observable: with DN clear and both the
   accumulator and the product quiet NaNs, the accumulator's payload is the result.
 - Half precision converts through the generic engine with a `FloatOps for u16` instance:
   round-to-nearest-even, overflow to ±Infinity with OF and IX, input sticky flags kept, and one
   F16 value of the source half (VCVTB the low half, VCVTT the high half) read or written, the
   other half of the destination untouched. FPSCR.FZ never flushes a half-precision value: the
   flush tests in FPUnpack and FPRound are guarded by `N != 16` (A2.5; ARMv7-M has no FZ16 bit),
   and IDC is set only where a flush really happened, on the single or double side. FPSCR.DN
   does apply to half results: a NaN in or a NaN result becomes the default NaN of that width.
   FPSCR bit 26, the alternate half-precision mode (AHP in the FPSCR description, AH in the
   A2.5 pseudocode — one bit, 26, both names mean it), is not modelled: the conversions take
   the IEEE path whatever the bit holds, and `test_ahp_seeded_conversions_stay_on_the_ieee_path`
   pins that documented limitation. Bit 26 was not observable before this batch; the half
   converts make it observable, so the gap is now testable and pinned rather than silent.
 - The odd word count is a real FLDMX: the base moves by imm8 words (17 words is 68 bytes)
   while only (imm8 − 1)/2 doubles are transferred; the pseudocode reads the registers upward
   from the low end of the range, so the highest-addressed word of the range is the one no
   transfer reads or writes, in either addressing mode. Even counts are unchanged.
 - Splitting the VLDM and VSTM table rows by PU (needed so an odd count with P ≠ U is claimed by the
   word list and not by LDC or STC) first dropped the rule `P == U` with writeback is UNDEFINED
   (A7.7.235): such words fell through to the LDC and STC patterns for coprocessor 11, which llvm-mc
   rejects too. Fixed with four reject rows: two per direction, each pinning P and U (bits 24-23) to
   00 or 11 and W (bit 21) to 1, and the load/store bit (bit 20) to 1 on the load rows and 0 on the
   store rows. They keep the words inside the word-list pattern, which refuses them, so they decode
   as UDF. `P = 0, U = 0` belongs to the 64-bit transfers (MRRC and MCRR with D = 1, UDF with D = 0)
   and `P = 1, W = 0` to VLDR and VSTR. The split changed the decode space beyond the word list. A
   diff of every decode the table can reach (hw1 0xEC00-0xEFFF and 0xFC00-0xFFFF against every hw2,
   134,217,728 words, batch against pre-batch) moves 918,880 words between decode classes. The four
   groups, each sampled back through both decoders: 710,976 the pre-batch table decoded as LDC, STC
   or CDP for coprocessor 11 now decode as UNDEFINED; counted exhaustively against the new table
   rows: 245,760 match one of the four reject rows, 348,480 an accepted split word-list row and are
   refused there — 2,816 with a word count under one double and 345,664 with a list running past D15
   — and 116,736 a new MAC or F16-convert row naming D16-D31 beyond the 16-double bank; the three
   parts sum to the group. Separately 153,600 it decoded as CDP are now the new MAC and F16-convert
   instructions; 11,968 it decoded as LDC and STC decode as VLDM and VSTM — every one of them
   odd-count, the FLDMX and FSTMX forms; and 42,336 that decoded as UNDEFINED — 21,952 MCRR and
   20,384 MRRC, hw1 0xEC40-0xEC5F — decode as coprocessor transfers, encodings llvm-mc's assembler
   emits (its disassembler rejects some, the six words noted below). Two batch-4 test entries that
   used a `P == U, W = 1` lookalike as a "not the FPU" example were replaced with llvm-encoded MRRC
   words.
 - Checks for batch 5: 84,000 vectors against a Unicorn 2.1.5 oracle (12,000 each: the four MAC
   operations in each width, the VCVT half conversions in both directions and widths, and
   FLDMX/FSTMX) — all 84,000 bit-exact on the spilled D-register words, the FPSCR under the compare
   mask only (cumulative exception flags, IDC, rounding mode, FZ, DN — not the whole word), the
   16-word memory window and the core registers R0-R12, SP and LR. The transfer base of the
   FLDMX/FSTMX buckets is the first word of the compared window, so the stores of the IA forms land
   inside it while the stores of the DB forms, which run downwards from the base, land below it:
   those stored words are verified through the spilled D registers only, not in memory. PC is read
   but not compared: the two sides map the vector blob at different addresses, so its absolute value
   can never match; the Unicorn side proves each blob ran exactly to its stop address instead. The
   harness asserts per vector that the FPSCR it writes has reached the oracle before the vector runs
   — Unicorn initialises FP state lazily, and a write before the first FP instruction is dropped: an
   unwarmed first harness ran each bucket's first vector with FPSCR = 0 and reported five mismatches
   that were harness artifacts, not behaviours of the oracle. An independent run also used a harness
   of its own, built independently, which compares the transfer base registers directly; it lives
   outside this repo and is not re-runnable here, so its figures are quoted as reported: 0
   mismatches in 25,000 vectors for each MAC kind and 20,000 FLDMX/FSTMX vectors, two planted bugs
   caught. What is on disk and re-runnable is this crate's own harness and its run. The corrected
   harness, its inputs, results and logs live under spikes/oracle-b5/ (locally excluded from git). A
   decode sweep of 5,200 words against llvm-mc (encode, disassemble, compare with zmu decode and
   Display) found no decode mismatch; six words are cases where llvm-mc's disassembler rejects MCRR
   and MRRC encodings its own assembler emits. Twelve planted bugs in the new code (the MAC
   negations and operand order, the half-word extract and store, the F16 FZ guard, FPNeg, the IA and
   DB base arithmetic, the FLDMX word count, the PU reject row, the VCVTT half flag) were run
   against the suite: ten were caught at once, the operand-order one survived until a NaN-payload
   test was added for it, and the one that rewrote a table row tripped the table's duplicate-pattern
   guard so it was replaced by the equivalent decoder-wrapper mutation, which was caught. The
   firmware tests print the same 33 instruction-count lines as before batch 5.

`cargo clippy -p zmu_cortex_m --all-targets -- -D warnings` is clean (the crate denies
`clippy::pedantic`; the test code had unseparated long literals, `assert!` with `==`, and a few
casts).

I also added tests for these changes and updated the one upstream test that expected a fault on a
store to flash.
