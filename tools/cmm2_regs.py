#!/usr/bin/env python3
"""Read 32-bit registers from a real Colour Maximite 2 over its USB console.

Reads `NAME ADDRESS` lines from stdin and prints one line per register.
The board must be idle at the MMBasic prompt. Only read-only PEEK commands
are ever sent.
"""

import argparse
import os
import re
import select
import sys
import termios
import time
import tty

BAUDS = {
    9600: termios.B9600,
    19200: termios.B19200,
    38400: termios.B38400,
    57600: termios.B57600,
    115200: termios.B115200,
    230400: termios.B230400,
    460800: termios.B460800,
}
PROMPT = b"> "
QUIET = 0.3  # the prompt only counts once the line has been silent this long
SYNC_TIMEOUT = 4
ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")


class BoardError(Exception):
    pass


def parse_line(line):
    """Return (name, address), or None for a blank or comment line."""
    line = line.strip()
    if not line or line.startswith("#"):
        return None
    fields = line.split()
    if len(fields) != 2:
        raise ValueError("expected NAME ADDRESS")
    name, text = fields
    if not re.fullmatch(r"(0[xX])?[0-9a-fA-F]+", text):
        raise ValueError(f"bad address {text!r}")
    address = int(text, 16)
    if address > 0xFFFFFFFF:
        raise ValueError(f"address {text!r} does not fit in 32 bits")
    return name, address


def peek_command(address):
    """The only command this tool ever sends."""
    if not 0 <= address <= 0xFFFFFFFF:
        raise ValueError(f"address {address:#x} does not fit in 32 bits")
    return f"PRINT HEX$(PEEK(WORD &H{address:X}))"


def parse_reply(raw, command):
    """Return (value, None) for a register value or (None, text) for an error."""
    lines = [line.strip() for line in ANSI.sub("", raw).replace("\r", "").split("\n")]
    if lines and lines[0] == command:
        lines = lines[1:]
    if lines and lines[-1] == ">":
        lines.pop()
    lines = [line for line in lines if line]
    if not lines:
        return None, "no reply"
    if re.fullmatch(r"[0-9A-Fa-f]{1,8}", lines[0]):
        return int(lines[0], 16), None
    return None, " ".join(lines)


class Board:
    def __init__(self, port, baud):
        try:
            self.fd = os.open(port, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
        except OSError as e:
            raise BoardError(f"cannot open {port}: {e.strerror}") from e
        try:
            mode = termios.tcgetattr(self.fd)
            tty.cfmakeraw(mode)
            mode[2] |= termios.CLOCAL | termios.CREAD
            mode[4] = mode[5] = BAUDS[baud]
            termios.tcsetattr(self.fd, termios.TCSAFLUSH, mode)
        except termios.error as e:
            self.close()
            raise BoardError(f"cannot set up {port}: {e}") from e

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    def close(self):
        os.close(self.fd)

    def _read_prompt(self, timeout):
        """Read until a prompt has arrived and the line has gone quiet."""
        last = time.monotonic()
        deadline = last + timeout
        buf = b""
        while True:
            now = time.monotonic()
            if buf.endswith(PROMPT) and now - last >= QUIET:
                return buf.decode("ascii", errors="replace")
            if now >= deadline:
                raise BoardError("timed out waiting for a prompt")
            if select.select([self.fd], [], [], min(deadline - now, QUIET))[0]:
                try:
                    data = os.read(self.fd, 4096)
                except OSError as e:
                    raise BoardError(f"read failed: {e.strerror}") from e
                if not data:
                    raise BoardError("port closed")
                buf += data
                last = time.monotonic()

    def _send(self, text, timeout):
        os.write(self.fd, text.encode("ascii") + b"\r")
        return self._read_prompt(timeout)

    def sync(self, timeout=SYNC_TIMEOUT):
        try:
            self._send("", timeout)
        except BoardError as e:
            raise BoardError("no '> ' prompt: is the board idle at the prompt?") from e

    def peek(self, address, timeout):
        """Read one register: (value, None) or (None, error text)."""
        command = peek_command(address)
        return parse_reply(self._send(command, timeout), command)


def main(argv=None, stdin=None, stdout=None):
    stdin = stdin or sys.stdin
    stdout = stdout or sys.stdout
    parser = argparse.ArgumentParser(description="Read CMM2 registers over USB serial.")
    parser.add_argument("--port", default="/dev/ttyUSB0")
    parser.add_argument("--baud", type=int, default=115200, choices=sorted(BAUDS))
    parser.add_argument("--timeout", type=float, default=10, help="seconds per read")
    args = parser.parse_args(argv)

    registers = []
    for number, line in enumerate(stdin, 1):
        try:
            parsed = parse_line(line)
        except ValueError as e:
            print(f"line {number}: {e}", file=sys.stderr)
            return 2
        if parsed:
            registers.append(parsed)
    if not registers:
        return 0

    width = max(len(name) for name, _ in registers)
    status = 0
    try:
        with Board(args.port, args.baud) as board:
            board.sync()
            for name, address in registers:
                value, error = board.peek(address, args.timeout)
                shown = f"ERROR {error}" if error else f"0x{value:08X}"
                status |= bool(error)
                print(f"{name:<{width}} 0x{address:08X} = {shown}", file=stdout, flush=True)
    except BoardError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2
    return status


if __name__ == "__main__":
    sys.exit(main())
