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
    too; they called the bus directly. A lazy FP state save is not one. The crate does not check
    alignment anywhere, so an unaligned LDM or VLDR does not fault UNALIGNED first.
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
      '1111':EXC_RETURN. I used to push a frame there. The reviewer was right.
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
    HardFault. A HardFault at an execution priority below 0 (HardFault or NMI running, or
    FAULTMASK) is a lockup. Checked against ExecutionPriority() (DDI0403E.e B1.5.4, pp. B1-528,
    B1-529: PRIGROUP applied to the active exceptions and to BASEPRI, PRIMASK 0, FAULTMASK -1),
    Priority escalation (p. B1-529: a synchronous fault or SVC whose group priority is not higher
    than the execution priority, or a disabled configurable fault) and B1.5.15 (pp. B1-555,
    B1-556: lockup is a fault at a priority below 0, HardFault, NMI or FAULTMASK); the code agrees
    with them. A priority that is not a group priority compares the same, since the execution
    priority is always a multiple of the group size. It used to be escalated only when disabled, and a lockup only when the IPSR said
    HardFault or NMI. The exception reported as running is the one in the IPSR when it is active,
    and otherwise the active one of the highest group priority, the lower number among equals.
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
  SVCall entry fails while stacking, the derived fault stacks the SVC's own address, so the IT
  state is put back to what it was before the SVC. BKPT with no
  semihosting host is a debug event (C1.5, p. C1-694; B1.5.14, HardFault on breakpoint
  escalation): DFSR.BKPT is set, the DebugMonitor is taken if DEMCR.MON_EN is set and its group
  priority is higher than the execution priority, and otherwise it is a HardFault with HFSR.DEBUGEVT, a lockup
  below priority 0. A debug event that generates the DebugMonitor sets DEMCR.MON_PEND (C1-694),
  and MON_PEND stays set until the exception is taken or a write clears it (C1-689); the BKPT takes
  the DebugMonitor at once, so the code does not set the bit for it and it reads 0 afterwards.
  A software write of MON_PEND = 1 to DEMCR pends the DebugMonitor whatever MON_EN is (C1-689):
  it is taken by the usual priority rules, taking it clears MON_PEND, and a write of 0 removes the
  pending exception. The return
  address is the BKPT itself for both (ReturnAddress(), synchronous). DFSR bits are set when the event halts or
  generates an exception (C1-694), so DFSR.BKPT is set on the HardFault path too; only whether
  they are set for an event that is ignored or causes lockup is IMPLEMENTATION DEFINED. BKPT #0xab with no host is the same.
  ARMv6-M has no DebugMonitor (exception 12) and no DEMCR, but the core stores every bit of DEMCR,
  so on an armv6m build BKPT never enters exception 12 now, and a MON_PEND write pends nothing. With a GDB server attached, a BKPT
  (not #0xab when a semihosting function is installed, which it is for the servers built by
  `GdbServer::new`; with none, #0xab halts too) halts into the debugger: the simulation reports a breakpoint
  stop and DFSR.BKPT is set, and the PC stays on the BKPT, as it does when silicon halts in Debug
  state. A plain continue or step from there hits the same BKPT again, as it does on hardware;
  the debugger has to move the PC past it (GDB's `set $pc += 2`, which the server supports as a
  register write), and then continue and step go on. A BKPT that halts does not advance the IT
  state, as it does not move the PC, and takes no pending exception in that step (the debugger
  sees the PC on the BKPT; the exception stays pending). The halt is consumed when the simulation
  reports it. It used to be a HardFault under the server. A stacking failure on the
  entry of an exception is a lockup by the same rule as a fault (execution priority below 0), not
  by the IPSR. The xPSR that a refused return stacks holds only the fields the xPSR has (reserved
  bits 23:20, and GE without the DSP extension, are zero).
- CCR.STKALIGN (B3.2, p. B3-605; B1.5.7, p. B1-535): whether the bit is RW or RO (RAO) and its reset
  value are IMPLEMENTATION DEFINED, and Arm recommends a reset of 1. On the armv7em build (the
  Cortex-M7) the CCR now resets with STKALIGN = 1; it stays writable. That the real M7 resets it
  to 1 (and whether it is RW or RAO) is from my memory of the Cortex-M7 TRM, which I do not have:
  unverified. Other builds still reset the CCR to 0.
- The 16-bit STM decoded only seven bits of its register list (`0b111_1111`), dropping r7 from
  every `stmia Rn!, {..., r7}` (`{r4-r7}` stored three words and wrote back 12). A7.7.159 T1 (p.
  A7-383) has an 8-bit list; fixed. The other list decoders (LDM T1, PUSH T1 with LR, POP T1 with
  PC, the 32-bit forms) were checked and are right. LDM.W with fewer than two registers, or with
  write back and the base in the list (A7.7.41 T2), is UDF in `decode_LDM_t2`, and has a test. STM.W and STMDB cannot have SP or PC in the
  list (the decode patterns have 0 in bits 15 and 13).
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
- GDB: `GdbServer`, `ZmuTarget` and `Simulation` can be built from a processor that has already
  been set up and reset (`from_processor`). Both mark the processor as having a debugger attached.
- IT state: the IF-THEN state is now saved in the stacked xPSR on exception entry and restored on
  return. It was dropped, so an interrupt inside an IT block corrupted the handler.
- Default features: `default` is now `["armv7em", "fpv5-d16"]`, the Cortex-M7 with the
  double-precision FPU. All other features are unchanged.

`cargo clippy -p zmu_cortex_m --all-targets -- -D warnings` is clean (the crate denies
`clippy::pedantic`; the test code had unseparated long literals, `assert!` with `==`, and a few
casts).

I also added tests for these changes and updated the one upstream test that expected a fault on a
store to flash.
