use super::*;
use crate::{MemoryMap, ResetCause};

const CR1_ADDR: u32 = USART1_BASE + CR1;
const CR2_ADDR: u32 = USART1_BASE + 0x04;
const BRR_ADDR: u32 = USART1_BASE + 0x0C;
const RQR_ADDR: u32 = USART1_BASE + RQR;
const ISR_ADDR: u32 = USART1_BASE + ISR;
const ICR_ADDR: u32 = USART1_BASE + ICR;
const RDR_ADDR: u32 = USART1_BASE + RDR;
const TDR_ADDR: u32 = USART1_BASE + TDR;

fn machine_with_irqs() -> (MemoryMap, Console, Irqs) {
    let console = Console::new();
    let irqs = Irqs::new();
    let mut m = MemoryMap::new();
    m.map_peripheral(
        USART1_BASE,
        USART_SIZE,
        Box::new(Usart::new(USART1_BASE, console.clone(), irqs.clone())),
    )
    .unwrap();
    (m, console, irqs)
}

fn machine() -> (MemoryMap, Console) {
    let (m, console, _) = machine_with_irqs();
    (m, console)
}

/// Whether the USART1 line, and only that, was raised since the last call.
fn raised(irqs: &Irqs) -> bool {
    irqs.take() == Some([0, 1 << (USART1_IRQ - 32), 0, 0, 0, 0, 0, 0])
}

fn nothing_raised(irqs: &Irqs) -> bool {
    irqs.take().is_none()
}

#[test]
fn enabling_tcie_raises_the_line() {
    let (mut m, _, irqs) = machine_with_irqs();
    m.write32(CR1_ADDR, UE | TE);
    assert!(nothing_raised(&irqs));
    m.write32(CR1_ADDR, UE | TE | TCIE);
    assert!(raised(&irqs));
}

#[test]
fn enabling_txeie_raises_the_line() {
    let (mut m, _, irqs) = machine_with_irqs();
    m.write32(CR1_ADDR, UE | TE | TXEIE);
    assert!(raised(&irqs));
}

#[test]
fn enabling_neither_raises_nothing() {
    let (mut m, _, irqs) = machine_with_irqs();
    m.write32(CR1_ADDR, UE | TE | RE | RXNEIE);
    assert!(nothing_raised(&irqs));
}

#[test]
fn rewriting_cr1_with_the_condition_already_true_raises_nothing() {
    let (mut m, _, irqs) = machine_with_irqs();
    m.write32(CR1_ADDR, UE | TE | TCIE);
    assert!(raised(&irqs));
    m.write32(CR1_ADDR, UE | TE | TCIE);
    assert!(nothing_raised(&irqs));
}

#[test]
fn enabling_again_after_disabling_raises_again() {
    let (mut m, _, irqs) = machine_with_irqs();
    m.write32(CR1_ADDR, UE | TE | TCIE);
    assert!(raised(&irqs));
    m.write32(CR1_ADDR, UE | TE);
    assert!(nothing_raised(&irqs));
    m.write32(CR1_ADDR, UE | TE | TCIE);
    assert!(raised(&irqs));
}

#[test]
fn rxneie_raises_only_with_input_waiting() {
    let (mut m, console, irqs) = machine_with_irqs();
    m.write32(CR1_ADDR, UE | RE | RXNEIE);
    assert!(nothing_raised(&irqs));
    m.write32(CR1_ADDR, UE | RE);
    console.push_input(b"1");
    assert!(nothing_raised(&irqs));
    m.write32(CR1_ADDR, UE | RE | RXNEIE);
    assert!(raised(&irqs));
}

#[test]
fn a_byte_sent_with_tcie_or_txeie_set_raises_the_line() {
    for enable in [TCIE, TXEIE] {
        let (mut m, console, irqs) = machine_with_irqs();
        m.write32(CR1_ADDR, UE | TE | enable);
        assert!(raised(&irqs));
        m.write32(TDR_ADDR, 0x41);
        assert!(raised(&irqs));
        assert_eq!(console.take_output(), b"A");
    }
}

#[test]
fn a_byte_sent_with_neither_enable_raises_nothing() {
    let (mut m, _, irqs) = machine_with_irqs();
    m.write32(CR1_ADDR, UE | TE | RE | RXNEIE);
    m.write32(TDR_ADDR, 0x41);
    assert!(nothing_raised(&irqs));
}

#[test]
fn a_byte_dropped_with_te_clear_raises_nothing() {
    let (mut m, _, irqs) = machine_with_irqs();
    m.write32(CR1_ADDR, UE | TCIE);
    assert!(raised(&irqs));
    m.write32(TDR_ADDR, 0x41);
    assert!(nothing_raised(&irqs));
}

#[test]
fn reading_with_more_input_waiting_raises_the_line() {
    let (mut m, console, irqs) = machine_with_irqs();
    console.push_input(b"hi");
    m.write32(CR1_ADDR, UE | RE | RXNEIE);
    assert!(raised(&irqs));
    assert_eq!(m.read32(RDR_ADDR), u32::from(b'h'));
    assert!(raised(&irqs));
}

#[test]
fn reading_the_last_byte_raises_nothing() {
    let (mut m, console, irqs) = machine_with_irqs();
    console.push_input(b"h");
    m.write32(CR1_ADDR, UE | RE | RXNEIE);
    assert!(raised(&irqs));
    m.read32(RDR_ADDR);
    assert!(nothing_raised(&irqs));
}

#[test]
fn reading_with_rxneie_clear_raises_nothing() {
    let (mut m, console, irqs) = machine_with_irqs();
    console.push_input(b"hi");
    m.write32(CR1_ADDR, UE | RE);
    m.read32(RDR_ADDR);
    assert!(nothing_raised(&irqs));
}

#[test]
fn pushed_input_raises_the_line_when_rxneie_is_set() {
    let (mut m, console, irqs) = machine_with_irqs();
    m.write32(CR1_ADDR, UE | RE | RXNEIE);
    console.push_input(b"1");
    assert!(raised(&irqs));
}

#[test]
fn pushed_input_raises_nothing_with_rxneie_clear() {
    let (mut m, console, irqs) = machine_with_irqs();
    m.write32(CR1_ADDR, UE | RE);
    console.push_input(b"1");
    assert!(nothing_raised(&irqs));
}

#[test]
fn a_second_push_while_input_waits_raises_nothing() {
    let (mut m, console, irqs) = machine_with_irqs();
    m.write32(CR1_ADDR, UE | RE | RXNEIE);
    console.push_input(b"1");
    assert!(raised(&irqs));
    // The first byte is still queued, so the line never dropped and has nothing new to say.
    console.push_input(b"2");
    assert!(nothing_raised(&irqs));
}

#[test]
fn pushing_nothing_raises_nothing() {
    let (mut m, console, irqs) = machine_with_irqs();
    m.write32(CR1_ADDR, UE | RE | RXNEIE);
    console.push_input(b"");
    assert!(nothing_raised(&irqs));
}

#[test]
fn isr_at_rest_has_the_transmitter_empty_only() {
    let (mut m, _) = machine();
    assert_eq!(m.read32(ISR_ADDR), 0xC0);
}

#[test]
fn teack_and_reack_follow_ue_with_te_and_re() {
    let (mut m, _) = machine();
    m.write32(CR1_ADDR, UE | TE);
    assert_eq!(m.read32(ISR_ADDR), 0x0020_00C0);
    m.write32(CR1_ADDR, UE | RE);
    assert_eq!(m.read32(ISR_ADDR), 0x0040_00C0);
    m.write32(CR1_ADDR, UE | TE | RE);
    assert_eq!(m.read32(ISR_ADDR), 0x0060_00C0);
}

#[test]
fn nothing_is_acknowledged_while_ue_is_clear() {
    let (mut m, _) = machine();
    m.write32(CR1_ADDR, TE | RE);
    assert_eq!(m.read32(ISR_ADDR), 0xC0);
}

#[test]
fn the_fifo_is_empty_and_below_its_threshold() {
    let (mut m, _) = machine();
    m.write32(CR1_ADDR, FIFOEN);
    assert_eq!(m.read32(ISR_ADDR), 0x0880_00C0);
}

#[test]
fn the_boards_cr1_reads_the_modelled_bits_of_its_isr() {
    let (mut m, _) = machine();
    m.write32(CR1_ADDR, 0x2000_002D);
    let isr = m.read32(ISR_ADDR);
    assert_eq!(isr, 0x08E0_00C0);
    // The board also has CMF, EOBF and IDLE set, which are not modelled.
    let board = 0x08E2_10D0;
    assert_eq!(isr & board, isr);
}

#[test]
fn rxne_follows_the_console_input() {
    let (mut m, console) = machine();
    assert_eq!(m.read32(ISR_ADDR) & RXNE, 0);
    console.push_input(b"A");
    assert_eq!(m.read32(ISR_ADDR) & RXNE, RXNE);
    assert_eq!(m.read32(RDR_ADDR), 0x41);
    assert_eq!(m.read32(ISR_ADDR) & RXNE, 0);
}

#[test]
fn received_bytes_read_back_in_order() {
    let (mut m, console) = machine();
    console.push_input(b"hi");
    assert_eq!(m.read32(RDR_ADDR), u32::from(b'h'));
    assert_eq!(m.read32(ISR_ADDR) & RXNE, RXNE);
    assert_eq!(m.read32(RDR_ADDR), u32::from(b'i'));
    assert_eq!(m.read32(ISR_ADDR) & RXNE, 0);
}

#[test]
fn rdr_with_no_input_reads_zero() {
    let (mut m, _) = machine();
    assert_eq!(m.read32(RDR_ADDR), 0);
}

#[test]
fn tdr_writes_are_captured() {
    let (mut m, console) = machine();
    m.write32(CR1_ADDR, UE | TE);
    m.write32(TDR_ADDR, 0x41);
    m.write32(TDR_ADDR, 0x42);
    assert_eq!(console.take_output(), b"AB");
}

#[test]
fn a_byte_write_to_tdr_is_captured() {
    let (mut m, console) = machine();
    m.write32(CR1_ADDR, UE | TE);
    m.write8(TDR_ADDR, 0x43);
    assert_eq!(console.take_output(), b"C");
}

#[test]
fn only_the_low_eight_bits_are_kept() {
    let (mut m, console) = machine();
    m.write32(CR1_ADDR, UE | TE);
    m.write32(TDR_ADDR, 0x1FF);
    assert_eq!(console.take_output(), [0xFF]);
}

#[test]
fn nothing_is_sent_with_te_clear() {
    let (mut m, console) = machine();
    m.write32(CR1_ADDR, UE | RE);
    m.write32(TDR_ADDR, 0x41);
    assert!(console.take_output().is_empty());
}

#[test]
fn nothing_is_sent_with_ue_clear() {
    let (mut m, console) = machine();
    m.write32(CR1_ADDR, TE);
    m.write32(TDR_ADDR, 0x41);
    assert!(console.take_output().is_empty());
}

#[test]
fn icr_rqr_and_isr_writes_change_nothing_readable() {
    let (mut m, _) = machine();
    m.write32(CR1_ADDR, UE | TE);
    m.write32(ICR_ADDR, 0xFFFF_FFFF);
    m.write32(RQR_ADDR, 0xFFFF_FFFF);
    m.write32(ISR_ADDR, 0xFFFF_FFFF);
    assert_eq!(m.read32(ISR_ADDR), 0x0020_00C0);
    assert_eq!(m.read32(ICR_ADDR), 0);
    assert_eq!(m.read32(RQR_ADDR), 0);
}

#[test]
fn other_registers_keep_their_reset_values_and_store_writes() {
    let (mut m, _) = machine();
    let svd = Svd::stm32h743();
    for addr in [CR2_ADDR, BRR_ADDR] {
        let reset = svd.lookup(addr).unwrap().reset_value;
        assert_eq!(m.read32(addr), reset);
    }
    m.write32(BRR_ADDR, 0x1234);
    m.write32(CR2_ADDR, 0x0000_2000);
    assert_eq!(m.read32(BRR_ADDR), 0x1234);
    assert_eq!(m.read32(CR2_ADDR), 0x0000_2000);
}

#[test]
fn cr1_stores_all_its_bits() {
    let (mut m, _) = machine();
    m.write32(CR1_ADDR, 0x2000_00FD);
    assert_eq!(m.read32(CR1_ADDR), 0x2000_00FD);
}

#[test]
fn reset_puts_cr1_back_and_stops_the_receive_interrupt() {
    let (mut m, console, irqs) = machine_with_irqs();
    m.write32(CR1_ADDR, UE | TE | RE | RXNEIE);
    m.write32(BRR_ADDR, 0x1234);
    let _ = irqs.take();
    m.reset_peripherals(ResetCause::Software);
    assert_eq!(m.read32(CR1_ADDR), 0);
    assert_eq!(m.read32(BRR_ADDR), 0);
    console.push_input(b"x");
    assert!(nothing_raised(&irqs));
}

#[test]
fn reset_leaves_the_consoles_queues_alone() {
    let (mut m, console, _) = machine_with_irqs();
    m.write32(CR1_ADDR, UE | TE);
    m.write32(TDR_ADDR, 0x41);
    console.push_input(b"hi");
    m.reset_peripherals(ResetCause::Software);
    assert_eq!(console.take_output(), b"A");
    assert_eq!(console.read(), Some(b'h'));
    assert_eq!(console.read(), Some(b'i'));
}
