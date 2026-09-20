fn main() {
    cc::Build::new()
        .std("c11")
        .include("src")
        .files(["src/parser.c", "src/scanner.c"])
        .compile("tree-sitter-rill");
    for path in ["src/parser.c", "src/scanner.c", "src/tree_sitter/parser.h"] {
        println!("cargo:rerun-if-changed={path}");
    }
}
