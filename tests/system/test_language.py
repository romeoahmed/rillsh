"""Verify language effects, modules, and diagnostics through the executable."""

import os
import subprocess
import unittest

from support import ShellCase


class LanguageTests(ShellCase):
    def run_source(self, source: str, status: int = 0) -> bytes:
        result = self.execute((self.shell, "-c", source), status=status)
        if status == 0:
            self.assertEqual(result.stderr, b"")
        return result.stdout

    def reject(
        self, source: str, kind: bytes, *, status: int = 1
    ) -> subprocess.CompletedProcess[bytes]:
        result = self.execute((self.shell, "-c", source), status=status)
        self.assertIn(b": " + kind + b": ", result.stderr)
        return result

    def check(self, expression: str) -> None:
        self.run_source(f"exit(if {expression} then 0 else 1)")

    def test_application_and_pipe_effect_order(self) -> None:
        result = self.run_source("""
fn mark(value) => do { ^./child write $value; value }
fn f(first) => do { mark("apply"); fn(second) => second }
f(mark("a"),mark("b"))
mark("left") |> do { mark("right"); fn(x) => x }
""")
        self.assertEqual(result, b"aapplybleftright")

    def test_raise_preserves_error_payload(self) -> None:
        self.check(
            'match attempt(fn()=>raise(error("Example","message"))) { Result.Err {error: Error {kind,message,..}} => kind=="Example" and message=="message", _=>false }'
        )

    def test_match_subject_guards_and_scope(self) -> None:
        result = self.run_source("""
fn subject() => do { ^./child write s; [1,2] }
fn guard(value) => do { ^./child write g; false }
let outer=9
let answer=match subject() {
  [outer,other] if guard(outer) => 0,
  [first,..rest] => first+rest[0]
}
exit(if answer==3 and outer==9 then 0 else 1)
""")
        self.assertEqual(result, b"sg")
        self.reject("match 1 { leaked if false=>0, _=>leaked }", b"TypeError")
        self.reject("match 1 { x if 1+true=>0, _=>0 }", b"TypeError")

    def test_sort_callback_runs_once_in_source_order(self) -> None:
        result = self.run_source("""
fn key(value) => do { ^./child write $(text(value)); value }
exit(if sort_by(key,[3,1,2])==[1,2,3] then 0 else 1)
""")
        self.assertEqual(result, b"312")
        result = self.reject(
            "sort_by_with({max_items:0},fn(x)=>do { ^./child write unexpected; x },[1])",
            b"LimitExceeded",
        )
        self.assertEqual(result.stdout, b"")
        large_key = "k" * 2048
        result = self.reject(
            f'fn key(x)=>do {{ ^./child write $(text(x)); "{large_key}" }}; sort_by_with({{max_bytes:1024}},key,[1,2])',
            b"LimitExceeded",
        )
        self.assertEqual(result.stdout, b"1")

    def test_modules_identity_cache_and_isolation(self) -> None:
        module = self.work / "types.rill"
        module.write_text(
            """
^./child write "init"
export struct Point {x,y}
export fn make(x)=>Point({x:x,y:2})
let private = 7
""",
            encoding="utf-8",
        )
        (self.work / "alias.rill").symlink_to(module)
        os.link(module, self.work / "hardlink.rill")
        result = self.run_source("""
import "./types.rill" as a
import "./alias.rill" as b
import "./hardlink.rill" as c
let value = a.make(3)
exit(match value { c.Point {x,y} => if x==3 and y==2 and b.make(3)==value then 0 else 1, _=>1 })
""")
        self.assertEqual(result, b"init")
        self.reject('import "./types.rill" as a; a.private', b"MissingField")
        (self.work / "other.rill").write_text(
            "export struct Point {x,y}\n", encoding="utf-8"
        )
        self.run_source(
            'import "./types.rill" as a; import "./other.rill" as b; exit(if a.make(1)==b.Point({x:1,y:2}) then 1 else 0)'
        )
        (self.work / "cycle.rill").write_text(
            'import "./cycle.rill" as self\n', encoding="utf-8"
        )
        self.reject('import "./cycle.rill" as cycle', b"TypeError")
        self.run_source(
            'import "std:seq" as seq; exit(if seq.map(add(1),[2])==[3] then 0 else 1)'
        )

    def test_import_base_survives_cd(self) -> None:
        directory = self.work / "module"
        directory.mkdir()
        (directory / "leaf.rill").write_text("export let value=7\n", encoding="utf-8")
        (directory / "main.rill").write_text(
            'cd(path("/")); import "./leaf.rill" as leaf; exit(leaf.value-7)\n',
            encoding="utf-8",
        )
        self.execute((self.shell, directory / "main.rill"))

    def test_module_failures(self) -> None:
        (self.work / "invalid.rill").write_bytes(b"\xff")
        (self.work / "broken.rill").write_text(
            "^./child mark forbidden; let x=", encoding="utf-8"
        )
        cases = (
            ('import "./missing.rill" as m', b"IOError", 1),
            ('import "./invalid.rill" as m', b"SyntaxError", 2),
            ('import "./broken.rill" as m', b"SyntaxError", 2),
            ('import "std:missing" as m', b"IOError", 1),
            ('import "std:prelude" as m', b"IOError", 1),
            ('import "bad\\u{0}path" as m', b"TypeError", 1),
        )
        for source, kind, status in cases:
            with self.subTest(source=source):
                result = self.reject(source, kind, status=status)
                self.assertEqual(result.stdout, b"")
                self.assertFalse((self.work / "forbidden").exists())

    def test_plans_validate_before_launch_and_reuse(self) -> None:
        result = self.run_source(
            'let p=job { ^./child write ...$(["a b"]) }; run(p); run(p)'
        )
        self.assertEqual(result, b"a ba b")
        result = self.run_source(
            'fn argument()=>do { ^./child write evaluated; "captured" }; let p=job { ^./child write $(argument()) }; run(p); run(p)'
        )
        self.assertEqual(result, b"evaluatedcapturedcaptured")
        self.run_source("let plan=job { ^./child write value > unopened }")
        self.assertFalse((self.work / "unopened").exists())
        self.run_source(
            "let p=job { ^./child exit 0 }; let a=start(p); let b=start(p); let first=wait(a); let second=wait(b); exit(if first.id!=second.id then 0 else 1)"
        )
        result = self.run_source('let words=["", "a b"]; ^./child args ...$words')
        self.assertEqual(result, b"0:\n3:a b\n")
        self.reject("job { ^./child write $(1) > untouched }", b"TypeError")
        self.assertFalse((self.work / "untouched").exists())
        invalid = self.reject('command("./child",["ok",bytes([0])])', b"TypeError")
        self.assertIn(b"argument 2", invalid.stderr)
        result = self.run_source(
            'run(pipe(command("./child",["args", "hello"]),command("./child",["echo"])))'
        )
        self.assertEqual(result, b"5:hello\n")

    def test_environment_overrides_and_launch_snapshots(self) -> None:
        result = self.run_source("""
let plan=job { ^./child env RILL_VALUE }
set_env("RILL_VALUE","first")
let first=start(plan)
set_env("RILL_VALUE","second")
check(wait(first))
run(plan)
run(with_env({RILL_VALUE:"override"},plan))
unset_env("RILL_VALUE")
exit(match get_env("RILL_VALUE") { Option.None=>0, _=>1 })
""")
        self.assertEqual(result, b"firstsecondoverride")
        directory = self.work / "dir"
        directory.mkdir()
        self.run_source(
            'let p=with_cwd(path("dir"),job { ^../child args cwd > written }); run(p)'
        )
        self.assertEqual((directory / "written").read_bytes(), b"3:cwd\n")

    def test_reports_and_stage_policies(self) -> None:
        self.run_source(
            "let p=accept_exit([7],job { ^./child exit 7 }); let r=wait(start(p)); check(r); exit(match r.stages[0].termination { Termination.Exited {code}=>code-7, _=>1 })"
        )
        self.reject("run(accept_exit([1],job { ^./child exit 0 }))", b"ProcessError")
        self.run_source(
            "let p=pipe(accept_exit([7],job { ^./child exit 7 }), job { ^./child echo }); run(p)"
        )
        self.run_source(
            "let r=wait(start(job { ^./child exit 9 })); check(r)", status=9
        )

    def test_error_identity_and_presentation(self) -> None:
        self.reject(
            'struct Forged {kind,message,span,notes}; raise(Forged({kind:"x",message:"x",span:null,notes:[]}))',
            b"TypeError",
        )
        result = self.reject(
            'raise(error("Example","line\\n\\u{1b}[31m\\u{0}tail"))', b"Example"
        )
        self.assertNotIn(b"\x1b", result.stderr)
        self.assertIn(b"line\\x0a\\x1b[31m\\x00tail", result.stderr)
        self.assertEqual(result.stderr.count(b"\n"), 1)

    def test_script_arguments(self) -> None:
        script = self.work / "args.rill"
        script.write_text(
            "exit(if args()==[bytes([]),bytes([97]),bytes([98,32,99]),bytes([255])] then 0 else 1)\n",
            encoding="utf-8",
        )
        self.execute((self.shell, script, "", "a", "b c", os.fsdecode(b"\xff")))


if __name__ == "__main__":
    unittest.main()
