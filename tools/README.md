# Board tools

## cmm2_regs.py

Reads 32-bit registers from a real Colour Maximite 2 over its USB serial
console, so real hardware values can be captured again. Python 3 standard
library only.

Give it `NAME ADDRESS` lines on stdin (address in hex, `0x` optional; blank
lines and `#` comments are ignored):

    $ cat regs.txt
    RCC_CR       58024400
    GPIOA_MODER  58020000
    $ python3 tools/cmm2_regs.py < regs.txt
    RCC_CR       0x58024400 = 0x3F03F025
    GPIOA_MODER  0x58020000 = 0xEAABFFFF

Options: `--port` (default `/dev/ttyUSB0`), `--baud` (default 115200),
`--timeout` (seconds to wait for each read, default 10).

A read BASIC rejects, such as the Cortex-M private peripheral bus at
`0xE000E100`, prints `ERROR Address` (or whatever word the board gave) for
that line. Exit status is 0 if every read gave a value, 1 if any line was an
error, 2 for bad input, port or sync problems. Input is checked before the
port is opened.

The tool only sends `PRINT HEX$(PEEK(WORD &H...))`. It never writes to memory.

### Serial permission

The console is a CH340 that shows up as `/dev/ttyUSB0`, owned by group
`dialout`. Add yourself, then log out and in again:

    sudo usermod -aG dialout $USER

### The board must be idle

Leave the board at the `> ` prompt. If a program is running or the full-screen
editor is open, it takes the serial bytes instead and the tool stops with
"no '> ' prompt". Do not type on the board's own keyboard while the script
runs, the input gets mixed with the script's commands.

### Tests

    python3 -m unittest discover -s tools -v

The tests use a fake board on a pseudo-terminal and never open a real port.
