"""Check codecs against independent Python values and byte-level boundaries."""

import json
import random

from support import ShellCase


class CodecTests(ShellCase):
    def check(self, expression: str) -> None:
        self.execute(
            (self.shell, "-c", f"exit(if (do {{{expression}}}) then 0 else 1)")
        )

    def decode_lines(self, source: str) -> object:
        result = self.execute(
            (
                self.shell,
                "-c",
                f"{source} |> chunks |> lines |> collect |> to_json |> chunks |> write_stdout",
            )
        )
        self.assertEqual(result.stderr, b"")
        return json.loads(result.stdout)

    def test_lines_boundaries_and_failures(self) -> None:
        for text, expected in (
            ("", []),
            ("\n", [""]),
            ("a\n", ["a"]),
            ("a\r\n\nend", ["a", "", "end"]),
            ("\U0001f642\n\u4e2d\r\nlast", ["\U0001f642", "\u4e2d", "last"]),
            ("\x00\n", ["\x00"]),
            ("a\rb\r", ["a\rb\r"]),
            ("\r\r\n", ["\r"]),
        ):
            with self.subTest(text=text):
                self.assertEqual(
                    self.decode_lines(f"bytes({list(text.encode())})"), expected
                )
        for data in (b"\xc0\x80", b"\xf0\x9f", b"a\n\xff", b"\xed\xa0\x80"):
            with self.subTest(data=data):
                result = self.execute(
                    (
                        self.shell,
                        "-c",
                        f"chunks(bytes({list(data)})) |> lines |> collect",
                    ),
                    status=1,
                )
                self.assertIn(b"DecodeError", result.stderr)
        self.check(
            'collect(lines_with({max_line_bytes:0},chunks(encode_utf8("\\n"))))==[""]'
        )
        self.check(
            'collect(lines_with({max_line_bytes:1},chunks(encode_utf8("x\\n"))))==["x"]'
        )

    def test_utf8_across_chunk_boundaries(self) -> None:
        # Cross several common transport sizes, without asserting any chunk layout.
        for size in (4095, 16383, 65533, 65534, 65535, 65536, 65537, 131071):
            with self.subTest(prefix_bytes=size):
                first = "a" * size + "\U0001f642"
                (self.work / "text").write_text(first + "\r\nlast\r", encoding="utf-8")
                self.assertEqual(
                    self.decode_lines('read_text(path("text")) |> encode_utf8'),
                    [first, "last\r"],
                )

    def test_json_strictness_and_roundtrips(self) -> None:
        invalid = (
            ' {"a":1,"a":2}',
            '{"a":1,"\\u0061":2}',
            "[1,]",
            "/*x*/null",
            "NaN",
            "9223372036854775808",
            "-9223372036854775809",
            "18446744073709551616",
            "1e9999",
            '"\\ud800"',
            "true false",
        )
        for text in invalid:
            with self.subTest(text=text):
                self.assertIn(
                    b"DecodeError",
                    self.execute(
                        (self.shell, "-c", f"from_json({json.dumps(text)})"), status=1
                    ).stderr,
                )
        random_source = random.Random(0)
        values = [
            None,
            True,
            False,
            -(2**63),
            2**63 - 1,
            1.0,
            -0.0,
            "\x00\u4e2d\U0001f642",
            [],
            {},
        ]
        for _ in range(30):
            values.append(
                {
                    "a": [
                        random_source.randrange(-10000, 10000),
                        random_source.random(),
                    ],
                    "b": random_source.choice(values[:10]),
                }
            )
        for value in values:
            with self.subTest(value=value):
                document = json.dumps(value, ensure_ascii=True, separators=(",", ":"))
                result = self.execute(
                    (
                        self.shell,
                        "-c",
                        f"from_json({json.dumps(document)}) |> to_json |> chunks |> write_stdout",
                    )
                )
                self.assertEqual(result.stderr, b"")
                # A same-adapter round trip can hide matching encode/decode bugs.
                # Canonical Python JSON also distinguishes Bool, Int, Float and -0.0.
                self.assertEqual(
                    json.dumps(json.loads(result.stdout), sort_keys=True),
                    json.dumps(value, sort_keys=True),
                )
        for value in ("()", 'path("x")', "bytes([])", "fn(x)=>x", "Option.None"):
            self.assertIn(
                b"TypeError",
                self.execute(
                    (self.shell, "-c", f"to_json({{nested:[{value}]}})"), status=1
                ).stderr,
            )

    def test_json_depth_and_wide_siblings(self) -> None:
        for depth in (1, 32, 65, 256):
            with self.subTest(depth=depth):
                document = "[" * (depth - 1) + "0" + "]" * (depth - 1)
                (self.work / "nested.json").write_text(document, encoding="utf-8")
                self.check(f"""
                  let input=read_text(path("nested.json"));
                  let value=from_json_with({{max_depth:{depth}}},input);
                  decode_utf8(to_json_with({{max_depth:{depth}}},value))==input
                """)
                if depth > 1:
                    result = self.execute(
                        (
                            self.shell,
                            "-c",
                            f'from_json_with({{max_depth:{depth - 1}}},read_text(path("nested.json")))',
                        ),
                        status=1,
                    )
                    self.assertIn(b"LimitExceeded", result.stderr)
        document = [{"n": i, "items": [None, str(i)]} for i in range(4096)]
        (self.work / "wide.json").write_text(json.dumps(document), encoding="utf-8")
        self.check("""
          let value=from_json(read_text(path("wide.json")));
          length(value)==4096 and value[0]=={n:0,items:[null,"0"]} and
          value[4095]=={n:4095,items:[null,"4095"]} and
          from_json(to_json(value))==value
        """)
