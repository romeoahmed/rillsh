//! Controlled readiness checks callback scheduling and ownership independently of OS timing.
use rill_runtime::{
    Engine, Error, Progress,
    host::{Request, Response},
    value::Value,
};
use rill_system::resources::{Item, SourceError, SourceId};
use slotmap::SlotMap;

fn drive(engine: &mut Engine, mut service: impl FnMut(Request) -> Response) -> Result<(), Error> {
    loop {
        if engine.step(8)? == Progress::Complete {
            return Ok(());
        }
        engine.collect();
        if let Some(request) = engine.take_request() {
            let request = match request {
                Request::Defer(request)
                    if matches!(*request, Request::Write(_) | Request::Display(_)) =>
                {
                    *request
                }
                request => request,
            };
            engine.resume(Ok(service(request)))?;
        }
    }
}

#[test]
fn a_deferred_host_wait_is_cancelled_when_its_peer_completes() {
    let mut keys = SlotMap::<SourceId, ()>::with_key();
    let operation = keys.insert(());
    let peer = keys.insert(());
    let mut engine = Engine::standard().unwrap();
    engine
        .begin(
            &rill_syntax::parse(
                "host-cutoff",
                r#"do {
  let input = seq.produce {
    acquire: { () => () },
    step: { () => some [read_text "pending", ()] },
    release: { _ _ => print "release" }
  }
  merge [input, stream plan { ^peer }] |> take 1 |> collect
}"#,
            )
            .unwrap(),
        )
        .unwrap();
    let mut started = false;
    let mut closed = Vec::new();
    let mut output = Vec::new();
    drive(&mut engine, |request| match request {
        Request::Defer(request) => {
            assert!(matches!(*request, Request::ReadText { .. }));
            started = true;
            Response::Deferred(operation)
        }
        Request::Stream(_) => Response::Source(peer),
        Request::ReadSources { keys, .. } => {
            if started && let Some(index) = keys.iter().position(|key| *key == peer) {
                Response::Item {
                    index,
                    value: Some(Item::Bytes(b"ready".to_vec())),
                }
            } else {
                Response::Pending
            }
        }
        Request::Write(bytes) => {
            output.extend(bytes);
            assert!(
                closed.contains(&operation),
                "pending work must stop before release"
            );
            Response::Unit
        }
        Request::Close(keys) => {
            closed.extend(keys);
            Response::Unit
        }
        request => panic!("unexpected request {request:?}"),
    })
    .unwrap();
    assert!(started);
    assert_eq!(output, b"release\n");
    assert!(closed.contains(&operation));
    assert!(
        engine
            .inspect(|value| matches!(value, Value::List(values) if values.as_slice().len() == 1))
    );
}

#[test]
fn nested_deferred_failure_reaches_the_original_callback_handler() {
    let mut keys = SlotMap::<SourceId, ()>::with_key();
    let operation = keys.insert(());
    let mut engine = Engine::standard().unwrap();
    engine
        .begin(
            &rill_syntax::parse(
                "host-error",
                r#"do {
  let input = items [()] |> map { () => match attempt { () => read_text "missing" } of {
    Result.Err {error} => if error.kind == "IOError" then 40 else 0,
    _ => 0
  } }
  merge [merge [input, items [1]], items [2]] |> fold { sum n => sum + n } 0
}"#,
            )
            .unwrap(),
        )
        .unwrap();
    let mut probes = 0;
    drive(&mut engine, |request| match request {
        Request::Defer(_) => Response::Deferred(operation),
        Request::ReadSources { keys, wait } => {
            let index = keys.iter().position(|key| *key == operation).unwrap();
            probes += 1;
            if wait {
                Response::Operation {
                    index,
                    result: Err(std::io::Error::from(std::io::ErrorKind::NotFound).into()),
                }
            } else {
                Response::Pending
            }
        }
        Request::Close(_) => Response::Unit,
        request => panic!("unexpected request {request:?}"),
    })
    .unwrap();
    assert!(probes > 1);
    assert!(engine.inspect(|value| matches!(value, Value::Int(43))));
}

#[test]
fn blocked_producer_callback_does_not_delay_a_ready_peer_or_its_cutoff() {
    let mut keys = SlotMap::<SourceId, ()>::with_key();
    let slow = keys.insert(());
    let peer = keys.insert(());
    let mut engine = Engine::standard().unwrap();
    engine
        .begin(
            &rill_syntax::parse(
                "merge-callback",
                r#"do {
  let input = seq.produce {
    acquire: { () => stdin () },
    step: { input => some [collect_bytes input, ()] },
    release: { _ reason => print (match reason of {
      seq.CloseReason.Cutoff => "cutoff",
      _ => "wrong reason"
    }) }
  }
  merge [input, stream plan { ^peer }] |> take 1 |> collect
}"#,
            )
            .unwrap(),
        )
        .unwrap();
    let mut blocked = false;
    let mut closed = Vec::new();
    let mut output = Vec::new();
    drive(&mut engine, |request| match request {
        Request::Stdin => Response::Source(slow),
        Request::Stream(_) => Response::Source(peer),
        Request::ReadSources { keys, wait } => {
            if keys.contains(&slow) {
                blocked = true;
                assert!(!wait, "callback must initially probe without blocking");
            }
            if blocked && let Some(index) = keys.iter().position(|key| *key == peer) {
                Response::Item {
                    index,
                    value: Some(Item::Bytes(b"ready".to_vec())),
                }
            } else {
                Response::Pending
            }
        }
        Request::Write(bytes) => {
            output.extend(bytes);
            Response::Unit
        }
        Request::Close(keys) => {
            closed.extend(keys);
            Response::Unit
        }
        request => panic!("unexpected host request {request:?}"),
    })
    .unwrap();
    assert!(blocked);
    assert_eq!(output, b"cutoff\n");
    assert!(closed.contains(&slow) && closed.contains(&peer));
    assert!(engine.inspect(|value| matches!(value, Value::List(items) if matches!(items.as_slice(), [Value::Bytes(bytes)] if bytes.0.as_ref() == b"ready"))));
}

#[test]
fn source_failure_is_caught_inside_its_callback_before_other_inputs_are_closed() {
    let mut keys = SlotMap::<SourceId, ()>::with_key();
    let source = keys.insert(());
    let mut engine = Engine::standard().unwrap();
    engine
        .begin(
            &rill_syntax::parse(
                "callback-error",
                r#"do {
  let input = seq.produce {
    acquire: { () => stdin () },
    step: { state => match state of {
      () => Option.None,
      input => some [match attempt { () => collect_bytes input } of {
        Result.Err {error} => if error.kind == "IOError" then 42 else 0,
        Result.Ok {value} => 0
      }, ()]
    } },
    release: { _ _ => () }
  }
  merge [merge [input, items [1]], items [2]] |> collect |> sum
}"#,
            )
            .unwrap(),
        )
        .unwrap();
    let mut failed = false;
    let mut closed = Vec::new();
    drive(&mut engine, |request| match request {
        Request::Stdin => Response::Source(source),
        Request::ReadSources { keys, wait } => {
            if !wait {
                return Response::Pending;
            }
            let index = keys.iter().position(|key| *key == source).unwrap();
            assert!(!failed);
            failed = true;
            Response::ReadFailure {
                index,
                error: Box::new(SourceError::Io(std::io::Error::from(
                    std::io::ErrorKind::BrokenPipe,
                ))),
            }
        }
        Request::Close(keys) => {
            closed.extend(keys);
            Response::Unit
        }
        request => panic!("unexpected host request {request:?}"),
    })
    .unwrap();
    assert!(failed && closed.contains(&source));
    assert!(engine.inspect(|value| matches!(value, Value::Int(45))));
}

#[test]
fn a_nonterminating_callback_yields_and_is_discarded_after_peer_cutoff() {
    let mut engine = Engine::standard().unwrap();
    engine
        .begin(
            &rill_syntax::parse(
                "callback-quantum",
                r"
fn spin () = spin ()
merge [unfold { _ => spin () } (), items [42]] |> take 1 |> collect |> sum
",
            )
            .unwrap(),
        )
        .unwrap();
    drive(&mut engine, |request| match request {
        Request::Close(_) => Response::Unit,
        request => panic!("unexpected effect {request:?}"),
    })
    .unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::Int(42))));
}

#[test]
fn peer_cutoff_finishes_an_already_started_release_callback() {
    let mut keys = SlotMap::<SourceId, ()>::with_key();
    let release_input = keys.insert(());
    let peer = keys.insert(());
    let mut engine = Engine::standard().unwrap();
    engine
        .begin(
            &rill_syntax::parse(
                "protected-callback",
                r#"do {
  let input = unfold { state => do {
    seq.produce {
      acquire: { () => () },
      step: { _ => Option.None },
      release: { _ _ => do {
        print "release begin"
        stdin () |> collect_bytes
        print "release end"
      } }
    } |> collect
    some [0, state]
  } } ()
  merge [input, stream plan { ^peer }] |> take 1 |> collect
}"#,
            )
            .unwrap(),
        )
        .unwrap();
    let mut releasing = false;
    let mut delivered = false;
    let mut output = Vec::new();
    drive(&mut engine, |request| match request {
        Request::Stdin => Response::Source(release_input),
        Request::Stream(_) => Response::Source(peer),
        Request::ReadSources { keys, .. } => {
            releasing |= keys.contains(&release_input);
            if releasing
                && !delivered
                && let Some(index) = keys.iter().position(|key| *key == peer)
            {
                delivered = true;
                Response::Item {
                    index,
                    value: Some(Item::Bytes(b"ready".to_vec())),
                }
            } else if delivered
                && let Some(index) = keys.iter().position(|key| *key == release_input)
            {
                Response::Item { index, value: None }
            } else {
                Response::Pending
            }
        }
        Request::Write(bytes) => {
            output.extend(bytes);
            Response::Unit
        }
        Request::Close(_) => Response::Unit,
        request => panic!("unexpected request {request:?}"),
    })
    .unwrap();
    assert!(releasing && delivered);
    assert_eq!(output, b"release begin\nrelease end\n");
}
