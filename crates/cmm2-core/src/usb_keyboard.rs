//! A low-speed HID boot keyboard, the device the firmware's USB stack enumerates. It answers
//! the control requests the firmware sends and hands out its 8-byte report on the interrupt
//! endpoint when it has changed. The report itself is a shared handle: whoever drives the
//! keys sets it, and the device sends it the next time the firmware polls.

use std::cell::Cell;
use std::rc::Rc;

use crate::usb::{In, Out, UsbDevice};

// The descriptors are my own: a standard boot keyboard, USB 1.10, one configuration with one
// HID interface and one interrupt IN endpoint.
const DEVICE_DESCRIPTOR: [u8; 18] = [
    0x12, 0x01, 0x10, 0x01, 0x00, 0x00, 0x00, 0x08, 0x34, 0x12, 0x78, 0x56, 0x00, 0x01, 0x00, 0x00,
    0x00, 0x01,
];
const CONFIGURATION: [u8; 34] = [
    // Configuration: 34 bytes in all, one interface, bus powered, 100 mA.
    0x09, 0x02, 0x22, 0x00, 0x01, 0x01, 0x00, 0xA0, 0x32,
    // Interface 0: HID, boot subclass, keyboard protocol.
    0x09, 0x04, 0x00, 0x00, 0x01, 0x03, 0x01, 0x01, 0x00,
    // HID: one report descriptor, of 63 bytes.
    0x09, 0x21, 0x11, 0x01, 0x00, 0x01, 0x22, 0x3F, 0x00,
    // Endpoint 1: interrupt IN, 8 bytes, every 10 ms.
    0x07, 0x05, 0x81, 0x03, 0x08, 0x00, 0x0A,
];

// The standard boot keyboard report: a byte of modifier bits, a reserved byte, six key usages,
// and a byte out for the five LEDs.
const REPORT_DESCRIPTOR: [u8; 63] = [
    0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01,
    0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0x95, 0x01, 0x75, 0x08, 0x81, 0x01, 0x95, 0x05, 0x75, 0x01,
    0x05, 0x08, 0x19, 0x01, 0x29, 0x05, 0x91, 0x02, 0x95, 0x01, 0x75, 0x03, 0x91, 0x01, 0x95, 0x06,
    0x75, 0x08, 0x15, 0x00, 0x25, 0x65, 0x05, 0x07, 0x19, 0x00, 0x29, 0x65, 0x81, 0x00, 0xC0,
];

const GET_DESCRIPTOR: u8 = 0x06;
const SET_ADDRESS: u8 = 0x05;
const SET_CONFIGURATION: u8 = 0x09;
const HID_SET_REPORT: u8 = 0x09;
const HID_SET_IDLE: u8 = 0x0A;
const HID_SET_PROTOCOL: u8 = 0x0B;

const DEVICE: u8 = 1;
const CONFIGURATION_TYPE: u8 = 2;
const HID_REPORT: u8 = 0x22;

/// The keys that are down, as the 8-byte boot report: modifier bits, a reserved byte and up
/// to six usage codes. A cheap handle: clones share the report, so the keyboard device and
/// whatever drives the keys each keep one.
#[derive(Clone, Debug, Default)]
pub struct KeyReport(Rc<Cell<[u8; 8]>>);

impl KeyReport {
    pub fn new() -> KeyReport {
        KeyReport::default()
    }

    pub fn set(&self, report: [u8; 8]) {
        self.0.set(report);
    }

    pub fn get(&self) -> [u8; 8] {
        self.0.get()
    }
}

/// The device. The firmware asks it for the report with an interrupt IN token every few
/// milliseconds, and the device says NAK until the report has changed.
pub struct HidKeyboard {
    keys: KeyReport,
    /// The last report the firmware was sent, so a report goes out once per change.
    sent: [u8; 8],
    address: u8,
    /// SET_ADDRESS has been asked for; the device answers on the new one after the status stage.
    new_address: Option<u8>,
    configured: bool,
    /// What is left of the answer to the control request in progress, or the stall that is.
    control: Control,
    /// The data stage of a SET_REPORT is next.
    expect_leds: bool,
    leds: u8,
}

enum Control {
    Data(Vec<u8>),
    Stall,
}

impl HidKeyboard {
    pub fn new(keys: KeyReport) -> HidKeyboard {
        HidKeyboard {
            keys,
            sent: [0; 8],
            address: 0,
            new_address: None,
            configured: false,
            control: Control::Data(Vec::new()),
            expect_leds: false,
            leds: 0,
        }
    }

    /// The LED byte the firmware last set: num, caps and scroll lock in bits 0 to 2.
    pub fn leds(&self) -> u8 {
        self.leds
    }

    // What a GET_DESCRIPTOR of `kind` and `index` is answered with.
    fn descriptor(request_type: u8, kind: u8, index: u8) -> Option<&'static [u8]> {
        match (request_type, kind, index) {
            (0x80, DEVICE, 0) => Some(&DEVICE_DESCRIPTOR),
            (0x80, CONFIGURATION_TYPE, 0) => Some(&CONFIGURATION),
            (0x81, HID_REPORT, 0) => Some(&REPORT_DESCRIPTOR),
            _ => None,
        }
    }

    // The control request, which is either answered with data (possibly none), or stalls.
    fn answer(&mut self, packet: [u8; 8]) -> Option<Vec<u8>> {
        let [
            request_type,
            request,
            value_low,
            value_high,
            _,
            _,
            length_low,
            length_high,
        ] = packet;
        match (request_type, request) {
            (_, GET_DESCRIPTOR) => {
                let data = Self::descriptor(request_type, value_high, value_low)?;
                let length = usize::from(u16::from_le_bytes([length_low, length_high]));
                Some(data[..data.len().min(length)].to_vec())
            }
            (0x00, SET_ADDRESS) => {
                self.new_address = Some(value_low & 0x7F);
                Some(Vec::new())
            }
            (0x00, SET_CONFIGURATION) if value_low <= 1 => {
                self.configured = value_low == 1;
                Some(Vec::new())
            }
            (0x21, HID_SET_REPORT) => {
                self.expect_leds = true;
                Some(Vec::new())
            }
            (0x21, HID_SET_IDLE | HID_SET_PROTOCOL) => Some(Vec::new()),
            _ => None,
        }
    }
}

impl UsbDevice for HidKeyboard {
    fn address(&self) -> u8 {
        self.address
    }

    fn reset(&mut self) {
        *self = HidKeyboard::new(self.keys.clone());
    }

    fn setup(&mut self, packet: [u8; 8]) {
        self.expect_leds = false;
        self.control = match self.answer(packet) {
            Some(data) => Control::Data(data),
            None => Control::Stall,
        };
    }

    // The status stage of a request that reads, or the data stage of SET_REPORT.
    fn out(&mut self, endpoint: u8, data: &[u8]) -> Out {
        if endpoint != 0 || matches!(self.control, Control::Stall) {
            return Out::Stall;
        }
        if let (true, Some(&leds)) = (self.expect_leds, data.first()) {
            self.leds = leds;
            self.expect_leds = false;
        }
        Out::Ack
    }

    fn in_token(&mut self, endpoint: u8, max: usize) -> In {
        match (endpoint, &mut self.control) {
            (0, Control::Stall) => In::Stall,
            (0, Control::Data(data)) => {
                let packet: Vec<u8> = data.drain(..data.len().min(max)).collect();
                // Nothing left to send is the status stage of a request without data.
                if let Some(address) = self.new_address.take() {
                    self.address = address;
                }
                In::Data(packet)
            }
            (1, _) if self.configured => {
                let report = self.keys.get();
                if report == self.sent {
                    return In::Nak;
                }
                self.sent = report;
                In::Data(report.to_vec())
            }
            _ => In::Stall,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyboard() -> (HidKeyboard, KeyReport) {
        let report = KeyReport::new();
        (HidKeyboard::new(report.clone()), report)
    }

    /// A control transfer that reads from the device: SETUP, then IN packets of 8 bytes until
    /// a short one. Returns what came back, or None if the device stalled.
    fn control_in(kb: &mut HidKeyboard, setup: [u8; 8]) -> Option<Vec<u8>> {
        kb.setup(setup);
        let mut data = Vec::new();
        loop {
            match kb.in_token(0, 8) {
                In::Data(packet) => {
                    let last = packet.len() < 8;
                    data.extend(packet);
                    if last {
                        return Some(data);
                    }
                }
                In::Stall => return None,
                In::Nak => panic!("endpoint 0 never NAKs"),
            }
        }
    }

    /// A request with no data stage: SETUP, then the status stage, a zero-length IN.
    fn control_no_data(kb: &mut HidKeyboard, setup: [u8; 8]) -> bool {
        kb.setup(setup);
        kb.in_token(0, 8) == In::Data(Vec::new())
    }

    fn get_descriptor(kind: u8, index: u8, length: u16) -> [u8; 8] {
        [
            0x80,
            0x06,
            index,
            kind,
            0,
            0,
            length as u8,
            (length >> 8) as u8,
        ]
    }

    #[test]
    fn it_starts_unaddressed() {
        assert_eq!(keyboard().0.address(), 0);
    }

    #[test]
    fn the_device_descriptor_comes_in_one_packet_when_asked_for_eight_bytes() {
        let (mut kb, _) = keyboard();
        let data = control_in(&mut kb, get_descriptor(1, 0, 8)).unwrap();
        assert_eq!(data, DEVICE_DESCRIPTOR[..8]);
        assert_eq!(data[7], 8, "bMaxPacketSize0");
    }

    #[test]
    fn the_whole_device_descriptor_comes_in_three_packets() {
        let (mut kb, _) = keyboard();
        let data = control_in(&mut kb, get_descriptor(1, 0, 18)).unwrap();
        assert_eq!(data, DEVICE_DESCRIPTOR);
    }

    #[test]
    fn a_request_for_more_than_there_is_gets_all_of_it() {
        let (mut kb, _) = keyboard();
        let data = control_in(&mut kb, get_descriptor(1, 0, 255)).unwrap();
        assert_eq!(data, DEVICE_DESCRIPTOR);
    }

    #[test]
    fn the_configuration_is_what_the_host_asked_for_in_two_goes() {
        let (mut kb, _) = keyboard();
        let head = control_in(&mut kb, get_descriptor(2, 0, 9)).unwrap();
        assert_eq!(head, CONFIGURATION[..9]);
        let total = usize::from(head[2]) | usize::from(head[3]) << 8;
        let all = control_in(&mut kb, get_descriptor(2, 0, total as u16)).unwrap();
        assert_eq!(all, CONFIGURATION);
    }

    #[test]
    fn the_descriptors_agree_with_each_other() {
        assert_eq!(DEVICE_DESCRIPTOR.len(), usize::from(DEVICE_DESCRIPTOR[0]));
        let total = usize::from(CONFIGURATION[2]) | usize::from(CONFIGURATION[3]) << 8;
        assert_eq!(CONFIGURATION.len(), total);
        // The HID descriptor follows the interface descriptor, and names the report descriptor.
        let hid = &CONFIGURATION[18..27];
        assert_eq!(hid[1], 0x21);
        assert_eq!(hid[7], REPORT_DESCRIPTOR.len() as u8);
        // The endpoint is interrupt IN, endpoint 1, 8 bytes.
        assert_eq!(&CONFIGURATION[27..], [7, 5, 0x81, 3, 8, 0, 10]);
    }

    #[test]
    fn the_report_descriptor_is_read_from_the_interface() {
        let (mut kb, _) = keyboard();
        let data = control_in(&mut kb, [0x81, 0x06, 0, 0x22, 0, 0, 63, 0]).unwrap();
        assert_eq!(data, REPORT_DESCRIPTOR);
        assert_eq!(data.len(), 63);
    }

    #[test]
    fn the_report_descriptor_is_not_a_device_descriptor_request() {
        let (mut kb, _) = keyboard();
        // The same wValue to the device (bmRequestType 0x80) is not a thing the device has.
        assert_eq!(
            control_in(&mut kb, [0x80, 0x06, 0, 0x22, 0, 0, 63, 0]),
            None
        );
    }

    #[test]
    fn strings_and_other_descriptors_stall() {
        let (mut kb, _) = keyboard();
        assert_eq!(control_in(&mut kb, get_descriptor(3, 0, 255)), None);
        assert_eq!(control_in(&mut kb, get_descriptor(6, 0, 10)), None);
        // And a request for a second configuration.
        assert_eq!(control_in(&mut kb, get_descriptor(2, 1, 9)), None);
    }

    #[test]
    fn an_unknown_request_stalls() {
        let (mut kb, _) = keyboard();
        assert!(!control_no_data(&mut kb, [0x00, 0x03, 0, 0, 0, 0, 0, 0]));
    }

    #[test]
    fn a_stall_lasts_for_the_transfer_only() {
        let (mut kb, _) = keyboard();
        assert_eq!(control_in(&mut kb, get_descriptor(3, 0, 255)), None);
        assert_eq!(kb.out(0, &[]), Out::Stall);
        assert!(control_in(&mut kb, get_descriptor(1, 0, 18)).is_some());
        assert_eq!(kb.out(0, &[]), Out::Ack);
    }

    #[test]
    fn set_address_takes_effect_after_the_status_stage() {
        let (mut kb, _) = keyboard();
        kb.setup([0x00, 0x05, 1, 0, 0, 0, 0, 0]);
        assert_eq!(kb.address(), 0);
        assert_eq!(kb.in_token(0, 8), In::Data(Vec::new()));
        assert_eq!(kb.address(), 1);
    }

    #[test]
    fn the_address_is_seven_bits() {
        let (mut kb, _) = keyboard();
        assert!(control_no_data(&mut kb, [0x00, 0x05, 0x81, 0, 0, 0, 0, 0]));
        assert_eq!(kb.address(), 1);
    }

    #[test]
    fn a_bus_reset_takes_the_address_and_the_configuration_back() {
        let (mut kb, _) = keyboard();
        assert!(control_no_data(&mut kb, [0x00, 0x05, 5, 0, 0, 0, 0, 0]));
        assert!(control_no_data(&mut kb, [0x00, 0x09, 1, 0, 0, 0, 0, 0]));
        assert_eq!(kb.in_token(1, 8), In::Nak);
        kb.reset();
        assert_eq!(kb.address(), 0);
        assert_eq!(kb.in_token(1, 8), In::Stall);
    }

    #[test]
    fn set_configuration_one_enables_the_interrupt_endpoint() {
        let (mut kb, _) = keyboard();
        assert_eq!(kb.in_token(1, 8), In::Stall);
        assert!(control_no_data(&mut kb, [0x00, 0x09, 1, 0, 0, 0, 0, 0]));
        assert_eq!(kb.in_token(1, 8), In::Nak);
    }

    #[test]
    fn set_configuration_zero_disables_it_again() {
        let (mut kb, _) = keyboard();
        assert!(control_no_data(&mut kb, [0x00, 0x09, 1, 0, 0, 0, 0, 0]));
        assert!(control_no_data(&mut kb, [0x00, 0x09, 0, 0, 0, 0, 0, 0]));
        assert_eq!(kb.in_token(1, 8), In::Stall);
    }

    #[test]
    fn a_configuration_that_does_not_exist_stalls() {
        let (mut kb, _) = keyboard();
        assert!(!control_no_data(&mut kb, [0x00, 0x09, 2, 0, 0, 0, 0, 0]));
        assert_eq!(kb.in_token(1, 8), In::Stall);
    }

    #[test]
    fn the_hid_class_requests_are_accepted() {
        let (mut kb, _) = keyboard();
        assert!(
            control_no_data(&mut kb, [0x21, 0x0A, 0, 0, 0, 0, 0, 0]),
            "SET_IDLE"
        );
        assert!(
            control_no_data(&mut kb, [0x21, 0x0B, 0, 0, 0, 0, 0, 0]),
            "SET_PROTOCOL"
        );
    }

    #[test]
    fn set_report_takes_the_led_byte() {
        let (mut kb, _) = keyboard();
        assert_eq!(kb.leds(), 0);
        kb.setup([0x21, 0x09, 0, 0x02, 0, 0, 1, 0]);
        assert_eq!(kb.out(0, &[0b010]), Out::Ack);
        assert_eq!(kb.in_token(0, 8), In::Data(Vec::new()));
        assert_eq!(kb.leds(), 0b010);
    }

    #[test]
    fn only_the_data_of_a_set_report_is_an_led_byte() {
        let (mut kb, _) = keyboard();
        kb.setup([0x21, 0x09, 0, 0x02, 0, 0, 1, 0]);
        kb.out(0, &[0b001]);
        // One byte, once.
        kb.out(0, &[0b111]);
        assert_eq!(kb.leds(), 0b001);
        // Not after a SET_REPORT that was given up on, either.
        kb.setup([0x21, 0x09, 0, 0x02, 0, 0, 1, 0]);
        control_in(&mut kb, get_descriptor(1, 0, 8)).unwrap();
        kb.out(0, &[0b111]);
        assert_eq!(kb.leds(), 0b001);
    }

    #[test]
    fn the_status_stage_of_a_read_is_an_empty_out() {
        let (mut kb, _) = keyboard();
        control_in(&mut kb, get_descriptor(1, 0, 8)).unwrap();
        assert_eq!(kb.out(0, &[]), Out::Ack);
        assert_eq!(kb.leds(), 0);
    }

    #[test]
    fn a_descriptor_is_asked_of_the_device_or_of_the_interface_it_belongs_to() {
        let (mut kb, _) = keyboard();
        assert_eq!(control_in(&mut kb, [0x81, 0x06, 0, 1, 0, 0, 18, 0]), None);
        assert_eq!(control_in(&mut kb, [0x81, 0x06, 0, 2, 0, 0, 9, 0]), None);
        assert_eq!(
            control_in(&mut kb, [0x80, 0x06, 0, 0x22, 0, 0, 63, 0]),
            None
        );
    }

    #[test]
    fn it_never_sends_more_than_the_packet_size_it_is_asked_for() {
        let (mut kb, _) = keyboard();
        kb.setup(get_descriptor(1, 0, 18));
        let sizes: Vec<usize> = (0..5)
            .map(|_| match kb.in_token(0, 5) {
                In::Data(packet) => packet.len(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(sizes, [5, 5, 5, 3, 0]);
    }

    #[test]
    fn a_new_setup_drops_what_was_left_of_the_last_transfer() {
        let (mut kb, _) = keyboard();
        kb.setup(get_descriptor(2, 0, 34));
        assert!(matches!(kb.in_token(0, 8), In::Data(_)));
        // The host gave up on it, and asks something else.
        assert_eq!(
            control_in(&mut kb, get_descriptor(1, 0, 8)).unwrap(),
            DEVICE_DESCRIPTOR[..8]
        );
    }

    #[test]
    fn other_endpoints_stall() {
        let (mut kb, _) = keyboard();
        assert_eq!(kb.in_token(2, 8), In::Stall);
        assert_eq!(kb.out(1, &[1]), Out::Stall);
    }

    fn configured() -> (HidKeyboard, KeyReport) {
        let (mut kb, report) = keyboard();
        assert!(control_no_data(&mut kb, [0x00, 0x09, 1, 0, 0, 0, 0, 0]));
        (kb, report)
    }

    #[test]
    fn it_naks_while_no_key_has_changed() {
        let (mut kb, _) = configured();
        assert_eq!(kb.in_token(1, 8), In::Nak);
        assert_eq!(kb.in_token(1, 8), In::Nak);
    }

    #[test]
    fn a_changed_report_is_sent_once() {
        let (mut kb, report) = configured();
        let a = [0, 0, 0x04, 0, 0, 0, 0, 0];
        report.set(a);
        assert_eq!(kb.in_token(1, 8), In::Data(a.to_vec()));
        assert_eq!(kb.in_token(1, 8), In::Nak);
    }

    #[test]
    fn releasing_the_key_sends_the_empty_report() {
        let (mut kb, report) = configured();
        report.set([0, 0, 0x04, 0, 0, 0, 0, 0]);
        kb.in_token(1, 8);
        report.set([0; 8]);
        assert_eq!(kb.in_token(1, 8), In::Data(vec![0; 8]));
        assert_eq!(kb.in_token(1, 8), In::Nak);
    }

    #[test]
    fn a_key_held_before_the_first_poll_is_reported_at_it() {
        let (mut kb, report) = keyboard();
        report.set([2, 0, 0x04, 0, 0, 0, 0, 0]);
        assert!(control_no_data(&mut kb, [0x00, 0x09, 1, 0, 0, 0, 0, 0]));
        assert_eq!(kb.in_token(1, 8), In::Data(vec![2, 0, 4, 0, 0, 0, 0, 0]));
    }

    #[test]
    fn a_bus_reset_sends_the_report_again() {
        let (mut kb, report) = configured();
        report.set([0, 0, 0x04, 0, 0, 0, 0, 0]);
        kb.in_token(1, 8);
        kb.reset();
        assert!(control_no_data(&mut kb, [0x00, 0x09, 1, 0, 0, 0, 0, 0]));
        assert_eq!(kb.in_token(1, 8), In::Data(vec![0, 0, 4, 0, 0, 0, 0, 0]));
    }

    #[test]
    fn the_report_is_shared_between_clones() {
        let report = KeyReport::new();
        assert_eq!(report.get(), [0; 8]);
        report.clone().set([1; 8]);
        assert_eq!(report.get(), [1; 8]);
    }
}
