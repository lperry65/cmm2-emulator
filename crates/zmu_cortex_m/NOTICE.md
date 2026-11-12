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
- GDB: `GdbServer`, `ZmuTarget` and `Simulation` can be built from a processor
  that has already been set up and reset (`from_processor`).
- Default features: `default` is now `["armv7em", "fpv5-d16"]`, the Cortex-M7
  with the double-precision FPU. All other features are unchanged.

I also added tests for these changes and updated the one upstream test that
expected a fault on a store to flash.
