import io
import os
import re
import select
import threading
import unittest
from unittest import mock

import cmm2_regs

ERROR_REPLY = "\x1b[?25h\x1b[37m\x1b[mAddress\r\n\r\n> "
COMMAND = "PRINT HEX$(PEEK(WORD &H58024400))"


class ParseLineTest(unittest.TestCase):
    def test_with_and_without_prefix(self):
        self.assertEqual(cmm2_regs.parse_line("RCC_CR 58024400"), ("RCC_CR", 0x58024400))
        self.assertEqual(cmm2_regs.parse_line("RCC_CR 0x58024400"), ("RCC_CR", 0x58024400))
        self.assertEqual(cmm2_regs.parse_line("  A   0Xff  "), ("A", 0xFF))

    def test_blank_and_comment(self):
        self.assertIsNone(cmm2_regs.parse_line(""))
        self.assertIsNone(cmm2_regs.parse_line("   "))
        self.assertIsNone(cmm2_regs.parse_line("# RCC_CR 58024400"))

    def test_malformed(self):
        for line in ["RCC_CR", "RCC_CR 58024400 extra", "RCC_CR xyz", "RCC_CR 0x"]:
            with self.subTest(line=line):
                with self.assertRaises(ValueError):
                    cmm2_regs.parse_line(line)

    def test_address_too_large(self):
        with self.assertRaises(ValueError):
            cmm2_regs.parse_line("X 100000000")


class PeekCommandTest(unittest.TestCase):
    def test_command(self):
        self.assertEqual(cmm2_regs.peek_command(0x58024400), COMMAND)
        self.assertEqual(
            cmm2_regs.peek_command(0xE000E100), "PRINT HEX$(PEEK(WORD &HE000E100))"
        )

    def test_rejects_bad_address(self):
        for address in (-1, 1 << 32):
            with self.assertRaises(ValueError):
                cmm2_regs.peek_command(address)


class ParseReplyTest(unittest.TestCase):
    def test_value(self):
        raw = COMMAND + "\r\n3F03F025\r\n> "
        self.assertEqual(cmm2_regs.parse_reply(raw, COMMAND), (0x3F03F025, None))

    def test_error_with_escape_codes(self):
        raw = COMMAND + "\r\n" + ERROR_REPLY
        self.assertEqual(cmm2_regs.parse_reply(raw, COMMAND), (None, "Address"))
        self.assertEqual(cmm2_regs.parse_reply(ERROR_REPLY, COMMAND), (None, "Address"))

    def test_blank_lines_and_lowercase(self):
        raw = COMMAND + "\r\n\r\neaabffff\r\n\r\n> "
        self.assertEqual(cmm2_regs.parse_reply(raw, COMMAND), (0xEAABFFFF, None))

    def test_empty_reply(self):
        value, error = cmm2_regs.parse_reply(COMMAND + "\r\n> ", COMMAND)
        self.assertIsNone(value)
        self.assertTrue(error)


class FakeBoard:
    """Plays the board on a pty: echoes lines, answers peeks, prints prompts."""

    def __init__(self, values, errors=(), slow=(), prompt=True):
        self.values = values
        self.errors = errors
        self.slow = slow
        self.prompt = prompt
        self.stopped = threading.Event()
        self.master, self.slave = os.openpty()
        self.path = os.ttyname(self.slave)
        self.thread = threading.Thread(target=self.run)
        self.thread.start()

    def close(self):
        self.stopped.set()
        self.thread.join()
        os.close(self.master)
        os.close(self.slave)

    def send(self, text):
        os.write(self.master, text.encode())

    def run(self):
        buf = b""
        while not self.stopped.is_set():
            if not select.select([self.master], [], [], 0.05)[0]:
                continue
            try:
                buf += os.read(self.master, 1024)
            except OSError:
                return
            while b"\r" in buf:
                line, _, buf = buf.partition(b"\r")
                self.answer(line.decode())

    def answer(self, line):
        if not self.prompt:
            return
        if not line:
            self.send("\r\n> ")
            return
        self.send(line + "\r\n")
        address = int(re.search(r"&H([0-9A-F]+)", line).group(1), 16)
        if address in self.slow:
            self.stopped.wait(1.5)
        if address in self.errors:
            self.send(ERROR_REPLY)
        else:
            self.send(f"{self.values[address]:X}\r\n> ")


class FakeBoardTestCase(unittest.TestCase):
    def fake(self, *args, **kwargs):
        board = FakeBoard(*args, **kwargs)
        self.addCleanup(board.close)
        return board

    def run_tool(self, board, text):
        out = io.StringIO()
        status = cmm2_regs.main(["--port", board.path], io.StringIO(text), out)
        return status, out.getvalue()


class ToolTest(FakeBoardTestCase):
    def test_reads_in_order(self):
        board = self.fake({0x58024400: 0x3F03F025, 0x58020000: 0xEAABFFFF})
        text = "# clocks\nRCC_CR 58024400\n\nGPIOA_MODER 0x58020000\n"
        status, out = self.run_tool(board, text)
        self.assertEqual(status, 0)
        self.assertEqual(
            out,
            "RCC_CR      0x58024400 = 0x3F03F025\n"
            "GPIOA_MODER 0x58020000 = 0xEAABFFFF\n",
        )

    def test_error_address(self):
        board = self.fake({0x58024400: 1}, errors={0xE000E100})
        status, out = self.run_tool(board, "A 58024400\nNVIC E000E100\nB 58024400\n")
        self.assertEqual(status, 1)
        self.assertEqual(
            out.splitlines(),
            [
                "A    0x58024400 = 0x00000001",
                "NVIC 0xE000E100 = ERROR Address",
                "B    0x58024400 = 0x00000001",
            ],
        )

    def test_slow_reply(self):
        board = self.fake({0x58024400: 0x10}, slow={0x58024400})
        status, out = self.run_tool(board, "A 58024400\n")
        self.assertEqual(status, 0)
        self.assertEqual(out, "A 0x58024400 = 0x00000010\n")

    def test_bad_line_exits_before_opening_port(self):
        with mock.patch.object(cmm2_regs, "Board") as board_class:
            out = io.StringIO()
            status = cmm2_regs.main(["--port", "/nonexistent"], io.StringIO("A 58024400\nbad\n"), out)
        self.assertEqual(status, 2)
        self.assertEqual(out.getvalue(), "")
        board_class.assert_not_called()

    def test_empty_input_opens_nothing(self):
        with mock.patch.object(cmm2_regs, "Board") as board_class:
            status = cmm2_regs.main([], io.StringIO("# nothing\n"), io.StringIO())
        self.assertEqual(status, 0)
        board_class.assert_not_called()


class PeekTest(FakeBoardTestCase):
    def test_no_way_to_send_arbitrary_text(self):
        self.assertFalse(hasattr(cmm2_regs.Board, "command"))

    def test_value_and_error(self):
        board = self.fake({0x58024400: 0x3F03F025}, errors={0xE000E100})
        with cmm2_regs.Board(board.path, 115200) as port:
            port.sync()
            self.assertEqual(port.peek(0x58024400, 5), (0x3F03F025, None))
            self.assertEqual(port.peek(0xE000E100, 5), (None, "Address"))


class SyncTest(FakeBoardTestCase):
    def test_no_prompt(self):
        board = self.fake({}, prompt=False)
        with cmm2_regs.Board(board.path, 115200) as port:
            with self.assertRaisesRegex(cmm2_regs.BoardError, "no '> ' prompt"):
                port.sync(timeout=0.5)

    def test_missing_port(self):
        with self.assertRaises(cmm2_regs.BoardError):
            cmm2_regs.Board("/nonexistent", 115200)


if __name__ == "__main__":
    unittest.main()
