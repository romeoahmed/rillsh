//! Check bounded producer state, close reasons and collection against an integer model.
use rill_runtime::{Engine, Progress, value::Value};

fn main() {
    afl::fuzz!(|data: &[u8]| {
        let count = i64::from(data.first().copied().unwrap_or(0) % 32);
        let demand = i64::from(data.get(1).copied().unwrap_or(0) % 32);
        let consumed = count.min(demand);
        let expected: i64 = (0..consumed).map(|n| n * n).sum();
        let reason = if demand <= count {
            "Cutoff"
        } else {
            "Exhausted"
        };
        let source = format!(
            r#"seq.produce {{
  acquire: {{ () => if {demand} == 0 then raise (error "EagerAcquire" "no demand") else 0 }},
  step: {{ n => if n < {count} then some [n * n, n + 1] else Option.None }},
  release: {{ state reason =>
    if state == {consumed} and (match reason of {{ seq.CloseReason.{reason} => true, _ => false }})
    then () else raise (error "InvalidRelease" "state or reason differs")
  }}
}} |> take {demand} |> sum"#,
        );
        let module = rill_syntax::parse("producer-fuzz", &source).unwrap();
        let mut engine = Engine::standard().unwrap();
        engine.begin(&module).unwrap();
        loop {
            match engine.step(16).unwrap() {
                Progress::Complete => {
                    assert!(
                        engine.inspect(|value| matches!(value, Value::Int(n) if n == expected))
                    );
                    return;
                }
                Progress::Yielded => engine.collect(),
                Progress::Waiting => panic!("pure producer requested a host effect"),
            }
        }
    });
}
