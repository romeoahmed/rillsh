//! An owned helper isolates blocking inherited outputs without changing their file flags.
//!
//! One frame is outstanding at a time. Its acknowledgement follows the actual write,
//! so cancellation can reap the helper and suspension never repeats a written prefix.
use crate::{
    job::{Job, LaunchMode, Snapshot, State, read_chunk},
    output::Output,
    plan::{Plan, Stage},
};
use std::{ffi::CString, io, os::unix::ffi::OsStrExt, path::Path};

/// One supervised output process and its cancellation-safe transport state.
#[must_use = "writers must be finished or cancelled before their owner exits"]
pub struct Writer {
    job: Job,
    input: Option<Output>,
    committed: bool,
    frame: Option<Frame>,
}
struct Frame {
    bytes: bytes::Bytes,
    sent: usize,
    acknowledgement: Vec<u8>,
}
impl Writer {
    /// Prepare one reusable helper. The helper inherits only the selected output.
    ///
    /// # Errors
    /// Reports launch failures after reaping any partially created child.
    pub async fn prepare(launcher: &Path, snapshot: &Snapshot, stderr: bool) -> io::Result<Self> {
        let plan = Plan {
            stages: vec![Stage {
                argv: vec![
                    CString::new(launcher.as_os_str().as_bytes())?,
                    c"--internal-write".into(),
                ],
                ..Stage::default()
            }],
        };
        let mut job =
            Job::prepare(launcher, &plan, snapshot, LaunchMode::Output { stderr }).await?;
        let input = job.stdin.take().map(Output::new);
        Ok(Self {
            job,
            input,
            committed: false,
            frame: None,
        })
    }
    /// Transfer a write into the helper's single retained frame.
    ///
    /// # Errors
    /// Rejects a second write while an earlier acknowledgement is outstanding.
    pub fn begin(&mut self, bytes: bytes::Bytes) -> io::Result<()> {
        if self.frame.is_some() {
            return Err(io::Error::other("output already has a pending write"));
        }
        self.input
            .as_mut()
            .ok_or(io::ErrorKind::BrokenPipe)?
            .enqueue(
                u64::try_from(bytes.len())
                    .map_err(io::Error::other)?
                    .to_le_bytes()
                    .to_vec(),
            )?;
        self.frame = Some(Frame {
            bytes,
            sent: 0,
            acknowledgement: Vec::new(),
        });
        Ok(())
    }
    /// Complete the retained write. Dropping this future preserves exact transport progress.
    ///
    /// # Errors
    /// Reports the original output errno or an incomplete helper response.
    pub async fn flush(&mut self) -> io::Result<()> {
        if !self.committed {
            self.job.commit().await?;
            self.committed = true;
        }
        let Some(frame) = self.frame.as_mut() else {
            return Ok(());
        };
        let input = self.input.as_mut().ok_or(io::ErrorKind::BrokenPipe)?;
        loop {
            if !input.flush().await? {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            if frame.sent == frame.bytes.len() {
                break;
            }
            let end = frame
                .sent
                .saturating_add(crate::IO_CHUNK_BYTES)
                .min(frame.bytes.len());
            input.enqueue(frame.bytes.slice(frame.sent..end))?;
            frame.sent = end;
        }
        while frame.acknowledgement.len() < size_of::<i32>() {
            let bytes =
                read_chunk(self.job.stdout.as_ref().ok_or(io::ErrorKind::BrokenPipe)?).await?;
            if bytes.is_empty() {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            frame.acknowledgement.extend(bytes);
        }
        let code = i32::from_le_bytes(
            frame
                .acknowledgement
                .as_slice()
                .try_into()
                .map_err(|_| io::Error::other("invalid output acknowledgement"))?,
        );
        self.frame = None;
        if code == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(code))
        }
    }
    /// Freeze the helper and observe its stop before returning control to the editor.
    ///
    /// # Errors
    /// Reports signal or child-observation failures.
    pub async fn suspend(&mut self) -> io::Result<()> {
        self.job.signal(rustix::process::Signal::STOP)?;
        self.job.wait().await.map(|_| ())
    }
    /// Resume the same helper and retained transport frame.
    ///
    /// # Errors
    /// Reports a failed continuation signal.
    pub fn resume(&mut self) -> io::Result<()> {
        self.job.resume()
    }
    /// Kill and reap a blocked helper; no thread remains blocked on inherited output.
    ///
    /// # Errors
    /// Reports cleanup failures after attempting every child operation.
    pub async fn cancel(&mut self) -> io::Result<()> {
        self.input.take();
        self.job.cancel().await
    }
    /// Finish an idle helper, or cancel an unfinished frame, before releasing its owner.
    ///
    /// # Errors
    /// Reports incomplete writes or child cleanup failures.
    pub async fn finish(&mut self) -> io::Result<()> {
        if !self.committed || self.frame.is_some() {
            return self.cancel().await;
        }
        self.input.take();
        match self.job.wait().await {
            Ok(State::Finished) => self.job.reap(),
            Ok(_) => self.cancel().await,
            Err(error) => match self.cancel().await {
                Ok(()) => Err(error),
                Err(cleanup) => Err(io::Error::new(
                    error.kind(),
                    format!("{error}; cleanup: {cleanup}"),
                )),
            },
        }
    }
}

/// Run only in the fresh internal helper. Standard error is its selected output;
/// standard output carries fixed-size acknowledgements, never user text.
///
/// # Errors
/// Reports malformed frames or a failed acknowledgement channel.
pub fn worker() -> io::Result<()> {
    use std::io::{Read, Write};
    crate::sys::ignore_terminal_output_signal()?;
    let mut input = io::BufReader::new(io::stdin());
    let mut reply = io::stdout().lock();
    let mut output = io::stderr().lock();
    let mut buffer = [0; crate::IO_CHUNK_BYTES];
    loop {
        let mut header = [0; size_of::<u64>()];
        if input.read(&mut header[..1])? == 0 {
            return Ok(());
        }
        input.read_exact(&mut header[1..])?;
        let mut remaining = u64::from_le_bytes(header);
        let mut code = 0;
        while remaining != 0 {
            let count =
                usize::try_from(remaining.min(buffer.len() as u64)).map_err(io::Error::other)?;
            input.read_exact(&mut buffer[..count])?;
            if code == 0
                && let Err(error) = output.write_all(&buffer[..count])
            {
                code = error
                    .raw_os_error()
                    .unwrap_or(rustix::io::Errno::IO.raw_os_error());
            }
            remaining -= count as u64;
        }
        if code == 0
            && let Err(error) = output.flush()
        {
            code = error
                .raw_os_error()
                .unwrap_or(rustix::io::Errno::IO.raw_os_error());
        }
        reply.write_all(&code.to_le_bytes())?;
        reply.flush()?;
    }
}
