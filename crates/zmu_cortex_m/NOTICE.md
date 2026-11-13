# zmu_cortex_m

This directory is a vendored copy of the `zmu_cortex_m` crate from zmu
(https://github.com/jjkt/zmu), taken from upstream commit 72feb75. zmu is
licensed under the Apache License 2.0; the original `LICENSE` is in this
directory and still applies to the whole crate, including my changes.

Two editor project files from the upstream tree were left out. Everything else
was imported unchanged first, and my changes went in as a separate commit.

## My changes

The firmware needs a few things zmu does not do, so I changed the following.

- Flash base address: the flash memory can now sit at a base address
  (`FlashMemory::new_at`, `Processor::flash_memory_at`), and instruction
  caching and fetching use that base, so the ITCM region is reachable.
- Interrupts: the interrupt count is raised from 32 to 240. The NVIC register
  decode used the wrong shift for the word index (it only worked for word 0),
  and the pending-exception and clear scans used the wrong interrupt number
  for words above 0.
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
- LDR literal: the decoder now treats `Rn = PC` in the LDR immediate (T4) and
  LDR register (T2) encodings as the literal form, which is how `U = 0` is
  encoded. LDR literal with `Rt = PC` now reports a branch instead of a normal
  instruction.
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
  undefined. The extend-and-add forms share one executor. The firmware has about thirty-five UXTAH and SXTAH.
- Rounding to an integer (FPRoundInt, FPToFixed): they now round on the bits of the value, not on
  the `BigFloat` approximation, which keeps 40 digits and so does not hold a double with a long
  mantissa exactly (a floor of such a whole number could come back one low). The rounding modes,
  ties to even and ties away included, are one `FPRounding` that the instruction or the FPSCR
  names. A tie in the FPSCR nearest mode went to the odd neighbour; it goes to the even one.
  A result that saturates now signals Invalid Operation, not Overflow. Fixed point to float scales
  by multiplying by 2^-n, because dividing by `BigFloat`'s 2^n lost digits.
- GDB: `GdbServer`, `ZmuTarget` and `Simulation` can be built from a processor
  that has already been set up and reset (`from_processor`).
- IT state: the IF-THEN state is now saved in the stacked xPSR on exception
  entry and restored on return. It was dropped, so an interrupt inside an IT
  block corrupted the handler.
- Default features: `default` is now `["armv7em", "fpv5-d16"]`, the Cortex-M7
  with the double-precision FPU. All other features are unchanged.

I also added tests for these changes and updated the one upstream test that
expected a fault on a store to flash.
