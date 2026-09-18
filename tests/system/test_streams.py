"""Exercise public stream ownership, codecs, limits, and real process transport."""

import os

from support import ShellCase


class StreamTests(ShellCase):
    def check(self, expression: str) -> None:
        self.execute(
            (
                self.shell,
                "-c",
                f"do {{ if (do {{ {expression} }}) then () else exit(42) }}",
            )
        )

    def test_lazy_callbacks_and_nested_sinks(self) -> None:
        self.check("""
          let source=chunks(encode_utf8("a\\nb\\nc\\n"));
          let output=source |> lines |> map(fn(x)=>do {
            let nested=chunks(encode_utf8(x)) |> collect_bytes;
            decode_utf8(nested) + "!"
          }) |> filter(fn(x)=>x!="b!") |> take(2) |> collect;
          output==["a!","c!"]
        """)

    def test_stream_sequence_contracts(self) -> None:
        self.check("""
          let input=chunks(encode_utf8("3\\n1\\n2\\n")) |> lines |> map(fn(x)=>from_json(x));
          let sorted=sort_by(identity,input);
          sorted==[1,2,3] and sum(chunks(encode_utf8("1\\n2\\n")) |> lines |> map(from_json))==3 and
          fold(fn(a,b)=>a+b,"",chunks(encode_utf8("a\\nb\\n")) |> lines)=="ab"
        """)
        self.check("""
          each(fn(x)=>set_env("LAST",x), chunks(encode_utf8("a\\nb\\n")) |> lines);
          get_env("LAST")==some(encode_utf8("b"))
        """)
        self.check("""
          fn loop(n)=>if n==0 then true else do {
            let item=chunks(encode_utf8("x")) |> collect_bytes;
            if item==encode_utf8("x") then loop(n-1) else false
          };
          loop(2000)
        """)

    def test_ownership_and_escape(self) -> None:
        cases = (
            'let a=chunks(encode_utf8("x")); let b=take(1,a); collect(a)',
            'let a=chunks(encode_utf8("x")); close(a); close(a)',
            'let a=chunks(encode_utf8("x")); collect(map(fn(x)=>collect(a),a))',
        )
        for code in cases:
            with self.subTest(code=code):
                result = self.execute((self.shell, "-c", f"do {{ {code} }}"), status=1)
                self.assertIn(b"StreamConsumed", result.stderr)
        for code in (
            'let escaped={s:chunks(encode_utf8("x"))}',
            'let escaped=do {let s=chunks(encode_utf8("x")); fn()=>s}',
        ):
            self.assertIn(
                b"StreamEscape", self.execute((self.shell, "-c", code), status=1).stderr
            )
        self.assertIn(
            b"UnconsumedStream",
            self.execute((self.shell, "-c", "chunks(bytes([]))"), status=1).stderr,
        )
        self.check("""do { let s=chunks(bytes([])); let f=fn(x)=>x+1; f }(2)==3""")

    def test_attempt_checkpoints_and_statement_cleanup(self) -> None:
        self.check("""
          let keep=chunks(encode_utf8("keep"));
          let moved=chunks(encode_utf8("moved"));
          let result=attempt(fn()=>do {
            let ignored=map(identity,moved);
            raise(error("Example","failed"))
          });
          let invalid=attempt(fn()=>collect(moved));
          match [result,invalid] {
            [Result.Err {error:a},Result.Err {error:b}] =>
              a.kind=="Example" and b.kind=="StreamConsumed" and
              collect_bytes(keep)==encode_utf8("keep"),
            _=>false
          }
        """)
        self.check("""
          let before=length(collect(files(path("/dev/fd"))));
          let outcome=attempt(fn()=>do {
            let abandoned=stream(job { ^./child rows });
            raise(error("Example","failed"))
          });
          let after=length(collect(files(path("/dev/fd"))));
          match outcome {Result.Err {error}=>error.kind=="Example" and before==after,_=>false}
        """)
        self.execute(
            (
                self.shell,
                "-c",
                """
          let before=length(collect(files(path("/dev/fd"))));
          do {let abandoned=stream(job { ^./child rows }); ()};
          let after=length(collect(files(path("/dev/fd"))));
          exit(if before==after then 0 else 42)
        """,
            )
        )

    def test_noninteractive_stop_is_a_checked_failure(self) -> None:
        self.check("""
          match attempt(fn()=>stream(job { ^./child selfstop }) |> collect_bytes) {
            Result.Err {error}=>error.kind=="ProcessError", _=>false
          }
        """)

    def test_module_resource_escape(self) -> None:
        (self.work / "resource.rill").write_text(
            'export let resource=chunks(encode_utf8("x"))', encoding="utf-8"
        )
        result = self.execute(
            (self.shell, "-c", 'import "./resource.rill" as resource'), status=1
        )
        self.assertIn(b"StreamEscape", result.stderr)

    def test_interleaved_chain_release(self) -> None:
        self.check("""
          let a=chunks(encode_utf8("a"));
          let b=chunks(encode_utf8("b"));
          let c=map(identity,a);
          let d=map(identity,b);
          let e=map(identity,c);
          let f=map(identity,d);
          collect_bytes(e)==encode_utf8("a") and
          collect_bytes(f)==encode_utf8("b") and
          collect_bytes(chunks(encode_utf8("c")))==encode_utf8("c")
        """)

    def test_take_does_not_prefetch_callbacks(self) -> None:
        self.check("""
          let result=chunks(encode_utf8("a\\nb\\n")) |> lines |>
            map(fn(x)=>do {set_env("VISITED",x);x}) |> take(1) |> collect;
          result==["a"] and get_env("VISITED")==some(encode_utf8("a"))
        """)

    def test_zero_take_and_limits(self) -> None:
        self.check(
            '(chunks(encode_utf8("x")) |> map(fn(x)=>div(1,0)) |> take(0) |> collect) == []'
        )
        cases = (
            'collect_with({max_items:0},lines(chunks(encode_utf8("x"))))',
            'collect_with({max_bytes:0},chunks(encode_utf8("x")))',
            'collect_bytes_with({max_bytes:0},chunks(encode_utf8("x")))',
            'collect(lines_with({max_line_bytes:1},chunks(encode_utf8("xx"))))',
            "to_json_with({max_bytes:1},[])",
            'from_json_with({max_depth:1},"[1]")',
        )
        for code in cases:
            with self.subTest(code=code):
                self.assertIn(
                    b"LimitExceeded",
                    self.execute((self.shell, "-c", code), status=1).stderr,
                )
        for options in (
            "{typo:1}",
            "{max_bytes:-1}",
            "{max_bytes:1.0}",
            "{max_depth:0}",
        ):
            self.assertIn(
                b"TypeError",
                self.execute(
                    (self.shell, "-c", f"to_json_with({options},[])"), status=1
                ).stderr,
            )

    def test_filesystem_and_pipeline(self) -> None:
        (self.work / "empty").mkdir()
        (self.work / "a").write_text("hello\n", encoding="utf-8")
        (self.work / ".hidden").write_bytes(b"")
        (self.work / "link").symlink_to("a")
        self.check("""do {
          let entries=files(path(".")) |> collect;
          let selected=filter(fn(e)=>e.name==path("a"),entries);
          let value=map(fn(e)=>{name:display_path(e.name),size:e.size},selected);
          let result=to_json(value) |> chunks |> through(job { ^./child echo }) |> collect_bytes |> from_json;
          length(filter(fn(e)=>e.kind=="symlink",entries))>=1 and result==[{name:"a",size:6}] and
          collect(files(path("empty")))==[] and read_text(path("a"))=="hello\\n" and glob("missing*")==[]
        }""")
        (self.work / "invalid").write_bytes(b"\xff")
        self.assertIn(
            b"DecodeError",
            self.execute(
                (self.shell, "-c", 'read_text(path("invalid"))'), status=1
            ).stderr,
        )
        os.mkfifo(self.work / "fifo")
        self.assertIn(
            b"IOError",
            self.execute(
                (self.shell, "-c", 'read_text(path("fifo"))'), status=1
            ).stderr,
        )

    def test_capture_and_checked_completion(self) -> None:
        self.check("""do { let result=capture(job { ^./child both });
          result.stdout==encode_utf8("out\\n") and result.stderr==encode_utf8("err\\n") and check(result.report)==()
        }""")
        self.check("""match attempt(fn()=>check(capture(job { ^./child exit 7 }).report)) {
          Result.Err {error} => error.kind=="ProcessError", _ => false
        }""")
        self.assertIn(
            b"ProcessError",
            self.execute(
                (self.shell, "-c", "stream(job { ^./child exit 7 }) |> collect_bytes"),
                status=7,
            ).stderr,
        )
        self.assertIn(
            b"LimitExceeded",
            self.execute(
                (self.shell, "-c", "capture_with({max_bytes:0},job { ^./child both })"),
                status=1,
            ).stderr,
        )
        self.assertIn(
            b"TypeError",
            self.execute(
                (
                    self.shell,
                    "-c",
                    'chunks(encode_utf8("x")) |> through(job { ^./child echo > output }) |> collect_bytes',
                ),
                status=1,
            ).stderr,
        )
        self.assertFalse((self.work / "output").exists())

    def test_eof_is_not_process_success(self) -> None:
        result = self.execute(
            (
                self.shell,
                "-c",
                "stream(job { ^./child output-error }) |> collect_bytes",
            ),
            status=7,
        )
        self.assertIn(b"ProcessError", result.stderr)
        result = self.execute(
            (self.shell, "-c", 'to_json({nested:[path("x")]})'), status=1
        )
        self.assertIn(b'$["nested"][0]', result.stderr)

    def test_duplex_backpressure_and_cutoff(self) -> None:
        data = bytes(range(256)) * 8192
        (self.work / "input").write_bytes(data)
        result = self.execute(
            (
                self.shell,
                "-c",
                "stream(job { ^./child echo < input }) |> through(job { ^./child flood }) |> collect_bytes |> chunks |> write_stdout",
            )
        )
        self.assertEqual(result.stdout, data)
        self.assertEqual(result.stderr, data)
        self.check(
            "length(stream(job { ^./child rows }) |> lines |> take(100) |> collect)==100"
        )

    def test_callback_error_cleanup_and_effect_order(self) -> None:
        self.check("""do {
          let result=attempt(fn()=>stream(job { ^./child rows }) |> lines |> map(fn(x)=>div(1,0)) |> collect);
          let good=stream(job { ^./child write "a\\nb\\n" }) |> lines |> map(fn(x)=>do {set_env("VISITED",x);x}) |> collect;
          match result { Result.Err {error} => error.kind=="ArithmeticError" and good==["a","b"] and get_env("VISITED")==some(encode_utf8("b")), _=>false }
        }""")
