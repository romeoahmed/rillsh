//! Bounded metadata requests cross threads; language values and evaluation never do.
use reedline::{
    Completer, CompletionOrigin, CompletionResult, CompletionStatus, Span, Suggestion, Suggestions,
};
use rill_syntax::completion::{PathQuery, Query, path_query, query};
use std::collections::BTreeMap;
use tokio::sync::{mpsc, oneshot};

/// One metadata query; dropping the reply invalidates the request without shared state.
pub struct Request {
    pub target: Target,
    reply: oneshot::Sender<Vec<(String, String)>>,
}
pub enum Target {
    Names(Query),
    Paths(PathQuery),
}
impl Request {
    /// Deliver owned names and their value categories to the requesting editor revision.
    pub fn respond(self, candidates: Vec<(String, String)>) {
        let _ = self.reply.send(candidates);
    }
    /// Whether a newer request or editor exit has already discarded this query.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.reply.is_closed()
    }
    /// Wait until the editor discards the originating query.
    pub async fn cancelled(&mut self) {
        self.reply.closed().await;
    }
}

struct Pending {
    reply: oneshot::Receiver<Vec<(String, String)>>,
    span: Span,
    paths: bool,
    at_end: bool,
    locals: Vec<String>,
}
/// A completer is recreated at each entry boundary so cached metadata cannot cross publication.
pub(crate) struct Completion {
    sender: mpsc::Sender<Request>,
    origin: Option<CompletionOrigin>,
    pending: Option<Pending>,
    queued: Option<Request>,
    ready: Option<Suggestions>,
}
impl Completion {
    pub const fn new(sender: mpsc::Sender<Request>) -> Self {
        Self {
            sender,
            origin: None,
            pending: None,
            queued: None,
            ready: None,
        }
    }
}
impl Completer for Completion {
    fn complete(&mut self, line: &str, pos: usize) -> CompletionResult {
        if self
            .origin
            .as_ref()
            .is_some_and(|origin| origin.matches(line, pos))
        {
            return self
                .ready
                .as_ref()
                .map_or(CompletionResult::Pending, |ready| {
                    CompletionResult::fresh(ready.clone())
                });
        }
        self.pending = None;
        self.queued = None;
        self.ready = None;
        self.origin = None;
        let target = query(line, pos)
            .map(Target::Names)
            .or_else(|| path_query(line, pos).map(Target::Paths));
        let Some(target) = target else {
            return CompletionResult::fresh(Vec::new());
        };
        let locals = if let Target::Names(query) = &target {
            query.local_names(line)
        } else {
            Vec::new()
        };
        if matches!(&target, Target::Names(query) if !query.parents.is_empty())
            && !locals.is_empty()
        {
            // Local initializers are not evaluated, so their fields are unknown.
            return CompletionResult::fresh(Vec::new());
        }
        let range = match &target {
            Target::Names(query) => &query.span,
            Target::Paths(query) => &query.span,
        };
        self.origin = Some(CompletionOrigin::new(line, pos));
        let span = Span {
            start: range.start,
            end: range.end,
        };
        let paths = matches!(target, Target::Paths(_));
        let (reply, receiver) = oneshot::channel();
        self.queued = Some(Request { target, reply });
        self.pending = Some(Pending {
            reply: receiver,
            span,
            paths,
            at_end: pos == line.len(),
            locals,
        });
        CompletionResult::Pending
    }
    fn poll_completion(&mut self) -> CompletionStatus {
        if let Some(request) = self.queued.take() {
            match self.sender.try_send(request) {
                Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
                Err(mpsc::error::TrySendError::Full(request)) => self.queued = Some(request),
            }
        }
        let Some(pending) = &mut self.pending else {
            return CompletionStatus::Idle;
        };
        let candidates = match pending.reply.try_recv() {
            Ok(candidates) => candidates,
            Err(oneshot::error::TryRecvError::Empty) => return CompletionStatus::Pending,
            Err(oneshot::error::TryRecvError::Closed) => Vec::new(),
        };
        let mut candidates: BTreeMap<_, _> = candidates.into_iter().collect();
        candidates.extend(
            std::mem::take(&mut pending.locals)
                .into_iter()
                .map(|name| (name, "Local binding".into())),
        );
        let mut remaining = 1024_usize * 1024;
        self.ready = Some(
            candidates
                .into_iter()
                .take(200)
                .map(|(mut value, kind)| {
                    let display_override = pending.paths.then(|| {
                        rill_syntax::token::lex(&value)
                            .ok()
                            .and_then(|tokens| match tokens.as_slice() {
                                [
                                    rill_syntax::token::Token {
                                        kind: rill_syntax::token::Kind::String(text),
                                        ..
                                    },
                                ] => Some(text.chars().flat_map(char::escape_debug).collect()),
                                _ => None,
                            })
                            .unwrap_or_else(|| value.clone())
                    });
                    let directory = kind == "Directory";
                    if directory && pending.at_end && value.starts_with('"') && value.ends_with('"')
                    {
                        // A directory is an editing prefix. Leave the quote open so the
                        // next segment stays in the same argument; a file closes it.
                        value.pop();
                    }
                    Suggestion {
                        value,
                        display_override,
                        description: Some(kind),
                        span: pending.span,
                        append_whitespace: pending.at_end && !directory,
                        ..Suggestion::default()
                    }
                })
                .take_while(|suggestion| {
                    let size = suggestion.value.len()
                        + suggestion.display_override.as_ref().map_or(0, String::len)
                        + suggestion.description.as_ref().map_or(0, String::len);
                    let Some(bytes) = remaining.checked_sub(size) else {
                        return false;
                    };
                    remaining = bytes;
                    true
                })
                .collect(),
        );
        self.pending = None;
        CompletionStatus::Ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_text_budget_includes_path_labels() {
        let (sender, mut receiver) = mpsc::channel(1);
        let mut completion = Completion::new(sender);
        let line = "^cat file";
        assert!(completion.complete(line, line.len()).is_pending());
        assert_eq!(completion.poll_completion(), CompletionStatus::Pending);
        receiver.try_recv().unwrap().respond(
            (0..200)
                .map(|index| {
                    (
                        format!("\"file_{index:03}_{}\"", "x".repeat(4000)),
                        "Path".into(),
                    )
                })
                .collect(),
        );
        assert_eq!(completion.poll_completion(), CompletionStatus::Ready);
        let result = completion.complete(line, line.len());
        assert!(!result.suggestions().is_empty());
        assert!(result.suggestions().len() < 200);
        let retained: usize = result
            .suggestions()
            .iter()
            .map(|suggestion| {
                suggestion.value.len()
                    + suggestion.display_override.as_ref().unwrap().len()
                    + suggestion.description.as_ref().unwrap().len()
            })
            .sum();
        assert!(retained <= 1024 * 1024);
    }

    #[test]
    fn path_suggestions_separate_display_insertion_and_directory_continuation() {
        for (line, cursor, candidate, kind, inserted, label, space) in [
            (
                "^cat sp",
                7,
                "\"space name/\"",
                "Directory",
                "\"space name/",
                "space name/",
                false,
            ),
            (
                "^cat fi",
                7,
                "\"file name\"",
                "Path",
                "\"file name\"",
                "file name",
                true,
            ),
            (
                "^cat fi next",
                7,
                "\"file name\"",
                "Path",
                "\"file name\"",
                "file name",
                false,
            ),
            (
                "^cat sp next",
                7,
                "\"space name/\"",
                "Directory",
                "\"space name/\"",
                "space name/",
                false,
            ),
            (
                "^cat es",
                7,
                "\"escape\\u{1b}[2J\"",
                "Path",
                "\"escape\\u{1b}[2J\"",
                "escape\\u{1b}[2J",
                true,
            ),
        ] {
            let (sender, mut receiver) = mpsc::channel(1);
            let mut completion = Completion::new(sender);
            assert!(completion.complete(line, cursor).is_pending());
            assert_eq!(completion.poll_completion(), CompletionStatus::Pending);
            receiver
                .try_recv()
                .unwrap()
                .respond(vec![(candidate.into(), kind.into())]);
            assert_eq!(completion.poll_completion(), CompletionStatus::Ready);
            let result = completion.complete(line, cursor);
            let [suggestion] = result.suggestions() else {
                panic!("one candidate")
            };
            assert_eq!(suggestion.value, inserted);
            assert_eq!(suggestion.display_override.as_deref(), Some(label));
            assert_eq!(suggestion.append_whitespace, space);
            assert_eq!(suggestion.span, Span::new(5, 7));
        }
    }

    #[test]
    fn local_bindings_hide_global_signatures_and_field_metadata() {
        let (sender, mut receiver) = mpsc::channel(1);
        let mut completion = Completion::new(sender);
        let line = "let map = 42; ma";
        assert!(completion.complete(line, line.len()).is_pending());
        assert_eq!(completion.poll_completion(), CompletionStatus::Pending);
        receiver.try_recv().unwrap().respond(vec![
            ("map".into(), "Function: map transform items".into()),
            ("max".into(), "Function: max a b".into()),
        ]);
        assert_eq!(completion.poll_completion(), CompletionStatus::Ready);
        let ready = completion.complete(line, line.len());
        assert_eq!(ready.suggestions().len(), 2);
        assert_eq!(ready.suggestions()[0].value, "map");
        assert_eq!(
            ready.suggestions()[0].description.as_deref(),
            Some("Local binding")
        );
        let line = "let text = missing (); text.sc";
        assert!(
            completion
                .complete(line, line.len())
                .suggestions()
                .is_empty()
        );
        assert_eq!(completion.poll_completion(), CompletionStatus::Idle);
        assert!(receiver.try_recv().is_err());
        assert!(completion.complete("text.sc", 7).is_pending());
    }

    #[test]
    fn requests_coalesce_and_only_the_latest_origin_can_receive_candidates() {
        let (sender, mut receiver) = mpsc::channel(1);
        let mut completion = Completion::new(sender);
        assert!(completion.complete("ma", 2).is_pending());
        assert_eq!(completion.poll_completion(), CompletionStatus::Pending);
        assert!(completion.complete("fi", 2).is_pending());
        assert_eq!(completion.poll_completion(), CompletionStatus::Pending);
        let obsolete = receiver.try_recv().unwrap();
        assert!(obsolete.is_cancelled());
        obsolete.respond(vec![("map".into(), "map transform values".into())]);
        assert_eq!(completion.poll_completion(), CompletionStatus::Pending);
        let current = receiver.try_recv().unwrap();
        assert!(matches!(&current.target, Target::Names(query) if query.prefix == "fi"));
        current.respond(vec![("filter".into(), "filter predicate values".into())]);
        assert_eq!(completion.poll_completion(), CompletionStatus::Ready);
        let ready = completion.complete("fi", 2);
        assert_eq!(ready.suggestions()[0].value, "filter");
        assert_eq!(ready.suggestions()[0].span, Span::new(0, 2));
        assert!(completion.complete("fi", 1).is_pending());
        assert_eq!(completion.poll_completion(), CompletionStatus::Pending);
        drop(receiver);
        assert_eq!(completion.poll_completion(), CompletionStatus::Ready);
        assert!(completion.complete("fi", 1).suggestions().is_empty());
    }

    #[test]
    fn unsupported_contexts_and_dropped_editors_never_leave_live_requests() {
        let (sender, mut receiver) = mpsc::channel(1);
        let mut completion = Completion::new(sender);
        assert!(completion.complete("# comment", 9).suggestions().is_empty());
        assert!(receiver.try_recv().is_err());
        assert!(completion.complete("module.fi", 9).is_pending());
        assert_eq!(completion.poll_completion(), CompletionStatus::Pending);
        let request = receiver.try_recv().unwrap();
        drop(completion);
        assert!(request.is_cancelled());
    }
}
