use super::*;

// A card of `blocks` blocks, each filled with its own number plus one.
fn card(blocks: usize) -> SdCard {
    SdCard::new(card_image(blocks)).unwrap()
}

// The image `card(blocks)` starts with.
fn card_image(blocks: usize) -> Vec<u8> {
    (0..blocks).flat_map(|b| [b as u8 + 1; BLOCK]).collect()
}

// One byte over the wire: the card's reply to it, as the host reads it.
fn exchange(card: &mut SdCard, byte: u8) -> u8 {
    let reply = card.miso();
    card.mosi(byte);
    reply
}

// Sends a command frame (the CRC is never checked) and returns the bytes of the reply, up
// to `len` after the first byte with a clear top bit, which is the R1.
fn command(card: &mut SdCard, index: u8, arg: u32, len: usize) -> Vec<u8> {
    for byte in [0x40 | index]
        .into_iter()
        .chain(arg.to_be_bytes())
        .chain([0x01])
    {
        exchange(card, byte);
    }
    let mut reply = Vec::new();
    for _ in 0..10 {
        let byte = exchange(card, 0xFF);
        if reply.is_empty() && byte & 0x80 != 0 {
            continue;
        }
        reply.push(byte);
        if reply.len() == len {
            break;
        }
    }
    reply
}

// CMD0, CMD8, then CMD55 and ACMD41 until the card is ready.
fn init(card: &mut SdCard) {
    assert_eq!(command(card, 0, 0, 1), [R1_IDLE]);
    assert_eq!(command(card, 8, 0x1AA, 5), [R1_IDLE, 0, 0, 0x01, 0xAA]);
    for _ in 0..10 {
        assert!(command(card, 55, 0, 1)[0] <= R1_IDLE);
        if command(card, 41, 0x4000_0000, 1) == [0] {
            return;
        }
    }
    panic!("the card never became ready");
}

fn read_block(card: &mut SdCard, block: u32) -> Vec<u8> {
    assert_eq!(command(card, 17, block, 1), [0]);
    // The token comes straight after the R1, and then the data.
    assert_eq!(exchange(card, 0xFF), 0xFE);
    let data = (0..BLOCK).map(|_| exchange(card, 0xFF)).collect();
    assert_eq!([exchange(card, 0xFF), exchange(card, 0xFF)], [0xFF, 0xFF]);
    data
}

// The data phase of CMD24: token, data, two CRC bytes. Returns the data response.
fn write_block(card: &mut SdCard, block: u32, fill: u8) -> u8 {
    assert_eq!(command(card, 24, block, 1), [0]);
    exchange(card, 0xFF);
    exchange(card, 0xFE);
    for _ in 0..BLOCK {
        exchange(card, fill);
    }
    exchange(card, 0xFF);
    exchange(card, 0xFF);
    exchange(card, 0xFF)
}

#[test]
fn an_image_of_whole_blocks_is_a_card() {
    assert!(SdCard::new(vec![0; BLOCK]).is_ok());
    assert!(SdCard::new(vec![0; 64 * BLOCK]).is_ok());
}

#[test]
fn an_image_that_is_not_whole_blocks_is_refused() {
    assert_eq!(
        SdCard::new(vec![0; BLOCK + 1]).unwrap_err(),
        ImageSizeError(BLOCK + 1)
    );
    assert_eq!(SdCard::new(Vec::new()).unwrap_err(), ImageSizeError(0));
}

#[test]
fn the_error_says_what_was_wrong() {
    assert_eq!(
        ImageSizeError(1000).to_string(),
        "1000 bytes is not a whole number of 512-byte blocks"
    );
}

#[test]
fn a_card_that_is_asked_nothing_reads_ff() {
    let mut card = card(1);
    for _ in 0..20 {
        assert_eq!(exchange(&mut card, 0xFF), 0xFF);
    }
}

#[test]
fn cmd0_answers_idle() {
    let mut card = card(1);
    assert_eq!(command(&mut card, 0, 0, 1), [R1_IDLE]);
}

#[test]
fn the_reply_comes_one_byte_after_the_command() {
    let mut card = card(1);
    for byte in [0x40, 0, 0, 0, 0, 0x95] {
        assert_eq!(exchange(&mut card, byte), 0xFF);
    }
    assert_eq!(exchange(&mut card, 0xFF), 0xFF);
    assert_eq!(exchange(&mut card, 0xFF), R1_IDLE);
    assert_eq!(exchange(&mut card, 0xFF), 0xFF);
}

#[test]
fn cmd8_echoes_the_voltage_and_the_check_pattern() {
    let mut card = card(1);
    assert_eq!(
        command(&mut card, 8, 0x0000_01AA, 5),
        [R1_IDLE, 0, 0, 0x01, 0xAA]
    );
    assert_eq!(
        command(&mut card, 8, 0x0000_0155, 5),
        [R1_IDLE, 0, 0, 0x01, 0x55]
    );
}

#[test]
fn acmd41_makes_the_card_ready() {
    let mut card = card(1);
    command(&mut card, 0, 0, 1);
    command(&mut card, 55, 0, 1);
    assert_eq!(command(&mut card, 41, 0x4000_0000, 1), [0]);
    assert_eq!(command(&mut card, 55, 0, 1), [0]);
}

#[test]
fn cmd41_without_cmd55_in_front_is_illegal() {
    let mut card = card(1);
    assert_eq!(
        command(&mut card, 41, 0x4000_0000, 1),
        [R1_IDLE | R1_ILLEGAL]
    );
}

#[test]
fn the_app_command_prefix_covers_one_command_only() {
    let mut card = card(1);
    command(&mut card, 55, 0, 1);
    command(&mut card, 0, 0, 1);
    assert_eq!(
        command(&mut card, 41, 0x4000_0000, 1),
        [R1_IDLE | R1_ILLEGAL]
    );
}

#[test]
fn cmd58_says_the_card_is_block_addressed() {
    let mut card = card(1);
    init(&mut card);
    let ocr = command(&mut card, 58, 0, 5);
    assert_eq!(ocr[0], 0);
    let ocr = u32::from_be_bytes(ocr[1..].try_into().unwrap());
    assert_eq!(ocr & 1 << 31, 1 << 31, "powered up");
    assert_eq!(ocr & 1 << 30, 1 << 30, "high capacity");
}

#[test]
fn cmd0_puts_a_ready_card_back_to_idle() {
    let mut card = card(1);
    init(&mut card);
    assert_eq!(command(&mut card, 0, 0, 1), [R1_IDLE]);
    assert_eq!(command(&mut card, 17, 0, 1), [R1_IDLE | R1_ILLEGAL]);
}

#[test]
fn data_commands_need_an_initialised_card() {
    let mut card = card(1);
    for index in [9, 10, 12, 17, 18, 24, 25] {
        assert_eq!(
            command(&mut card, index, 0, 1),
            [R1_IDLE | R1_ILLEGAL],
            "CMD{index}"
        );
    }
    command(&mut card, 55, 0, 1);
    assert_eq!(command(&mut card, 23, 1, 1), [R1_IDLE | R1_ILLEGAL]);
}

#[test]
fn an_unknown_command_is_illegal() {
    let mut card = card(1);
    init(&mut card);
    assert_eq!(command(&mut card, 13, 0, 1), [R1_ILLEGAL]);
    assert_eq!(command(&mut card, 16, 512, 1), [R1_ILLEGAL]);
    // CMD12 with no multiple block read to end, and ACMD23 without CMD55 in front.
    assert_eq!(command(&mut card, 12, 0, 1), [R1_ILLEGAL]);
    assert_eq!(command(&mut card, 23, 1, 1), [R1_ILLEGAL]);
}

#[test]
fn cmd17_reads_a_block() {
    let mut card = card(4);
    init(&mut card);
    assert_eq!(read_block(&mut card, 0), vec![1; BLOCK]);
    assert_eq!(read_block(&mut card, 3), vec![4; BLOCK]);
    assert_eq!(read_block(&mut card, 1), vec![2; BLOCK]);
}

#[test]
fn cmd17_past_the_end_is_a_parameter_error_with_no_data() {
    let mut card = card(4);
    init(&mut card);
    assert_eq!(command(&mut card, 17, 4, 1), [R1_PARAMETER]);
    for _ in 0..BLOCK + 8 {
        assert_eq!(exchange(&mut card, 0xFF), 0xFF);
    }
    assert_eq!(command(&mut card, 17, u32::MAX, 1), [R1_PARAMETER]);
}

#[test]
fn cmd24_writes_a_block_and_the_others_are_left_alone() {
    let mut card = card(4);
    init(&mut card);
    assert_eq!(write_block(&mut card, 2, 0xA5) & 0x1F, 0x05);
    assert_eq!(read_block(&mut card, 2), vec![0xA5; BLOCK]);
    let image = card.into_image();
    assert_eq!(image.len(), 4 * BLOCK);
    assert!(
        image[..2 * BLOCK]
            .chunks(BLOCK)
            .enumerate()
            .all(|(b, data)| data == [b as u8 + 1; BLOCK])
    );
    assert_eq!(image[2 * BLOCK..3 * BLOCK], [0xA5; BLOCK]);
    assert_eq!(image[3 * BLOCK..], [4; BLOCK]);
}

#[test]
fn a_write_is_busy_for_a_byte_and_then_ready() {
    let mut card = card(1);
    init(&mut card);
    assert_eq!(command(&mut card, 24, 0, 1), [0]);
    exchange(&mut card, 0xFF);
    exchange(&mut card, 0xFE);
    for _ in 0..BLOCK + 2 {
        exchange(&mut card, 0x11);
    }
    assert_eq!(exchange(&mut card, 0xFF) & 0x1F, 0x05);
    assert_eq!(exchange(&mut card, 0xFF), 0x00);
    assert_eq!(exchange(&mut card, 0xFF), 0xFF);
}

#[test]
fn a_write_waits_for_its_token() {
    let mut card = card(1);
    init(&mut card);
    command(&mut card, 24, 0, 1);
    for _ in 0..20 {
        exchange(&mut card, 0xFF);
    }
    exchange(&mut card, 0xFE);
    for _ in 0..BLOCK + 2 {
        exchange(&mut card, 0x22);
    }
    assert_eq!(read_block(&mut card, 0), vec![0x22; BLOCK]);
}

#[test]
fn cmd24_past_the_end_is_a_parameter_error_and_takes_no_data() {
    let mut card = card(2);
    init(&mut card);
    assert_eq!(command(&mut card, 24, 2, 1), [R1_PARAMETER]);
    exchange(&mut card, 0xFE);
    for _ in 0..BLOCK + 2 {
        exchange(&mut card, 0x33);
    }
    assert_eq!(
        card.into_image(),
        [1; BLOCK]
            .into_iter()
            .chain([2; BLOCK])
            .collect::<Vec<u8>>()
    );
}

#[test]
fn the_card_never_refuses_a_write_for_a_write_protect_switch() {
    // The switch is on the socket and the firmware reads it; the card has no say.
    let mut card = card(1);
    init(&mut card);
    assert_eq!(write_block(&mut card, 0, 0x5A) & 0x1F, 0x05);
}

#[test]
fn deselecting_drops_a_command_half_way() {
    let mut card = card(1);
    init(&mut card);
    for byte in [0x51, 0, 0] {
        exchange(&mut card, byte);
    }
    card.deselect();
    // A fresh frame is taken from its first byte, not as the end of the old one.
    assert_eq!(command(&mut card, 17, 0, 1), [0]);
}

#[test]
fn deselecting_drops_the_reply_in_progress() {
    let mut card = card(1);
    init(&mut card);
    command(&mut card, 17, 0, 1);
    card.deselect();
    assert_eq!(card.miso(), 0xFF);
    for _ in 0..BLOCK + 4 {
        assert_eq!(exchange(&mut card, 0xFF), 0xFF);
    }
}

#[test]
fn deselecting_drops_a_write_before_it_lands() {
    let mut card = card(1);
    init(&mut card);
    command(&mut card, 24, 0, 1);
    exchange(&mut card, 0xFE);
    for _ in 0..100 {
        exchange(&mut card, 0x77);
    }
    card.deselect();
    for _ in 0..BLOCK {
        exchange(&mut card, 0x77);
    }
    assert_eq!(card.into_image(), vec![1; BLOCK]);
}

#[test]
fn deselecting_ends_a_multiple_block_read() {
    let mut card = card(3);
    init(&mut card);
    command(&mut card, 18, 0, 1);
    card.deselect();
    assert_eq!(shift_out(&mut card, BLOCK * 2), vec![0xFF; BLOCK * 2]);
}

#[test]
fn deselecting_keeps_the_card_initialised() {
    let mut card = card(1);
    init(&mut card);
    card.deselect();
    assert_eq!(read_block(&mut card, 0), vec![1; BLOCK]);
}

#[test]
fn a_command_cuts_short_the_reply_before_it() {
    let mut card = card(2);
    init(&mut card);
    command(&mut card, 17, 0, 1);
    exchange(&mut card, 0xFF);
    assert_eq!(read_block(&mut card, 1), vec![2; BLOCK]);
}

// Reads `len` bytes the card has queued, whatever they are.
fn shift_out(card: &mut SdCard, len: usize) -> Vec<u8> {
    (0..len).map(|_| exchange(card, 0xFF)).collect()
}

// The data block of a read already under way: the token, 512 bytes and two CRC bytes.
fn next_block(card: &mut SdCard) -> Vec<u8> {
    assert_eq!(exchange(card, 0xFF), 0xFE, "the token");
    let data = shift_out(card, BLOCK);
    assert_eq!(shift_out(card, DATA_CRC), [0xFF; 2]);
    data
}

// Sends the frame of a command and does not read the reply.
fn send_frame(card: &mut SdCard, index: u8, arg: u32) {
    for byte in [0x40 | index]
        .into_iter()
        .chain(arg.to_be_bytes())
        .chain([0x01])
    {
        exchange(card, byte);
    }
}

// One block of a multiple block write: the token, the data and two CRC bytes. Returns the
// data response, and reads the busy byte after it.
fn write_multi_block(card: &mut SdCard, fill: u8) -> u8 {
    exchange(card, 0xFF);
    exchange(card, 0xFC);
    for _ in 0..BLOCK + DATA_CRC {
        exchange(card, fill);
    }
    let response = exchange(card, 0xFF);
    assert_eq!(exchange(card, 0xFF), 0x00, "busy");
    response
}

#[test]
fn cmd58_before_acmd41_has_the_busy_bit_clear() {
    let mut card = card(1);
    let ocr = command(&mut card, 58, 0, 5);
    assert_eq!(ocr[0], R1_IDLE);
    let ocr = u32::from_be_bytes(ocr[1..].try_into().unwrap());
    assert_eq!(ocr & 1 << 31, 0, "not powered up yet");
    assert_eq!(ocr & 1 << 30, 0, "the capacity is not valid yet");
    assert_eq!(ocr & 0x00FF_8000, 0x00FF_8000, "the voltage window");
}

#[test]
fn cmd18_reads_the_blocks_one_after_the_other_until_cmd12() {
    let mut card = card(5);
    init(&mut card);
    assert_eq!(command(&mut card, 18, 1, 1), [0]);
    assert_eq!(next_block(&mut card), vec![2; BLOCK]);
    assert_eq!(next_block(&mut card), vec![3; BLOCK]);
    assert_eq!(next_block(&mut card), vec![4; BLOCK]);
    // Stopped, the card is out of the read: the host gets the idle byte, and a read of one
    // block after it is as ever.
    send_frame(&mut card, 12, 0);
    shift_out(&mut card, 4);
    assert_eq!(read_block(&mut card, 0), vec![1; BLOCK]);
}

#[test]
fn cmd12_is_answered_after_a_stuff_byte_and_then_the_card_is_busy() {
    let mut card = card(5);
    init(&mut card);
    command(&mut card, 18, 0, 1);
    // The host is half way through a block when it sends CMD12. Whatever the card was
    // shifting out, the byte after the frame is the stuff byte, then R1, then busy.
    shift_out(&mut card, 100);
    send_frame(&mut card, 12, 0);
    assert_eq!(shift_out(&mut card, 5), [0xFF, 0x00, 0x00, 0xFF, 0xFF]);
}

#[test]
fn a_multiple_block_read_waits_for_cmd12_to_arrive_and_sends_no_more_after() {
    let mut card = card(5);
    init(&mut card);
    command(&mut card, 18, 0, 1);
    next_block(&mut card);
    // The next token is out, and the frame goes in over the top of the block behind it.
    send_frame(&mut card, 12, 0);
    shift_out(&mut card, 2 + BLOCK + 8);
    assert_eq!(shift_out(&mut card, 16), [0xFF; 16]);
}

#[test]
fn cmd18_past_the_end_is_a_parameter_error_with_no_data() {
    let mut card = card(2);
    init(&mut card);
    assert_eq!(command(&mut card, 18, 2, 1), [R1_PARAMETER]);
    assert_eq!(shift_out(&mut card, BLOCK + 8), vec![0xFF; BLOCK + 8]);
}

#[test]
fn a_multiple_block_read_that_runs_off_the_end_ends_with_the_out_of_range_token() {
    let mut card = card(2);
    init(&mut card);
    command(&mut card, 18, 0, 1);
    next_block(&mut card);
    next_block(&mut card);
    assert_eq!(exchange(&mut card, 0xFF), 0x08);
    assert_eq!(shift_out(&mut card, 20), [0xFF; 20]);
}

#[test]
fn cmd12_ends_a_read_that_has_run_off_the_end() {
    let mut card = card(2);
    init(&mut card);
    command(&mut card, 18, 0, 1);
    next_block(&mut card);
    next_block(&mut card);
    assert_eq!(exchange(&mut card, 0xFF), 0x08);
    send_frame(&mut card, 12, 0);
    assert_eq!(shift_out(&mut card, 4), [0xFF, 0x00, 0x00, 0xFF]);
    // And that was the end of it: CMD12 again is out of place.
    assert_eq!(command(&mut card, 12, 0, 1), [R1_ILLEGAL]);
}

#[test]
fn another_command_ends_a_multiple_block_read() {
    for (index, arg) in [(17, 3), (24, 3), (13, 0)] {
        let mut card = card(5);
        init(&mut card);
        command(&mut card, 18, 0, 1);
        next_block(&mut card);
        assert_eq!(
            command(&mut card, index, arg, 1)[0] & !R1_ILLEGAL,
            0,
            "CMD{index}"
        );
        // Nothing streams on after it, and CMD12 has nothing to stop. (CMD24 is waiting for
        // its data token, so it is left there.)
        if index == 17 {
            assert_eq!(next_block(&mut card), vec![4; BLOCK]);
        }
        if index != 24 {
            shift_out(&mut card, 2 * BLOCK);
            assert_eq!(command(&mut card, 12, 0, 1), [R1_ILLEGAL], "CMD{index}");
        }
    }
}

#[test]
fn cmd25_writes_several_blocks_and_stops_at_the_stop_token() {
    let mut card = card(6);
    init(&mut card);
    assert_eq!(command(&mut card, 55, 0, 1), [0]);
    assert_eq!(command(&mut card, 23, 3, 1), [0]);
    assert_eq!(command(&mut card, 25, 1, 1), [0]);
    for fill in [0xA1, 0xA2, 0xA3] {
        assert_eq!(write_multi_block(&mut card, fill) & 0x1F, 0x05);
    }
    // The stop token, and busy for a byte after it.
    exchange(&mut card, 0xFF);
    exchange(&mut card, 0xFD);
    assert_eq!(exchange(&mut card, 0xFF), 0x00);
    assert_eq!(exchange(&mut card, 0xFF), 0xFF);
    // Out of the write: a stray block token is nothing, and a read is as ever.
    assert_eq!(read_block(&mut card, 0), vec![1; BLOCK]);
    assert_eq!(read_block(&mut card, 3), vec![0xA3; BLOCK]);
    let image = card.into_image();
    assert_eq!(image[..BLOCK], [1; BLOCK]);
    assert_eq!(image[BLOCK..2 * BLOCK], [0xA1; BLOCK]);
    assert_eq!(image[2 * BLOCK..3 * BLOCK], [0xA2; BLOCK]);
    assert_eq!(image[3 * BLOCK..4 * BLOCK], [0xA3; BLOCK]);
    assert_eq!(image[4 * BLOCK..5 * BLOCK], [5; BLOCK]);
    assert_eq!(image[5 * BLOCK..], [6; BLOCK]);
}

#[test]
fn a_multiple_block_write_does_not_take_the_single_block_token() {
    let mut card = card(2);
    init(&mut card);
    command(&mut card, 25, 0, 1);
    exchange(&mut card, 0xFE);
    for _ in 0..BLOCK + DATA_CRC {
        exchange(&mut card, 0x55);
    }
    assert_eq!(card.into_image(), card_image(2));
}

#[test]
fn a_multiple_block_write_that_runs_off_the_end_takes_the_block_and_refuses_it() {
    let mut card = card(2);
    init(&mut card);
    command(&mut card, 25, 1, 1);
    assert_eq!(write_multi_block(&mut card, 0x77) & 0x1F, 0x05);
    exchange(&mut card, 0xFF);
    exchange(&mut card, 0xFC);
    // The block is a CMD24 for block 0 and then a data token and data, which would write
    // it if any of it were taken for commands.
    let mut block = vec![0x58, 0, 0, 0, 0, 0x01, 0xFF, 0xFE];
    block.resize(BLOCK + DATA_CRC, 0x66);
    for byte in block {
        // Nothing is answered while the block is going in.
        assert_eq!(exchange(&mut card, byte), 0xFF);
    }
    assert_eq!(exchange(&mut card, 0xFF) & 0x1F, 0x0D);
    // The write is over: the card is idle, and a command is taken as one.
    assert_eq!(read_block(&mut card, 0), vec![1; BLOCK]);
    let image = card.into_image();
    assert_eq!(image[BLOCK..], [0x77; BLOCK]);
    assert_eq!(image[..BLOCK], [1; BLOCK]);
}

#[test]
fn cmd25_past_the_end_is_a_parameter_error() {
    let mut card = card(2);
    init(&mut card);
    assert_eq!(command(&mut card, 25, 2, 1), [R1_PARAMETER]);
}

#[test]
fn acmd23_is_accepted_and_changes_nothing() {
    let mut card = card(2);
    init(&mut card);
    command(&mut card, 55, 0, 1);
    assert_eq!(command(&mut card, 23, 1000, 1), [0]);
    assert!(!card.dirty());
    assert_eq!(read_block(&mut card, 1), vec![2; BLOCK]);
}

// The data block of CMD9 or CMD10: R1, the token, 16 bytes and two CRC bytes.
fn register_block(card: &mut SdCard, index: u8) -> [u8; 16] {
    assert!(command(card, index, 0, 1)[0] <= R1_IDLE);
    assert_eq!(exchange(card, 0xFF), 0xFE);
    let bytes = shift_out(card, 16);
    assert_eq!(shift_out(card, DATA_CRC), [0xFF; 2]);
    bytes.try_into().unwrap()
}

#[test]
fn cmd9_gives_a_version_2_csd_sized_by_the_image() {
    // 64 MB is 128 units of 512 KB, so C_SIZE is 127.
    let mut card = SdCard::new(vec![0; 64 << 20]).unwrap();
    init(&mut card);
    let csd = register_block(&mut card, 9);
    assert_eq!(csd[0] >> 6, 1, "CSD_STRUCTURE 1 is version 2.0");
    assert_eq!(csd[3], 0x32, "TRAN_SPEED 25 MHz");
    assert_eq!(csd[5] & 0x0F, 9, "READ_BL_LEN 512");
    let c_size = u32::from(csd[7] & 0x3F) << 16 | u32::from(csd[8]) << 8 | u32::from(csd[9]);
    assert_eq!(c_size, 127);
    assert_eq!((c_size + 1) * 512 * 1024, 64 << 20);
    assert_eq!(csd[15] & 1, 1, "the end bit");
}

#[test]
fn the_csd_capacity_is_the_image_rounded_down_to_whole_units() {
    // Three units and 7 blocks over.
    let mut card = SdCard::new(vec![0; 3 * 512 * 1024 + 7 * BLOCK]).unwrap();
    init(&mut card);
    let csd = register_block(&mut card, 9);
    assert_eq!(csd[7..10], [0, 0, 2]);
}

#[test]
fn the_csd_of_a_card_under_512_kb_says_one_unit() {
    let mut card = card(1);
    init(&mut card);
    let csd = register_block(&mut card, 9);
    assert_eq!(csd[7..10], [0, 0, 0]);
}

#[test]
fn the_csd_and_cid_are_illegal_in_the_idle_state() {
    // Only CMD0, CMD1, CMD8, ACMD41, CMD58 and CMD59 are valid there.
    let mut card = card(1);
    assert_eq!(command(&mut card, 9, 0, 1), [R1_IDLE | R1_ILLEGAL]);
    assert_eq!(command(&mut card, 10, 0, 1), [R1_IDLE | R1_ILLEGAL]);
    assert_eq!(shift_out(&mut card, 20), [0xFF; 20]);
}

#[test]
fn the_csd_is_not_marked_as_a_copy() {
    let mut card = card(1);
    init(&mut card);
    assert_eq!(register_block(&mut card, 9)[14], 0x00);
}

#[test]
fn cmd10_gives_the_made_up_cid() {
    let mut card = card(1);
    init(&mut card);
    let cid = register_block(&mut card, 10);
    assert_eq!(cid[0], 0x00, "manufacturer");
    assert_eq!(&cid[1..3], b"CM");
    assert_eq!(&cid[3..8], b"CMM2E");
    assert_eq!(cid[12], 1, "serial number");
    assert_eq!(cid[15] & 1, 1, "the end bit");
    // It is the same every time.
    assert_eq!(register_block(&mut card, 10), cid);
}

#[test]
fn the_registers_end_in_a_right_crc7() {
    let mut card = card(1);
    init(&mut card);
    for index in [9, 10] {
        let bytes = register_block(&mut card, index);
        assert_eq!(bytes[15] >> 1, crc7(&bytes[..15]), "CMD{index}");
    }
}

#[test]
fn crc7_matches_the_commands_everyone_knows() {
    // CMD0 is sent with CRC byte 0x95 and CMD8 with 0x87: the CRC7 and the end bit.
    assert_eq!(crc7(&[0x40, 0, 0, 0, 0]) << 1 | 1, 0x95);
    assert_eq!(crc7(&[0x48, 0, 0, 0x01, 0xAA]) << 1 | 1, 0x87);
}

#[test]
fn a_new_card_is_not_dirty_and_reads_do_not_make_it_so() {
    let mut card = card(2);
    assert!(!card.dirty());
    init(&mut card);
    read_block(&mut card, 0);
    assert!(!card.dirty());
}

#[test]
fn a_write_that_changes_a_block_makes_the_card_dirty() {
    let mut card = card(2);
    init(&mut card);
    write_block(&mut card, 1, 0x99);
    assert!(card.dirty());
}

#[test]
fn a_write_of_what_the_block_holds_leaves_the_card_clean() {
    let mut card = card(2);
    init(&mut card);
    write_block(&mut card, 1, 2);
    assert!(!card.dirty());
}

#[test]
fn a_multiple_block_write_makes_the_card_dirty() {
    let mut card = card(3);
    init(&mut card);
    command(&mut card, 25, 0, 1);
    write_multi_block(&mut card, 0x44);
    assert!(card.dirty());
}
