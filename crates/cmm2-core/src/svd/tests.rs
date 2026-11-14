use super::*;

// Wraps peripherals in a device header with the default register size of 32 bits.
fn device(peripherals: &str) -> String {
    format!("<device><size>0x20</size><peripherals>{peripherals}</peripherals></device>")
}

fn register(name: &str, offset: &str, fields: &str) -> String {
    format!(
        "<register><name>{name}</name><addressOffset>{offset}</addressOffset>\
         <size>0x20</size><fields>{fields}</fields></register>"
    )
}

fn field(name: &str, offset: u32, width: u32) -> String {
    format!(
        "<field><name>{name}</name><bitOffset>{offset}</bitOffset>\
         <bitWidth>{width}</bitWidth></field>"
    )
}

fn peripheral(name: &str, base: &str, registers: &str) -> String {
    format!(
        "<peripheral><name>{name}</name><baseAddress>{base}</baseAddress>\
         <registers>{registers}</registers></peripheral>"
    )
}

fn parse(peripherals: &str) -> Svd {
    Svd::parse(&device(peripherals)).unwrap()
}

#[test]
fn register_at_base_plus_offset() {
    let svd = parse(&peripheral(
        "UART",
        "0x40000000",
        &register("CR", "0x8", ""),
    ));
    let found = svd.lookup(0x4000_0008).unwrap();
    assert_eq!(found.name, "UART_CR");
    assert_eq!(found.peripheral, "UART");
    assert_eq!(found.start, 0x4000_0008);
    assert_eq!(found.offset, 0);
    assert!(svd.lookup(0x4000_0000).is_none());
}

#[test]
fn fields_keep_file_order() {
    let fields = field("EN", 0, 1) + &field("DIV", 3, 2);
    let svd = parse(&peripheral("P", "0x1000", &register("R", "0", &fields)));
    let found = svd.lookup(0x1000).unwrap();
    let expected = [
        Field {
            name: "EN".into(),
            bit_offset: 0,
            bit_width: 1,
        },
        Field {
            name: "DIV".into(),
            bit_offset: 3,
            bit_width: 2,
        },
    ];
    assert_eq!(found.fields, expected);
}

#[test]
fn derived_peripheral_uses_its_own_base_and_name() {
    let xml = peripheral("A", "0x1000", &register("R", "0x4", &field("F", 0, 1)))
        + "<peripheral derivedFrom=\"A\"><name>B</name><baseAddress>0x2000</baseAddress></peripheral>";
    let svd = parse(&xml);
    let found = svd.lookup(0x2004).unwrap();
    assert_eq!(found.name, "B_R");
    assert_eq!(found.peripheral, "B");
    assert_eq!(found.fields.len(), 1);
    assert_eq!(svd.lookup(0x1004).unwrap().name, "A_R");
}

#[test]
fn derived_peripheral_may_come_before_its_base() {
    let xml =
        "<peripheral derivedFrom=\"A\"><name>B</name><baseAddress>0x2000</baseAddress></peripheral>"
            .to_string() + &peripheral("A", "0x1000", &register("R", "0", ""));
    assert_eq!(parse(&xml).lookup(0x2000).unwrap().name, "B_R");
}

#[test]
fn address_inside_a_register_gives_the_byte_offset() {
    let svd = parse(&peripheral("P", "0x1000", &register("R", "0x10", "")));
    let found = svd.lookup(0x1013).unwrap();
    assert_eq!(found.name, "P_R");
    assert_eq!(found.start, 0x1010);
    assert_eq!(found.offset, 3);
    assert!(svd.lookup(0x1014).is_none());
}

#[test]
fn address_without_a_register_is_none() {
    let regs = register("A", "0", "") + &register("B", "0x8", "");
    let svd = parse(&peripheral("P", "0x1000", &regs));
    assert!(svd.lookup(0x1004).is_none());
    assert!(svd.lookup(0x0FFF).is_none());
    assert!(svd.lookup(0xFFFF_FFFF).is_none());
}

#[test]
fn first_register_wins_at_a_shared_address() {
    let regs = register("FIRST", "0x4", &field("X", 0, 1)) + &register("SECOND", "0x4", "");
    let svd = parse(&peripheral("P", "0x1000", &regs));
    let found = svd.lookup(0x1004).unwrap();
    assert_eq!(found.name, "P_FIRST");
    assert_eq!(found.fields.len(), 1);
}

#[test]
fn register_without_size_takes_the_device_default() {
    let reg = "<register><name>R</name><addressOffset>0</addressOffset></register>";
    let xml = format!(
        "<device><size>0x10</size><peripherals>{}</peripherals></device>",
        peripheral("P", "0x1000", reg)
    );
    let svd = Svd::parse(&xml).unwrap();
    assert!(svd.lookup(0x1001).is_some());
    assert!(svd.lookup(0x1002).is_none());
}

#[test]
fn decimal_numbers_are_accepted() {
    let svd = parse(&peripheral("P", "4096", &register("R", "8", "")));
    assert_eq!(svd.lookup(0x1008).unwrap().name, "P_R");
}

#[test]
fn missing_base_address_is_an_error() {
    let xml = device("<peripheral><name>P</name></peripheral>");
    let err = Svd::parse(&xml).unwrap_err();
    assert_eq!(err.to_string(), "peripheral P: missing baseAddress");
}

#[test]
fn bad_number_is_an_error() {
    let xml = device(&peripheral("P", "0x1000", &register("R", "0xZZ", "")));
    let err = Svd::parse(&xml).unwrap_err();
    assert_eq!(err.to_string(), "register P_R: bad addressOffset '0xZZ'");
}

#[test]
fn bad_field_width_is_an_error() {
    let xml = device(&peripheral(
        "P",
        "0x1000",
        &register(
            "R",
            "0",
            "<field><name>F</name><bitOffset>0</bitOffset><bitWidth>x</bitWidth></field>",
        ),
    ));
    let err = Svd::parse(&xml).unwrap_err();
    assert_eq!(err.to_string(), "field P_R.F: bad bitWidth 'x'");
}

#[test]
fn unknown_derived_from_is_an_error() {
    let xml = device(
        "<peripheral derivedFrom=\"NOPE\"><name>B</name><baseAddress>0</baseAddress></peripheral>",
    );
    let err = Svd::parse(&xml).unwrap_err();
    assert_eq!(err.to_string(), "peripheral B: derivedFrom NOPE not found");
}

#[test]
fn broken_xml_is_an_error() {
    let err = Svd::parse("<device>").unwrap_err();
    assert!(matches!(err, SvdError::Xml(_)));
}

// The tests below use the SVD embedded in the library.

fn names(addr: u32) -> Option<String> {
    Svd::stm32h743().lookup(addr).map(|l| l.name.to_string())
}

#[test]
fn embedded_rcc_cr_has_its_fields() {
    let found = Svd::stm32h743().lookup(0x5802_4400).unwrap();
    assert_eq!(found.name, "RCC_CR");
    assert_eq!(found.peripheral, "RCC");
    let field = |name: &str| found.fields.iter().find(|f| f.name == name).unwrap();
    assert_eq!(
        (field("HSION").bit_offset, field("HSION").bit_width),
        (0, 1)
    );
    assert_eq!(
        (field("HSIDIV").bit_offset, field("HSIDIV").bit_width),
        (3, 2)
    );
    assert_eq!(found.fields[0].name, "HSION");
}

#[test]
fn embedded_fmc_sdtr2() {
    assert_eq!(names(0x5200_414C).as_deref(), Some("FMC_SDTR2"));
    assert_eq!(names(0x5200_4000).as_deref(), Some("FMC_BCR1"));
}

#[test]
fn embedded_derived_gpiob() {
    assert_eq!(names(0x5802_0400).as_deref(), Some("GPIOB_MODER"));
    assert_eq!(names(0x5802_0000).as_deref(), Some("GPIOA_MODER"));
}

#[test]
fn embedded_alternate_registers_first_wins() {
    // ICSCR and HSICFGR share this address; ICSCR comes first in the file.
    assert_eq!(names(0x5802_4404).as_deref(), Some("RCC_ICSCR"));
}

#[test]
fn embedded_gap_in_a_peripheral_is_none() {
    // RCC has nothing between CRRCR (0x8) and CFGR (0x10).
    assert!(Svd::stm32h743().lookup(0x5802_440C).is_none());
    assert!(Svd::stm32h743().lookup(0x1234_5678).is_none());
}

#[test]
fn embedded_svd_parses() {
    let _ = Svd::stm32h743();
}

// Reset values.

fn reg_with_reset(name: &str, offset: &str, reset: Option<&str>) -> String {
    let reset = reset.map_or(String::new(), |r| format!("<resetValue>{r}</resetValue>"));
    format!(
        "<register><name>{name}</name><addressOffset>{offset}</addressOffset>{reset}</register>"
    )
}

fn reset_of(svd: &Svd, addr: u32) -> u32 {
    svd.lookup(addr).unwrap().reset_value
}

#[test]
fn register_reset_value_wins() {
    let xml = "<device><resetValue>0x1</resetValue><peripherals><peripheral><name>P</name>\
               <baseAddress>0x1000</baseAddress><resetValue>0x2</resetValue><registers>"
        .to_string()
        + &reg_with_reset("R", "0", Some("0x00C0"))
        + "</registers></peripheral></peripherals></device>";
    assert_eq!(reset_of(&Svd::parse(&xml).unwrap(), 0x1000), 0xC0);
}

#[test]
fn peripheral_reset_value_is_inherited() {
    let xml = "<device><resetValue>0x1</resetValue><peripherals><peripheral><name>P</name>\
               <baseAddress>0x1000</baseAddress><resetValue>0x2</resetValue><registers>"
        .to_string()
        + &reg_with_reset("R", "0", None)
        + "</registers></peripheral></peripherals></device>";
    assert_eq!(reset_of(&Svd::parse(&xml).unwrap(), 0x1000), 2);
}

#[test]
fn device_reset_value_is_the_default() {
    let xml = "<device><resetValue>0x5</resetValue><peripherals>".to_string()
        + &peripheral("P", "0x1000", &reg_with_reset("R", "0", None))
        + "</peripherals></device>";
    assert_eq!(reset_of(&Svd::parse(&xml).unwrap(), 0x1000), 5);
}

#[test]
fn no_reset_value_anywhere_is_zero() {
    let svd = parse(&peripheral("P", "0x1000", &reg_with_reset("R", "0", None)));
    assert_eq!(reset_of(&svd, 0x1000), 0);
}

#[test]
fn derived_peripheral_takes_the_reset_values_of_its_base() {
    let xml = "<device><peripherals><peripheral><name>A</name><baseAddress>0x1000</baseAddress>\
               <resetValue>0x7</resetValue><registers>"
        .to_string()
        + &reg_with_reset("X", "0", None)
        + &reg_with_reset("Y", "4", Some("0x9"))
        + "</registers></peripheral>\
           <peripheral derivedFrom=\"A\"><name>B</name><baseAddress>0x2000</baseAddress>\
           <resetValue>0x3</resetValue></peripheral></peripherals></device>";
    let svd = Svd::parse(&xml).unwrap();
    assert_eq!(reset_of(&svd, 0x2000), 7);
    assert_eq!(reset_of(&svd, 0x2004), 9);
}

#[test]
fn bad_reset_value_is_an_error() {
    let xml = device(&peripheral(
        "P",
        "0x1000",
        &reg_with_reset("R", "0", Some("0xQ")),
    ));
    let err = Svd::parse(&xml).unwrap_err();
    assert_eq!(err.to_string(), "register P_R: bad resetValue '0xQ'");
}

#[test]
fn embedded_reset_values() {
    let svd = Svd::stm32h743();
    assert_eq!(reset_of(svd, 0x5802_4400), 0x0000_0083); // RCC_CR
    assert_eq!(reset_of(svd, 0x5802_4410), 0); // RCC_CFGR
    assert_eq!(reset_of(svd, 0x5802_4800), 0xF000_C000); // PWR_CR1
    assert_eq!(reset_of(svd, 0x5802_480C), 0x0000_0006); // PWR_CR3
    assert_eq!(reset_of(svd, 0x5802_4818), 0x0000_4000); // PWR_D3CR
    assert_eq!(reset_of(svd, 0x5802_0000), 0xABFF_FFFF); // GPIOA_MODER
    assert_eq!(reset_of(svd, 0x4001_101C), 0x0000_00C0); // USART1_ISR
}
