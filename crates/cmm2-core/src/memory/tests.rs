use super::*;
use crate::{FLASH_BASE, Svd};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

fn region(name: &str) -> &'static RegionSpec {
    REGIONS.iter().find(|r| r.name == name).unwrap()
}

fn round_trip(name: &str) {
    let spec = region(name);
    let first = spec.base;
    let last = spec.base + (spec.size as u32 - 1);
    let mut m = MemoryMap::new();

    m.write8(first, 0xA5);
    m.write8(last, 0x5A);
    assert_eq!(m.read8(first), 0xA5);
    assert_eq!(m.read8(last), 0x5A);

    m.write16(first, 0xBEEF);
    m.write16(last - 1, 0xCAFE);
    assert_eq!(m.read16(first), 0xBEEF);
    assert_eq!(m.read16(last - 1), 0xCAFE);

    m.write32(first, 0xDEAD_BEEF);
    m.write32(last - 3, 0x1234_5678);
    assert_eq!(m.read32(first), 0xDEAD_BEEF);
    assert_eq!(m.read32(last - 3), 0x1234_5678);

    assert!(m.take_unmapped().is_empty());
}

#[test]
fn itcm_round_trips() {
    round_trip("ITCM");
}

#[test]
fn axi_sram_round_trips() {
    round_trip("AXI SRAM");
}

#[test]
fn sram12_round_trips() {
    round_trip("SRAM1/2");
}

#[test]
fn sram3_round_trips() {
    round_trip("SRAM3");
}

#[test]
fn sram4_round_trips() {
    round_trip("SRAM4");
}

#[test]
fn backup_sram_round_trips() {
    round_trip("Backup SRAM");
}

#[test]
fn sdram_round_trips() {
    round_trip("SDRAM");
}

#[test]
fn memory_starts_zeroed() {
    let mut m = MemoryMap::new();
    assert_eq!(m.read32(0xD000_0000), 0);
    assert_eq!(m.read32(0xD1FF_FFFC), 0);
}

#[test]
fn writes_are_little_endian() {
    let mut m = MemoryMap::new();
    m.write32(0x2400_0000, 0x1122_3344);
    assert_eq!(m.read8(0x2400_0000), 0x44);
    assert_eq!(m.read8(0x2400_0001), 0x33);
    assert_eq!(m.read8(0x2400_0002), 0x22);
    assert_eq!(m.read8(0x2400_0003), 0x11);
    assert_eq!(m.read16(0x2400_0002), 0x1122);
}

#[test]
fn unaligned_access_round_trips() {
    let mut m = MemoryMap::new();
    m.write32(0x3000_0001, 0xA1B2_C3D4);
    assert_eq!(m.read32(0x3000_0001), 0xA1B2_C3D4);
    m.write16(0x3000_0101, 0x7788);
    assert_eq!(m.read16(0x3000_0101), 0x7788);
}

#[test]
fn access_across_adjacent_regions() {
    let mut m = MemoryMap::new();
    m.write32(0x3003_FFFE, 0x4433_2211);
    assert_eq!(m.read32(0x3003_FFFE), 0x4433_2211);
    assert_eq!(m.read16(0x3003_FFFE), 0x2211);
    assert_eq!(m.read16(0x3004_0000), 0x4433);
    assert!(m.take_unmapped().is_empty());
}

#[test]
fn addresses_around_each_region_are_unmapped() {
    let mut m = MemoryMap::new();
    for spec in &REGIONS {
        let end = spec.base.wrapping_add(spec.size as u32);
        let before = spec.base.wrapping_sub(1);
        for addr in [end, before] {
            // The neighbours that are other regions are mapped.
            let mapped = REGIONS
                .iter()
                .any(|r| addr.wrapping_sub(r.base) < r.size as u32);
            if !mapped {
                m.read8(addr);
                assert_eq!(m.take_unmapped().len(), 1, "{} {addr:#x}", spec.name);
            }
        }
    }
    assert_eq!(m.read8(0xD200_0000), 0);
    assert_eq!(m.take_unmapped().len(), 1);
    assert_eq!(m.read8(0xCFFF_FFFF), 0);
    assert_eq!(m.take_unmapped().len(), 1);
}

#[test]
fn unmapped_read_returns_zero_and_is_logged() {
    let mut m = MemoryMap::new();
    assert_eq!(m.read16(0x4000_0000), 0);
    assert_eq!(
        m.take_unmapped(),
        [UnmappedAccess {
            addr: 0x4000_0000,
            size: 2,
            kind: AccessKind::Read,
            pc: 0,
        }]
    );
}

#[test]
fn unmapped_write_is_ignored_and_logged() {
    let mut m = MemoryMap::new();
    m.write32(0xE000_ED00, 0xCAFE_F00D);
    assert_eq!(
        m.take_unmapped(),
        [UnmappedAccess {
            addr: 0xE000_ED00,
            size: 4,
            kind: AccessKind::Write(0xCAFE_F00D),
            pc: 0,
        }]
    );
    assert_eq!(m.read32(0xE000_ED00), 0);
}

#[test]
fn access_off_the_end_of_sdram_is_split() {
    let mut m = MemoryMap::new();
    m.write32(0xD1FF_FFFE, 0x4433_2211);
    assert_eq!(
        m.take_unmapped(),
        [UnmappedAccess {
            addr: 0xD1FF_FFFE,
            size: 4,
            kind: AccessKind::Write(0x4433_2211),
            pc: 0,
        }]
    );
    // The two bytes inside SDRAM were written.
    assert_eq!(m.read16(0xD1FF_FFFE), 0x2211);
    m.take_unmapped();

    // A read gets the mapped bytes and zeros for the rest.
    assert_eq!(m.read32(0xD1FF_FFFE), 0x0000_2211);
    assert_eq!(
        m.take_unmapped(),
        [UnmappedAccess {
            addr: 0xD1FF_FFFE,
            size: 4,
            kind: AccessKind::Read,
            pc: 0,
        }]
    );
}

#[test]
fn straddle_below_a_region_is_split() {
    let mut m = MemoryMap::new();
    m.write16(0xCFFF_FFFF, 0xAABB);
    assert_eq!(m.take_unmapped().len(), 1);
    assert_eq!(m.read8(0xD000_0000), 0xAA);
}

#[test]
fn flash_and_dtcm_windows_belong_to_the_cpu() {
    let mut m = MemoryMap::new();
    assert_eq!(m.read32(FLASH_BASE), 0);
    assert_eq!(m.read32(DTCM_BASE), 0);
    m.write32(DTCM_BASE + DTCM_SIZE as u32 - 4, 1);
    assert_eq!(m.take_unmapped().len(), 3);
}

#[test]
fn log_is_bounded() {
    let mut m = MemoryMap::new();
    for i in 0..1030 {
        m.read8(0x4000_0000 + i);
    }
    assert_eq!(m.unmapped_dropped(), 6);
    let log = m.take_unmapped();
    assert_eq!(log.len(), 1024);
    assert_eq!(log[0].addr, 0x4000_0000);
    assert_eq!(log[1023].addr, 0x4000_0000 + 1023);

    m.read8(0x5000_0000);
    assert_eq!(m.take_unmapped().len(), 1);
}

#[test]
fn load_copies_into_memory() {
    let mut m = MemoryMap::new();
    m.load(0x0000_0000, &[1, 2, 3, 4]).unwrap();
    assert_eq!(m.read32(0), 0x0403_0201);
    m.load(0xD000_1000, &[9, 8, 7]).unwrap();
    assert_eq!(m.read8(0xD000_1002), 7);
}

#[test]
fn load_into_unmapped_memory_fails() {
    let mut m = MemoryMap::new();
    assert_eq!(
        m.load(0xD1FF_FFFE, &[1, 2, 3, 4]),
        Err(MemoryError::Unmapped(0xD200_0000))
    );
    assert_eq!(
        m.load(FLASH_BASE, &[1]),
        Err(MemoryError::Unmapped(FLASH_BASE))
    );
    assert!(m.take_unmapped().is_empty());
}

#[test]
fn creating_the_map_is_cheap() {
    // Zeroed allocations come straight from the OS, so the 32 MB of SDRAM costs nothing.
    let _ = MemoryMap::new();
}

/// Returns a count on each read of offset 0, and keeps what is written to offset 4.
/// Every access is also noted in `seen` as (offset, size, written value).
type Seen = Rc<RefCell<Vec<(u32, u8, Option<u32>)>>>;

struct Counter {
    next: u32,
    stored: u32,
    seen: Seen,
}

impl Counter {
    fn new() -> (Counter, Seen) {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let counter = Counter {
            next: 1,
            stored: 0,
            seen: seen.clone(),
        };
        (counter, seen)
    }
}

impl Peripheral for Counter {
    fn read(&mut self, offset: u32, size: u8) -> u32 {
        self.seen.borrow_mut().push((offset, size, None));
        if offset == 0 {
            self.next += 1;
            self.next - 1
        } else {
            self.stored
        }
    }

    fn write(&mut self, offset: u32, size: u8, value: u32) {
        self.seen.borrow_mut().push((offset, size, Some(value)));
        if offset == 4 {
            self.stored = value;
        }
    }
}

fn events() -> (Rc<RefCell<Vec<PeripheralAccess>>>, Option<Trace>) {
    let log = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();
    (log, Some(Box::new(move |e| sink.borrow_mut().push(*e))))
}

#[test]
fn peripheral_receives_offsets_and_sizes() {
    let (counter, seen) = Counter::new();
    let mut m = MemoryMap::new();
    m.map_peripheral(0x4000_1000, 0x100, Box::new(counter))
        .unwrap();

    assert_eq!(m.read32(0x4000_1000), 1);
    assert_eq!(m.read16(0x4000_1000), 2);
    m.write32(0x4000_1004, 0xAABB_CCDD);
    assert_eq!(m.read32(0x4000_1004), 0xAABB_CCDD);
    m.write8(0x4000_1004, 0x7F);
    assert_eq!(m.read8(0x4000_1004), 0x7F);
    m.write16(0x4000_1004, 0x1234);

    assert_eq!(
        *seen.borrow(),
        [
            (0, 4, None),
            (0, 2, None),
            (4, 4, Some(0xAABB_CCDD)),
            (4, 4, None),
            (4, 1, Some(0x7F)),
            (4, 1, None),
            (4, 2, Some(0x1234)),
        ]
    );
    assert!(m.take_unmapped().is_empty());
}

#[test]
fn window_edges_are_inside_and_outside() {
    let (counter, seen) = Counter::new();
    let mut m = MemoryMap::new();
    m.map_peripheral(0x4000_1000, 0x100, Box::new(counter))
        .unwrap();
    m.read32(0x4000_10FC);
    m.read8(0x4000_10FF);
    assert_eq!(seen.borrow().len(), 2);
    m.read8(0x4000_1100);
    m.read8(0x4000_0FFF);
    assert_eq!(seen.borrow().len(), 2);
    assert_eq!(m.take_unmapped().len(), 2);
}

#[test]
fn large_window_costs_nothing() {
    let mut m = MemoryMap::new();
    m.map_peripheral(0x4000_0000, 0x2000_0000, Box::new(RegisterStub::new()))
        .unwrap();
}

fn mapped() -> MemoryMap {
    let mut m = MemoryMap::new();
    m.map_peripheral(0x4000_0000, 0x1000, Box::new(RegisterStub::new()))
        .unwrap();
    m
}

#[test]
fn window_over_ram_is_rejected() {
    let mut m = mapped();
    let stub = || Box::new(RegisterStub::new());
    // Start inside, end inside, and containing a RAM region.
    assert_eq!(
        m.map_peripheral(0x2400_0000, 0x10, stub()),
        Err(MemoryError::Overlap(0x2400_0000))
    );
    assert_eq!(
        m.map_peripheral(0x23FF_FFF0, 0x20, stub()),
        Err(MemoryError::Overlap(0x23FF_FFF0))
    );
    assert_eq!(
        m.map_peripheral(0x2000_0000, 0x1000_0000, stub()),
        Err(MemoryError::Overlap(0x2000_0000))
    );
}

#[test]
fn window_over_another_window_is_rejected() {
    let mut m = mapped();
    let stub = || Box::new(RegisterStub::new());
    // Partial overlap at either end, containment both ways, and the same window.
    for (base, size) in [
        (0x3FFF_FF00, 0x200),
        (0x4000_0F00, 0x200),
        (0x4000_0100, 0x10),
        (0x3FFF_0000, 0x2_0000),
        (0x4000_0000, 0x1000),
    ] {
        assert_eq!(
            m.map_peripheral(base, size, stub()),
            Err(MemoryError::Overlap(base)),
            "{base:#x}"
        );
    }
}

#[test]
fn adjacent_windows_are_accepted() {
    let mut m = mapped();
    let stub = || Box::new(RegisterStub::new());
    assert_eq!(m.map_peripheral(0x4000_1000, 0x1000, stub()), Ok(()));
    assert_eq!(m.map_peripheral(0x3FFF_F000, 0x1000, stub()), Ok(()));
    // Touching a RAM region without overlapping it.
    assert_eq!(m.map_peripheral(0x3004_8000, 0x1000, stub()), Ok(()));
}

#[test]
fn failed_mapping_leaves_the_map_alone() {
    let mut m = mapped();
    let _ = m.map_peripheral(0x4000_0800, 0x1000, Box::new(RegisterStub::new()));
    assert_eq!(m.read32(0x4000_1000), 0);
    assert_eq!(m.take_unmapped().len(), 1);
}

#[test]
fn overlap_error_is_readable() {
    assert_eq!(
        MemoryError::Overlap(0x4000_0000).to_string(),
        "window at 0x40000000 overlaps mapped memory"
    );
}

#[test]
fn access_past_the_end_of_a_window_is_unmapped() {
    let (counter, seen) = Counter::new();
    let mut m = MemoryMap::new();
    m.map_peripheral(0x4000_1000, 0x100, Box::new(counter))
        .unwrap();

    m.write32(0x4000_10FE, 0x4433_2211);
    assert_eq!(
        m.take_unmapped(),
        [UnmappedAccess {
            addr: 0x4000_10FE,
            size: 4,
            kind: AccessKind::Write(0x4433_2211),
            pc: 0,
        }]
    );
    assert_eq!(m.read32(0x4000_10FE), 0);
    assert_eq!(m.take_unmapped().len(), 1);
    // The peripheral did not see any of it.
    assert!(seen.borrow().is_empty());
}

#[test]
fn access_from_ram_into_a_window_is_unmapped() {
    let mut m = MemoryMap::new();
    m.map_peripheral(0x3004_8000, 0x100, Box::new(RegisterStub::new()))
        .unwrap();
    m.write16(0x3004_7FFF, 0xAABB);
    assert_eq!(m.take_unmapped().len(), 1);
    assert_eq!(m.read8(0x3004_7FFF), 0xBB);
    // The byte that fell in the window was not delivered.
    assert_eq!(m.read8(0x3004_8000), 0);
    assert!(m.take_unmapped().is_empty());
}

struct AllOnes;

impl Peripheral for AllOnes {
    fn read(&mut self, _offset: u32, _size: u8) -> u32 {
        0xFFFF_FFFF
    }

    fn write(&mut self, _offset: u32, _size: u8, _value: u32) {}
}

#[test]
fn read_value_is_masked_to_the_access_size() {
    let mut m = MemoryMap::new();
    m.map_peripheral(0x4000_0000, 0x100, Box::new(AllOnes))
        .unwrap();
    let (log, trace) = events();
    m.set_peripheral_trace(trace);
    assert_eq!(m.read8(0x4000_0000), 0xFF);
    assert_eq!(m.read16(0x4000_0000), 0xFFFF);
    assert_eq!(m.read32(0x4000_0000), 0xFFFF_FFFF);
    let values: Vec<u32> = log.borrow().iter().map(|e| e.value).collect();
    assert_eq!(values, [0xFF, 0xFFFF, 0xFFFF_FFFF]);
}

#[test]
fn write_value_is_masked_for_the_trace_only() {
    let (counter, seen) = Counter::new();
    let mut m = MemoryMap::new();
    m.map_peripheral(0x4000_0000, 0x100, Box::new(counter))
        .unwrap();
    let (log, trace) = events();
    m.set_peripheral_trace(trace);
    m.write8(0x4000_0004, 0xAB);
    m.write16(0x4000_0004, 0x1234);
    let values: Vec<u32> = log.borrow().iter().map(|e| e.value).collect();
    assert_eq!(values, [0xAB, 0x1234]);
    // The device is given the value as it came in.
    assert_eq!(*seen.borrow(), [(4, 1, Some(0xAB)), (4, 2, Some(0x1234))]);
}

#[test]
fn unmodelled_peripherals_read_their_reset_values() {
    let mut m = MemoryMap::new();
    m.map_unmodelled_peripherals().unwrap();
    assert_eq!(m.read32(0x5802_4400), 0x0000_0083); // RCC_CR
    assert_eq!(m.read32(0x5802_4818), 0x0000_4000); // PWR_D3CR
    assert_eq!(m.read32(0x4001_101C), 0x0000_00C0); // USART1_ISR
    assert_eq!(m.read32(0x5FFF_FFFC), 0);
    assert_eq!(m.read32(0xE000_ED14), 0);
}

#[test]
fn unmodelled_peripheral_write_replaces_the_reset_value() {
    let mut m = MemoryMap::new();
    m.map_unmodelled_peripherals().unwrap();
    m.write32(0x5802_4400, 0x1234_5678);
    assert_eq!(m.read32(0x5802_4400), 0x1234_5678);
}

#[test]
fn unmodelled_peripherals_twice_is_an_overlap() {
    let mut m = MemoryMap::new();
    m.map_unmodelled_peripherals().unwrap();
    assert_eq!(
        m.map_unmodelled_peripherals(),
        Err(MemoryError::Overlap(0x4000_0000))
    );
}

#[test]
fn unmodelled_peripherals_round_trip() {
    let mut m = MemoryMap::new();
    m.map_unmodelled_peripherals().unwrap();
    m.write32(0x5802_4400, 0x0303_0083);
    assert_eq!(m.read32(0x5802_4400), 0x0303_0083);
    m.write32(0xE000_ED14, 0x200);
    assert_eq!(m.read32(0xE000_ED14), 0x200);
    m.write16(0x4000_0002, 0xAAAA);
    assert_eq!(m.read32(0x4000_0000), 0xAAAA_0000);
    assert!(m.take_unmapped().is_empty());
}

#[test]
fn unmodelled_peripherals_stop_at_their_ends() {
    let mut m = MemoryMap::new();
    m.map_unmodelled_peripherals().unwrap();
    for addr in [0x3FFF_FFFF, 0x6000_0000, 0xDFFF_FFFF, 0xE010_0000] {
        m.write8(addr, 1);
        assert_eq!(m.read8(addr), 0, "{addr:#x}");
        assert_eq!(m.take_unmapped().len(), 2, "{addr:#x}");
    }
    for addr in [0x5FFF_FFFC, 0xE00F_FFFC, 0x4000_0000, 0xE000_0000] {
        m.write32(addr, 1);
        assert_eq!(m.read32(addr), 1, "{addr:#x}");
    }
    assert!(m.take_unmapped().is_empty());
}

#[test]
fn ram_still_works_and_is_not_traced() {
    let mut m = MemoryMap::new();
    m.map_unmodelled_peripherals().unwrap();
    let (log, trace) = events();
    m.set_peripheral_trace(trace);
    m.write32(0x2400_0000, 0x1234_5678);
    assert_eq!(m.read32(0x2400_0000), 0x1234_5678);
    m.write8(0xD000_0000, 9);
    assert!(log.borrow().is_empty());
}

#[test]
fn trace_sees_reads_and_writes_in_order() {
    let mut m = MemoryMap::new();
    m.map_unmodelled_peripherals().unwrap();
    let (log, trace) = events();
    m.set_peripheral_trace(trace);

    m.write32(0x5802_4400, 0xAB);
    assert_eq!(m.read16(0x5802_4400), 0xAB);

    assert_eq!(
        *log.borrow(),
        [
            PeripheralAccess {
                addr: 0x5802_4400,
                size: 4,
                write: true,
                value: 0xAB,
                pc: 0,
            },
            PeripheralAccess {
                addr: 0x5802_4400,
                size: 2,
                write: false,
                value: 0xAB,
                pc: 0,
            },
        ]
    );
}

#[test]
fn trace_can_be_switched_off() {
    let mut m = MemoryMap::new();
    m.map_unmodelled_peripherals().unwrap();
    let (log, trace) = events();
    m.set_peripheral_trace(trace);
    m.read32(0x5802_4400);
    m.set_peripheral_trace(None);
    m.read32(0x5802_4400);
    m.write32(0x5802_4400, 1);
    assert_eq!(log.borrow().len(), 1);
}

#[test]
fn trace_does_not_see_unmapped_accesses() {
    let mut m = MemoryMap::new();
    m.map_unmodelled_peripherals().unwrap();
    let (log, trace) = events();
    m.set_peripheral_trace(trace);
    m.read32(0x6000_0000);
    assert!(log.borrow().is_empty());
}

#[test]
fn trace_events_can_be_named_with_the_svd() {
    let mut m = MemoryMap::new();
    m.map_unmodelled_peripherals().unwrap();
    let named = Rc::new(RefCell::new(Vec::new()));
    let sink = named.clone();
    let svd = Svd::stm32h743();
    m.set_peripheral_trace(Some(Box::new(move |e| {
        let name = svd.lookup(e.addr).map(|l| l.name.to_string());
        sink.borrow_mut().push((name.unwrap(), e.write));
    })));

    m.write32(0x5802_4400, 1);
    m.read32(0x5200_414C);

    assert_eq!(
        *named.borrow(),
        [
            ("RCC_CR".to_string(), true),
            ("FMC_SDTR2".to_string(), false)
        ]
    );
}

#[test]
fn trace_sees_modelled_peripherals() {
    let (counter, _) = Counter::new();
    let mut m = MemoryMap::new();
    m.map_peripheral(0x4000_1000, 0x100, Box::new(counter))
        .unwrap();
    let (log, trace) = events();
    m.set_peripheral_trace(trace);

    m.read32(0x4000_1000);
    m.write32(0x4000_1004, 7);

    assert_eq!(
        *log.borrow(),
        [
            PeripheralAccess {
                addr: 0x4000_1000,
                size: 4,
                write: false,
                value: 1,
                pc: 0,
            },
            PeripheralAccess {
                addr: 0x4000_1004,
                size: 4,
                write: true,
                value: 7,
                pc: 0,
            },
        ]
    );
}

#[test]
fn records_carry_the_shared_pc() {
    let mut m = MemoryMap::new();
    m.map_unmodelled_peripherals().unwrap();
    let (log, trace) = events();
    m.set_peripheral_trace(trace);
    let pc = m.pc_cell();

    pc.set(0x0800_1234);
    m.write32(0x6000_0000, 1);
    pc.set(0x0800_5678);
    m.read32(0x4000_0000);

    assert_eq!(m.take_unmapped()[0].pc, 0x0800_1234);
    assert_eq!(log.borrow()[0].pc, 0x0800_5678);
}

// A window at 0x4000_1000 (0x100 bytes) that only answers with its own marker, inside a
// fallback stub over 0x4000_0000 (0x1000_0000 bytes).
fn with_fallback() -> (MemoryMap, Seen) {
    let (counter, seen) = Counter::new();
    let mut m = MemoryMap::new();
    m.map_fallback(0x4000_0000, 0x1000_0000, Box::new(RegisterStub::new()))
        .unwrap();
    m.map_peripheral(0x4000_1000, 0x100, Box::new(counter))
        .unwrap();
    (m, seen)
}

#[test]
fn specific_window_wins_inside_a_fallback() {
    let (mut m, seen) = with_fallback();
    assert_eq!(m.read32(0x4000_1000), 1);
    assert_eq!(*seen.borrow(), [(0, 4, None)]);
    m.write32(0x4000_1004, 0xAABB_CCDD);
    assert_eq!(m.read32(0x4000_1004), 0xAABB_CCDD);
}

#[test]
fn fallback_serves_the_rest() {
    let (mut m, seen) = with_fallback();
    m.write32(0x4000_0FFC, 7);
    m.write32(0x4000_1100, 8);
    assert_eq!(m.read32(0x4000_0FFC), 7);
    assert_eq!(m.read32(0x4000_1100), 8);
    assert!(seen.borrow().is_empty());
    assert!(m.take_unmapped().is_empty());
}

// An access that is not wholly inside the specific window goes to the fallback when the
// fallback holds all its bytes: the specific window never sees a partial access.
#[test]
fn access_straddling_a_specific_window_goes_to_the_fallback() {
    let (mut m, seen) = with_fallback();
    m.write32(0x4000_10FE, 0x4433_2211);
    assert_eq!(m.read32(0x4000_10FE), 0x4433_2211);
    m.write16(0x4000_0FFF, 0xAABB);
    assert_eq!(m.read16(0x4000_0FFF), 0xAABB);
    assert!(seen.borrow().is_empty());
    assert!(m.take_unmapped().is_empty());
}

#[test]
fn fallback_only_serves_accesses_it_holds_whole() {
    let mut m = MemoryMap::new();
    m.map_fallback(0x4000_0000, 0x100, Box::new(RegisterStub::new()))
        .unwrap();
    m.write32(0x4000_00FE, 1);
    assert_eq!(m.take_unmapped().len(), 1);
}

#[test]
fn fallback_over_ram_is_rejected() {
    let mut m = MemoryMap::new();
    let stub = || Box::new(RegisterStub::new());
    assert_eq!(
        m.map_fallback(0x2400_0000, 0x10, stub()),
        Err(MemoryError::Overlap(0x2400_0000))
    );
    assert_eq!(
        m.map_fallback(0x2000_0000, 0x1000_0000, stub()),
        Err(MemoryError::Overlap(0x2000_0000))
    );
}

#[test]
fn fallback_over_another_fallback_is_rejected() {
    let mut m = MemoryMap::new();
    let stub = || Box::new(RegisterStub::new());
    m.map_fallback(0x4000_0000, 0x1000, stub()).unwrap();
    assert_eq!(
        m.map_fallback(0x4000_0800, 0x1000, stub()),
        Err(MemoryError::Overlap(0x4000_0800))
    );
    assert_eq!(m.map_fallback(0x4000_1000, 0x1000, stub()), Ok(()));
}

#[test]
fn specific_windows_still_cannot_overlap_inside_a_fallback() {
    let (mut m, _) = with_fallback();
    assert_eq!(
        m.map_peripheral(0x4000_1080, 0x100, Box::new(RegisterStub::new())),
        Err(MemoryError::Overlap(0x4000_1080))
    );
    // Over a part of the fallback that no specific window holds.
    assert_eq!(
        m.map_peripheral(0x4000_2000, 0x100, Box::new(RegisterStub::new())),
        Ok(())
    );
}

#[test]
fn specific_window_can_be_mapped_before_the_fallback() {
    let (counter, _) = Counter::new();
    let mut m = MemoryMap::new();
    m.map_peripheral(0x4000_1000, 0x100, Box::new(counter))
        .unwrap();
    m.map_fallback(0x4000_0000, 0x1000_0000, Box::new(RegisterStub::new()))
        .unwrap();
    assert_eq!(m.read32(0x4000_1000), 1);
}

#[test]
fn unmodelled_peripherals_map_both_ranges() {
    let mut m = MemoryMap::new();
    m.map_unmodelled_peripherals().unwrap();
    m.write32(0x4000_0000, 1);
    m.write32(0xE000_0000, 2);
    assert_eq!(m.read32(0x4000_0000), 1);
    assert_eq!(m.read32(0xE000_0000), 2);
}

#[test]
fn modelled_peripheral_wins_over_the_unmodelled_ones() {
    let (counter, _) = Counter::new();
    let mut m = MemoryMap::new();
    m.map_peripheral(0x5802_4800, 0x400, Box::new(counter))
        .unwrap();
    m.map_unmodelled_peripherals().unwrap();
    assert_eq!(m.read32(0x5802_4800), 1);
    assert_eq!(m.read32(0x5802_4400), 0x83);
}

// Counts its resets and remembers the last cause.
struct Resettable(Rc<RefCell<Vec<ResetCause>>>);

impl Peripheral for Resettable {
    fn read(&mut self, _offset: u32, _size: u8) -> u32 {
        0
    }

    fn write(&mut self, _offset: u32, _size: u8, _value: u32) {}

    fn reset(&mut self, cause: ResetCause) {
        self.0.borrow_mut().push(cause);
    }
}

#[test]
fn reset_peripherals_resets_windows_and_fallbacks_but_not_ram() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let mut m = MemoryMap::new();
    m.map_peripheral(0x4000_0000, 0x100, Box::new(Resettable(seen.clone())))
        .unwrap();
    m.map_fallback(0x5000_0000, 0x100, Box::new(Resettable(seen.clone())))
        .unwrap();
    m.map_fallback(0x6000_0000, 0x1000, Box::new(RegisterStub::new()))
        .unwrap();
    m.write32(0x6000_0010, 9);
    m.write32(0x2400_0000, 0xCAFE);
    m.write32(0x7000_0000, 1);
    let unmapped = m.take_unmapped().len();
    m.set_peripheral_trace(Some(Box::new(|_| {})));

    m.reset_peripherals(ResetCause::Software);

    assert_eq!(*seen.borrow(), [ResetCause::Software; 2]);
    assert_eq!(m.read32(0x6000_0010), 0);
    assert_eq!(m.read32(0x2400_0000), 0xCAFE);
    assert_eq!(unmapped, 1);
    assert!(m.trace.is_some());
}

#[test]
fn reset_peripherals_keeps_the_unmapped_log() {
    let mut m = MemoryMap::new();
    m.write32(0x7000_0000, 1);
    m.reset_peripherals(ResetCause::Software);
    assert_eq!(m.take_unmapped().len(), 1);
}

// Counts its ticks.
struct Ticking(Rc<Cell<u32>>);

impl Peripheral for Ticking {
    fn read(&mut self, _offset: u32, _size: u8) -> u32 {
        0
    }

    fn write(&mut self, _offset: u32, _size: u8, _value: u32) {}

    fn tick(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn tick_reaches_windows_and_fallbacks() {
    let ticks = Rc::new(Cell::new(0));
    let mut m = MemoryMap::new();
    m.map_peripheral(0x4000_0000, 0x100, Box::new(Ticking(ticks.clone())))
        .unwrap();
    m.map_fallback(0x5000_0000, 0x100, Box::new(Ticking(ticks.clone())))
        .unwrap();
    m.tick();
    assert_eq!(ticks.get(), 2);
}
