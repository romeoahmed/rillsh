//! Input ownership ends before evaluation can hand the terminal to a foreground job.
use crate::{
    Color,
    session::{InputEvent, Session},
};
use rill_editor::{Editor, Notifications, Signal};
use std::{collections::VecDeque, io};

#[derive(Default)]
pub struct Input {
    editor: Option<Editor>,
    plain: Plain,
    notifications: Notifications,
    pub status: u8,
}
impl Input {
    pub fn take_module(&mut self, source: &str) -> Option<rill_syntax::ast::Module> {
        self.editor
            .as_mut()
            .and_then(|editor| editor.take_module(source))
    }
    pub fn restore(&mut self, source: String) {
        if let Some(editor) = &mut self.editor {
            editor.restore(source);
        }
    }
    #[expect(
        clippy::future_not_send,
        reason = "The session and traced VM stay on the coordinator thread"
    )]
    pub async fn read(&mut self, session: &mut Session, color: Color) -> io::Result<Signal> {
        // Reedline paints stderr and resets the stdout cursor on drop. Its supported API
        // cannot redirect those endpoints, so redirected sessions use the controlling tty.
        let rich = self.plain.pending.is_empty()
            && session.terminal_hints().supports_ansi()
            && session.terminal().has_dimensions()
            && session.terminal().owns_standard_streams();
        if !rich {
            drop(self.editor.take());
            for notice in self.notifications.drain() {
                session.terminal().write(notice.as_bytes())?;
                session.terminal().finish_line()?;
            }
            return self.plain.read(session, &self.notifications).await;
        }
        let depth = session.color(color, true);
        let mut editor = self
            .editor
            .take()
            .unwrap_or_else(|| Editor::new(depth, self.notifications.clone()));
        editor.set_color(depth);
        editor.set_context(session.prompt_context(self.status));
        let (sender, mut requests) = tokio::sync::mpsc::channel(1);
        editor.complete_with(sender);
        let control = editor.control();
        let notifications = editor.notifications();
        let mut input = tokio::task::spawn_blocking(move || {
            let signal = editor.read();
            (editor, signal)
        });
        let mut worker: Option<(
            tokio::task::JoinHandle<()>,
            Option<tokio::sync::oneshot::Sender<()>>,
        )> = None;
        let mut queued = None;
        let result = loop {
            if worker.is_none()
                && let Some(request) = queued.take()
                && let Ok((launcher, snapshot)) = session.completion_context()
            {
                let (cancel, cancelled) = tokio::sync::oneshot::channel();
                worker = Some((
                    tokio::spawn(crate::completion::complete(
                        request, launcher, snapshot, cancelled,
                    )),
                    Some(cancel),
                ));
            }
            tokio::select! {
                result = &mut input => break result,
                event = session.input_event(&notifications) => {
                    control.interrupt(matches!(event, InputEvent::Interrupt));
                    break input.await;
                }
                Some(request) = requests.recv() => {
                    if let Some((_, cancel)) = &mut worker { cancel.take(); }
                    queued = None;
                    if !request.is_cancelled() {
                        match &request.target {
                            rill_editor::completion::Target::Names(query) => {
                                let candidates = session.engine.complete(query);
                                request.respond(candidates);
                            }
                            rill_editor::completion::Target::Paths(_) => queued = Some(request),
                        }
                    }
                }
                () = async {
                    if let Some((task, _)) = &mut worker { let _ = task.await; }
                    else { std::future::pending::<()>().await; }
                } => { worker = None; }
            }
        };
        if let Some((worker, cancel)) = worker {
            drop(cancel);
            worker.await.map_err(io::Error::other)?;
        }
        let result = result.map_err(io::Error::other)?;
        self.editor = Some(result.0);
        if let Ok(Signal::HostCommand(command)) = &result.1 {
            self.host_command(command, session, color).await?;
        }
        result.1
    }
    #[expect(
        clippy::future_not_send,
        reason = "The editor shares the coordinator's session"
    )]
    async fn host_command(
        &mut self,
        command: &str,
        session: &mut Session,
        color: Color,
    ) -> io::Result<()> {
        match command {
            "suspend" => session.terminal().suspend()?,
            "edit" => {
                let source = self
                    .editor
                    .as_ref()
                    .map_or_else(String::new, Editor::buffer);
                match session.edit_buffer(&source).await {
                    Ok(source) => self.restore(source),
                    Err(error) if error.is_cancelled() => {}
                    Err(error) => {
                        crate::report_error(&error, session.color(color, true));
                    }
                }
            }
            "help" => {
                session.terminal().write(b"\r\nEnter: submit or continue  Alt-Enter/Ctrl-J: newline\r\nTab: complete  Ctrl-R: search history  Ctrl-O: external editor  Ctrl-_: undo  Alt-r: redo\r\nShift-Tab: previous completion  Ctrl-C: cancel  Ctrl-D: EOF  Ctrl-Z: suspend\r\n")?;
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Default)]
struct Plain {
    pending: VecDeque<u8>,
}
impl Plain {
    #[expect(
        clippy::future_not_send,
        reason = "Canonical input shares the session's coordinator"
    )]
    async fn read(
        &mut self,
        session: &mut Session,
        notifications: &Notifications,
    ) -> io::Result<Signal> {
        let mut reader = session.terminal().reader()?;
        let result = self.read_entry(session, notifications, &mut reader).await;
        // Joining the reader precedes queue flushing, error reporting and all execution.
        reader.close().await?;
        if matches!(result, Ok(Signal::CtrlC)) {
            session.terminal().cancel_input()?;
        }
        result
    }
    #[expect(
        clippy::future_not_send,
        reason = "Canonical input shares the session's coordinator"
    )]
    async fn read_entry(
        &mut self,
        session: &mut Session,
        notifications: &Notifications,
        reader: &mut rill_system::terminal::Reader,
    ) -> io::Result<Signal> {
        const ENTRY_LIMIT: usize = 1024 * 1024;
        let mut source = Vec::new();
        let mut oversized = false;
        session.terminal().write(b"rill> ")?;
        loop {
            if self.pending.is_empty() {
                let chunk = tokio::select! {
                    chunk = reader.next() => chunk?,
                    event = session.input_event(notifications) => {
                        match event {
                            InputEvent::Interrupt => {
                                self.pending.clear();
                                return Ok(Signal::CtrlC);
                            }
                            InputEvent::Suspend => {
                                reader.pause().await?;
                                session.terminal().suspend()?;
                                session.terminal().write(if source.is_empty() { b"rill> " } else { b"... " })?;
                                continue;
                            }
                        }
                    }
                };
                if chunk.is_empty() {
                    session.terminal().finish_line()?;
                    if oversized {
                        return Err(entry_too_large());
                    }
                    // Submit the whole accumulated entry so EOF diagnoses incomplete
                    // syntax before any valid prefix could execute.
                    return if source.is_empty() {
                        Ok(Signal::CtrlD)
                    } else {
                        entry(source).map(Signal::Success)
                    };
                }
                self.pending.extend(chunk);
            }
            while let Some(byte) = self.pending.pop_front() {
                if source.len() < ENTRY_LIMIT {
                    source.push(byte);
                } else {
                    oversized = true;
                }
                if byte != b'\n' {
                    continue;
                }
                if oversized {
                    return Err(entry_too_large());
                }
                let text = entry(std::mem::take(&mut source))?;
                if rill_syntax::parse("<input>", &text)
                    .is_err_and(|errors| errors.iter().all(|error| error.incomplete))
                {
                    source = text.into_bytes();
                    session.terminal().write(b"... ")?;
                } else {
                    return Ok(Signal::Success(text));
                }
            }
        }
    }
}
fn entry(source: Vec<u8>) -> io::Result<String> {
    String::from_utf8(source)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "input must be UTF-8"))
}
fn entry_too_large() -> io::Error {
    io::Error::new(
        io::ErrorKind::FileTooLarge,
        "entry exceeds the 1 MiB input limit",
    )
}
