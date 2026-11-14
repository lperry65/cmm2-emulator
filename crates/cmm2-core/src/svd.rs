//! Register names from ST's SVD description, so an address can be shown as `RCC_CR`.

use std::fmt;
use std::sync::OnceLock;

const STM32H743_SVD: &str = include_str!("../data/STM32H743.svd");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub bit_offset: u32,
    pub bit_width: u32,
}

#[derive(Debug)]
struct Register {
    name: String,
    peripheral: String,
    start: u32,
    bytes: u32,
    reset_value: u32,
    fields: Vec<Field>,
}

#[derive(Debug)]
pub struct Svd {
    // Sorted by start address, one register per address.
    registers: Vec<Register>,
}

/// The register an address falls in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lookup<'a> {
    /// Peripheral and register name, such as `RCC_CR`.
    pub name: &'a str,
    pub peripheral: &'a str,
    pub start: u32,
    /// How far the looked-up address is into the register.
    pub offset: u32,
    /// What the register holds after reset, in its low `size` bits. Taken from the register,
    /// else its peripheral, else the device. GPIOB to GPIOK are derived from GPIOA in the
    /// SVD and so share its reset values, which is wrong for those ports on the chip. The
    /// values are not guaranteed to match the reference manual everywhere.
    pub reset_value: u32,
    pub fields: &'a [Field],
}

#[derive(Debug)]
pub enum SvdError {
    Xml(String),
    Missing {
        what: &'static str,
        whereabouts: String,
    },
    BadReference {
        whereabouts: String,
        name: String,
    },
    BadNumber {
        what: &'static str,
        whereabouts: String,
        text: String,
    },
}

impl Svd {
    pub fn parse(xml: &str) -> Result<Svd, SvdError> {
        let doc = roxmltree::Document::parse(xml).map_err(|e| SvdError::Xml(e.to_string()))?;
        let device = doc.root_element();
        let default_bits = match child(device, "size") {
            Some(_) => number(device, "size", "device")?,
            None => 32,
        };
        let default_reset = match child(device, "resetValue") {
            Some(_) => number(device, "resetValue", "device")?,
            None => 0,
        };
        let peripherals: Vec<Node> = child(device, "peripherals")
            .ok_or_else(|| missing("peripherals", "device"))?
            .children()
            .filter(|n| n.has_tag_name("peripheral"))
            .collect();

        let mut registers = Vec::new();
        for peripheral in &peripherals {
            let name = text(*peripheral, "name", "peripheral")?;
            let whereabouts = format!("peripheral {name}");
            let base = number(*peripheral, "baseAddress", &whereabouts)?;
            // A derived peripheral has no registers of its own; it reuses the base's
            // at its own address.
            let source = match peripheral.attribute("derivedFrom") {
                None => *peripheral,
                Some(from) => *peripherals
                    .iter()
                    .find(|p| child_text(**p, "name") == Some(from))
                    .ok_or_else(|| SvdError::BadReference {
                        whereabouts: whereabouts.clone(),
                        name: from.to_string(),
                    })?,
            };
            let Some(list) = child(source, "registers") else {
                continue;
            };
            let source_name = text(source, "name", "peripheral")?;
            let reset = match child(source, "resetValue") {
                Some(_) => number(source, "resetValue", &format!("peripheral {source_name}"))?,
                None => default_reset,
            };
            for node in list.children().filter(|n| n.has_tag_name("register")) {
                registers.push(parse_register(node, name, base, default_bits, reset)?);
            }
        }
        // The sort is stable, so for registers at the same address the first in the
        // file is the one kept.
        registers.sort_by_key(|r| r.start);
        registers.dedup_by_key(|r| r.start);
        Ok(Svd { registers })
    }

    /// The register description embedded in the library.
    pub fn stm32h743() -> &'static Svd {
        static SVD: OnceLock<Svd> = OnceLock::new();
        SVD.get_or_init(|| Svd::parse(STM32H743_SVD).expect("embedded STM32H743.svd is broken"))
    }

    pub fn lookup(&self, addr: u32) -> Option<Lookup<'_>> {
        let after = self.registers.partition_point(|r| r.start <= addr);
        let register = &self.registers[after.checked_sub(1)?];
        let offset = addr - register.start;
        (offset < register.bytes).then(|| Lookup {
            name: &register.name,
            peripheral: &register.peripheral,
            start: register.start,
            offset,
            reset_value: register.reset_value,
            fields: &register.fields,
        })
    }
}

type Node<'a, 'input> = roxmltree::Node<'a, 'input>;

fn child<'a, 'input>(node: Node<'a, 'input>, tag: &str) -> Option<Node<'a, 'input>> {
    node.children().find(|n| n.has_tag_name(tag))
}

fn child_text<'a>(node: Node<'a, '_>, tag: &str) -> Option<&'a str> {
    child(node, tag).and_then(|n| n.text()).map(str::trim)
}

fn missing(what: &'static str, whereabouts: &str) -> SvdError {
    SvdError::Missing {
        what,
        whereabouts: whereabouts.to_string(),
    }
}

fn text<'a>(node: Node<'a, '_>, tag: &'static str, whereabouts: &str) -> Result<&'a str, SvdError> {
    child_text(node, tag).ok_or_else(|| missing(tag, whereabouts))
}

fn number(node: Node, tag: &'static str, whereabouts: &str) -> Result<u32, SvdError> {
    let text = text(node, tag, whereabouts)?;
    let parsed = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => text.parse(),
    };
    parsed.map_err(|_| SvdError::BadNumber {
        what: tag,
        whereabouts: whereabouts.to_string(),
        text: text.to_string(),
    })
}

fn parse_register(
    node: Node,
    peripheral: &str,
    base: u32,
    default_bits: u32,
    default_reset: u32,
) -> Result<Register, SvdError> {
    let name = format!(
        "{peripheral}_{}",
        text(node, "name", &format!("peripheral {peripheral}"))?
    );
    let whereabouts = format!("register {name}");
    let offset = number(node, "addressOffset", &whereabouts)?;
    let start = base
        .checked_add(offset)
        .ok_or_else(|| SvdError::BadNumber {
            what: "addressOffset",
            whereabouts: whereabouts.clone(),
            text: format!("{offset:#X}"),
        })?;
    let bits = match child(node, "size") {
        Some(_) => number(node, "size", &whereabouts)?,
        None => default_bits,
    };

    let reset_value = match child(node, "resetValue") {
        Some(_) => number(node, "resetValue", &whereabouts)?,
        None => default_reset,
    };

    let mut fields = Vec::new();
    if let Some(list) = child(node, "fields") {
        for field in list.children().filter(|n| n.has_tag_name("field")) {
            let field_name = text(field, "name", &whereabouts)?;
            let whereabouts = format!("field {name}.{field_name}");
            fields.push(Field {
                name: field_name.to_string(),
                bit_offset: number(field, "bitOffset", &whereabouts)?,
                bit_width: number(field, "bitWidth", &whereabouts)?,
            });
        }
    }
    Ok(Register {
        name,
        peripheral: peripheral.to_string(),
        start,
        bytes: bits.div_ceil(8),
        reset_value,
        fields,
    })
}

impl fmt::Display for SvdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SvdError::Xml(msg) => write!(f, "invalid XML: {msg}"),
            SvdError::Missing { what, whereabouts } => write!(f, "{whereabouts}: missing {what}"),
            SvdError::BadReference { whereabouts, name } => {
                write!(f, "{whereabouts}: derivedFrom {name} not found")
            }
            SvdError::BadNumber {
                what,
                whereabouts,
                text,
            } => {
                write!(f, "{whereabouts}: bad {what} '{text}'")
            }
        }
    }
}

impl std::error::Error for SvdError {}

#[cfg(test)]
mod tests;
