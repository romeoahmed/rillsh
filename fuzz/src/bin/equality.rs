//! Compare shared acyclic Rill graphs with independently expanded Rust trees.
use rill_runtime::{Engine, Progress, value::Value};
use std::fmt::Write;

#[derive(Clone, PartialEq, Eq)]
enum Tree {
    Leaf(u8),
    List(Box<Self>, Box<Self>),
    Record(Box<Self>, Box<Self>),
}

fn graph(source: &mut String, prefix: char, input: &[u8], side: usize) -> Vec<Tree> {
    let mut trees = vec![Tree::Leaf(0)];
    writeln!(source, "let {prefix}0 = 0").unwrap();
    for (index, record) in input.as_chunks::<6>().0.iter().enumerate() {
        let [kind, left, right] = record[side..side + 3] else {
            unreachable!()
        };
        let a = usize::from(left) % trees.len();
        let b = usize::from(right) % trees.len();
        let name = index + 1;
        let (value, tree) = match kind % 3 {
            0 => (format!("{}", left % 7), Tree::Leaf(left % 7)),
            1 => (
                format!("[{prefix}{a}, {prefix}{b}]"),
                Tree::List(Box::new(trees[a].clone()), Box::new(trees[b].clone())),
            ),
            _ => (
                if kind & 128 == 0 {
                    format!("{{x: {prefix}{a}, y: {prefix}{b}}}")
                } else {
                    format!("{{y: {prefix}{b}, x: {prefix}{a}}}")
                },
                Tree::Record(Box::new(trees[a].clone()), Box::new(trees[b].clone())),
            ),
        };
        writeln!(source, "let {prefix}{name} = {value}").unwrap();
        trees.push(tree);
    }
    trees
}

fn main() {
    afl::fuzz!(|input: &[u8]| {
        // At most twelve binary nodes per side; the expanded oracle stays bounded.
        if input.len() < 2 || input.len() > 74 {
            return;
        }
        let mut source = String::new();
        let a = graph(&mut source, 'a', &input[2..], 0);
        let b = graph(&mut source, 'b', &input[2..], 3);
        let left = usize::from(input[0]) % a.len();
        let right = usize::from(input[1]) % b.len();
        let expected = a[left] == b[right];
        write!(
            source,
            "[a{left} == b{right}, b{right} == a{left}, a{left} != b{right}, a{left} == a{left}]"
        )
        .unwrap();
        let module = rill_syntax::parse("equality-fuzz", &source).unwrap();
        let mut engine = Engine::default();
        engine.begin(&module).unwrap();
        loop {
            match engine.step(8).unwrap() {
                Progress::Complete => {
                    engine.inspect(|value| {
                        let Value::List(values) = value else {
                            panic!("expected comparison results")
                        };
                        assert_eq!(values.as_slice().len(), 4);
                        for (value, expected) in values
                            .as_slice()
                            .iter()
                            .zip([expected, expected, !expected, true])
                        {
                            assert!(matches!(value, Value::Bool(equal) if *equal == expected));
                        }
                    });
                    return;
                }
                Progress::Yielded => engine.collect(),
                Progress::Waiting => panic!("pure graph comparison requested a host effect"),
            }
        }
    });
}
