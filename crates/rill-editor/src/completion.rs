//! Bounded metadata requests cross threads; language values and evaluation never do.
use reedline::{
    Completer, CompletionOrigin, CompletionResult, CompletionStatus, Span, Suggestion, Suggestions,
};
use rill_syntax::completion::{PathQuery, Query, path_query, query};
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
        let range = match &target {
            Target::Names(query) => &query.span,
            Target::Paths(query) => &query.span,
        };
        self.origin = Some(CompletionOrigin::new(line, pos));
        let span = Span {
            start: range.start,
            end: range.end,
        };
        let (reply, receiver) = oneshot::channel();
        self.queued = Some(Request { target, reply });
        self.pending = Some(Pending {
            reply: receiver,
            span,
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
        self.ready = Some(
            candidates
                .into_iter()
                .map(|(value, kind)| Suggestion {
                    value,
                    description: Some(kind),
                    span: pending.span,
                    append_whitespace: false,
                    ..Suggestion::default()
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
