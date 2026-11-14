//!
//! Cortex Instruction Trace Macrocell simulation
//!

use crate::Processor;

///
/// ITM peripheral API via register access
///
pub trait InstrumentationTraceMacrocell {
    ///
    /// read value of stim0 register
    ///
    fn read_stim0(&self) -> u32;

    ///
    /// write value with 32 bit width to given STIM port
    /// data is sent in little endian order
    ///
    fn write_stim_u32(&mut self, port: u8, value: u32);

    ///
    /// write value with 16 bit width to given STIM port
    /// data is sent in little endian order
    ///
    fn write_stim_u16(&mut self, port: u8, value: u16);

    ///
    /// write value with 8 bit width to given STIM port
    /// data is sent in little endian order
    ///
    fn write_stim_u8(&mut self, port: u8, value: u8);

    ///
    /// write value to LAR register. (Lock Access Register)
    /// Value of `0xC5AC_CE55` unlocks the access to debug registers.
    ///
    fn itm_write_lar_u32(&mut self, value: u32);
}

trait InstrumentationTraceMacrocellHelper {
    fn write_itm_packet(&mut self, packet: Vec<u8>);
}

fn make_header(port: u8, payload_size: usize) -> u8 {
    // The stimulus writes are 1, 2 or 4 bytes, and SS says which as 1, 2 or 3: log2(size) + 1.
    let ss = payload_size.trailing_zeros() as u8 + 1;

    ((port & 0b11111) << 3) + ss
}

fn make_instrumentation_packet(port: u8, payload: &[u8]) -> Vec<u8> {
    // header is single byte
    // ----------------------
    // bits 0..=1 : SS, payload size = (1 << (SS -1)) bytes
    // bit 2 = 0
    // bits 3..=7: A[4:0], port number, 0-31
    //
    // followed by payload size bytes of payload, lsb order
    let mut packet = Vec::new();
    packet.push(make_header(port, payload.len()));
    packet.extend(payload);
    packet
}

impl InstrumentationTraceMacrocellHelper for Processor {
    fn write_itm_packet(&mut self, packet: Vec<u8>) {
        // A trace file that can not be written to (a full disk, a closed pipe) stops the tracing,
        // and the guest runs on without it. The error is reported once, as the file goes.
        if let Some(f) = &mut self.itm_file
            && let Err(error) = f.write_all(packet.as_slice()).and_then(|()| f.flush())
        {
            log::warn!("ITM trace file write failed, tracing stopped: {error}");
            self.itm_file = None;
        }
    }
}

impl InstrumentationTraceMacrocell for Processor {
    fn read_stim0(&self) -> u32 {
        // return 0 if fifo is full, 1 otherwise
        1
    }

    fn write_stim_u32(&mut self, port: u8, value: u32) {
        let payload: [u8; 4] = [
            (value & 0xff) as u8,
            ((value & 0xff00) >> 8) as u8,
            ((value & 0xff_0000) >> 16) as u8,
            ((value & 0xff00_0000) >> 24) as u8,
        ];
        self.write_itm_packet(make_instrumentation_packet(port, &payload));
    }

    fn itm_write_lar_u32(&mut self, _value: u32) {}

    fn write_stim_u16(&mut self, port: u8, value: u16) {
        let payload: [u8; 2] = [(value & 0xff) as u8, ((value & 0xff00) >> 8) as u8];
        self.write_itm_packet(make_instrumentation_packet(port, &payload));
    }

    fn write_stim_u8(&mut self, port: u8, value: u8) {
        let payload: [u8; 1] = [value];
        self.write_itm_packet(make_instrumentation_packet(port, &payload));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io;
    use std::rc::Rc;

    struct Sink(Rc<RefCell<Vec<u8>>>);

    impl io::Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct Broken;

    impl io::Write for Broken {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("disk full"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("disk full"))
        }
    }

    #[test]
    fn stimulus_writes_of_each_width_make_packets_of_the_right_size() {
        let written = Rc::new(RefCell::new(Vec::new()));
        let mut processor = Processor::new();
        processor.itm(Some(Box::new(Sink(written.clone()))));

        processor.write_stim_u8(1, 0xaa);
        processor.write_stim_u16(2, 0xbbcc);
        processor.write_stim_u32(3, 0x1122_3344);

        assert_eq!(
            *written.borrow(),
            vec![
                1 << 3 | 1,
                0xaa,
                2 << 3 | 2,
                0xcc,
                0xbb,
                3 << 3 | 3,
                0x44,
                0x33,
                0x22,
                0x11
            ]
        );
    }

    struct Capture(std::sync::Mutex<Vec<String>>);

    impl log::Log for Capture {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }
        fn log(&self, record: &log::Record) {
            self.0.lock().unwrap().push(format!("{}", record.args()));
        }
        fn flush(&self) {}
    }

    static CAPTURE: Capture = Capture(std::sync::Mutex::new(Vec::new()));

    #[test]
    fn a_trace_file_that_fails_is_reported_once_and_stops_the_tracing() {
        log::set_logger(&CAPTURE).expect("no other test sets a logger");
        log::set_max_level(log::LevelFilter::Warn);
        let mut processor = Processor::new();
        processor.itm(Some(Box::new(Broken)));

        processor.write_stim_u32(0, 1);

        assert!(processor.itm_file.is_none());
        // the file is gone, so the second write has nothing to fail on, and nothing is reported
        processor.write_stim_u32(0, 2);
        let reports = CAPTURE.0.lock().unwrap();
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert!(reports[0].contains("disk full"), "{reports:?}");
    }
}
