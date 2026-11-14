//! MMBasic's floating point on the core's FPU, at the prompt.

mod common;

use cmm2_core::{Cpu, Irqs, cmm2_memory};
use common::*;

// MMBasic's floating point runs on the core's FPU. A plain FOR loop used to stop at the first
// VRINTM, which the core did not execute, and the rest of its maths library uses VRINTA, VRINTP
// and VFNMA. The answers are the ones Python gives, to the ten digits MMBasic prints.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_does_floating_point_at_the_prompt() {
    let firmware = firmware();
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let console = memory.console();
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();
    boot_to_second_prompt(&mut cpu, &console);

    let cases = [
        ("FOR i=1 TO 3:PRINT i:NEXT", " 1\r\n 2\r\n 3"),
        ("PRINT 1/3", " 0.3333333333"),
        ("PRINT SQR(2)", " 1.414213562"),
        ("PRINT SIN(1)", " 0.8414709848"),
        ("PRINT INT(2.7)", " 2"),
        ("PRINT 2^0.5", " 1.414213562"),
        ("PRINT 10/4", " 2.5"),
        ("PRINT 1E10", " 1e+10"),
        ("DIM a(10):FOR i=0 TO 10:a(i)=i*1.5:NEXT:PRINT a(10)", " 15"),
        (
            "PRINT COS(1),TAN(1),ATN(1),LOG(10),EXP(1)",
            " 0.5403023059\t 1.557407725\t 0.7853981634\t 2.302585093\t 2.718281828",
        ),
        ("PRINT STR$(1/3)", "0.3333333333"),
        ("PRINT VAL(\"3.14159\")", " 3.14159"),
        (
            "PRINT INT(-2.5),FIX(-2.5),CINT(2.5),CINT(3.5),CINT(-2.5)",
            "-3\t-2\t 3\t 4\t-3",
        ),
        // These need the whole of a double rounded down: 1E15+0.5 has 51 significant bits.
        (
            "PRINT INT(1E15+0.5),INT(-1E15-0.5),FIX(123456789012.9)",
            " 1000000000000000\t-1000000000000001\t 123456789012",
        ),
        (
            "x=0:FOR i=1 TO 200:x=x+SQR(i)*SIN(i)/(1+LOG(i+1)):NEXT:PRINT x",
            "-1.494066728",
        ),
    ];
    for (line, expected) in cases {
        assert_eq!(answer_to(&mut cpu, &console, line), expected, "{line}");
    }
}

// The FPU used to do its arithmetic in a decimal type with a narrow exponent: 1E300*10 never
// came back and 1E-130/10 printed 0. These are what the board prints (7.00.00b5, at the prompt),
// subnormals kept and an overflow giving INF.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_does_floating_point_over_the_whole_range_of_a_double() {
    let firmware = firmware();
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let console = memory.console();
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();
    boot_to_second_prompt(&mut cpu, &console);
    // The first error after the boot prints no message, so one goes before the one asserted.
    answer_to(&mut cpu, &console, "PRINT INT(1E300)");

    let cases = [
        ("PRINT 1E300*10", " 1e+301"),
        ("PRINT -1E300*10", "-1e+301"),
        ("PRINT 1E-120/10", " 1e-121"),
        ("PRINT 1E-300/1E10", " 1e-310"),
        ("PRINT 1E-320", " 1.001980198e-320"),
        ("PRINT 1E308*10/10", " INF"),
        ("PRINT 0.1+0.2", " 0.3"),
        ("PRINT 1/3*3", " 1"),
        ("PRINT 1E15+0.3", " 1e+15"),
        ("PRINT 2^0.5*2^0.5", " 2"),
        ("PRINT SQR(1E300)", " 1e+150"),
        ("PRINT EXP(800)", " INF"),
        ("PRINT EXP(-800)", " 0"),
        ("PRINT LOG(1E-300)", "-690.7755279"),
        // An error is the console's cursor and colour codes, then its message.
        (
            "PRINT INT(1E300)",
            "\u{1b}[?25h\u{1b}[37m\u{1b}[mNumber too large\r\n",
        ),
        ("PRINT STR$(1E300*1E10)", "INF"),
    ];
    for (line, expected) in cases {
        assert_eq!(answer_to(&mut cpu, &console, line), expected, "{line}");
    }
}
