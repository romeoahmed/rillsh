//! Command-line entry; the isolated launch helper runs before any runtime starts.
mod diagnostic;
mod input;
use diagnostic::{render as diagnostic, report as report_error};
mod session;
use clap::{Parser, ValueEnum};
use rill_editor::profile::ColorChoice;
use std::{
    ffi::OsString,
    io::{self, IsTerminal, Read},
    os::unix::ffi::OsStrExt,
    path::PathBuf,
    process::ExitCode,
};

#[derive(Clone, Copy, ValueEnum)]
enum Color {
    Auto,
    Always,
    Never,
}
impl From<Color> for ColorChoice {
    fn from(value: Color) -> Self {
        match value {
            Color::Auto => Self::Auto,
            Color::Always => Self::Always,
            Color::Never => Self::Never,
        }
    }
}
#[derive(Parser)]
#[command(version, about)]
struct Options {
    /// Evaluate source instead of reading a file or standard input.
    #[arg(short = 'c', conflicts_with_all = ["file", "interactive"])]
    command: Option<String>,
    /// Read interactive entries from the controlling terminal.
    #[arg(short = 'i', conflicts_with = "file")]
    interactive: bool,
    /// Skip the interactive startup file.
    #[arg(long)]
    no_config: bool,
    /// Read this startup file instead of the default user configuration.
    #[arg(long, value_name = "FILE", conflicts_with_all = ["no_config", "command", "file"])]
    config: Option<PathBuf>,
    /// Control editor and diagnostic colors.
    #[arg(long, value_enum, default_value = "auto")]
    color: Color,
    /// Source file, followed by its native byte arguments.
    file: Option<PathBuf>,
    #[arg(trailing_var_arg = true, requires = "file", allow_hyphen_values = true)]
    arguments: Vec<OsString>,
}
mod completion;
fn main() -> ExitCode {
    let mut arguments = std::env::args_os();
    arguments.next();
    match arguments.next().as_deref() {
        Some(argument) if argument == "--internal-exec" => {
            // SAFETY: process startup precedes threads and inherited-descriptor owners.
            return match unsafe { rill_system::helper::run(arguments) } {
                Ok(code) => ExitCode::from(code),
                Err(error) => {
                    diagnostic::message(format_args!("launch helper: {error}"));
                    ExitCode::FAILURE
                }
            };
        }
        Some(argument) if argument == "--internal-write" => {
            return if rill_system::writer::worker().is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };
        }
        Some(argument) if argument == "--internal-glob" => {
            return match glob_worker(arguments) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    diagnostic::message(format_args!("glob: {error}"));
                    ExitCode::FAILURE
                }
            };
        }
        Some(argument) if argument == "--internal-complete" => {
            return match completion::worker(arguments) {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => ExitCode::FAILURE,
            };
        }
        _ => {}
    }
    let options = Options::parse();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            diagnostic::message(error);
            return ExitCode::FAILURE;
        }
    };
    ExitCode::from(runtime.block_on(run(options)))
}
// This fresh process inherits its cwd through the ordinary gated launch helper.
// NUL separates paths without Unicode conversion or shell interpretation.
fn glob_worker(mut arguments: impl Iterator<Item = OsString>) -> io::Result<()> {
    use std::{ffi::CString, io::Write};
    let pattern = arguments
        .next()
        .filter(|_| arguments.next().is_none())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "expected one glob pattern"))?;
    let pattern = CString::new(pattern.as_bytes()).map_err(io::Error::other)?;
    let mut output = io::stdout().lock();
    for path in rill_system::glob::expand(&pattern)? {
        output.write_all(path.as_os_str().as_bytes())?;
        output.write_all(&[0])?;
    }
    output.flush()
}
#[expect(
    clippy::future_not_send,
    reason = "The traced VM and its coordinator run exclusively on the current-thread runtime"
)]
async fn run(options: Options) -> u8 {
    let is_interactive = options.interactive
        || (options.file.is_none() && options.command.is_none() && io::stdin().is_terminal());
    let input = if is_interactive {
        None
    } else {
        match source(&options) {
            Ok(source) => Some(source),
            Err(error) => {
                diagnostic::message(error);
                return 1;
            }
        }
    };
    let mut session = match session::Session::new(
        options
            .arguments
            .iter()
            .map(|arg| arg.as_bytes().to_vec())
            .collect(),
    ) {
        Ok(session) => session,
        Err(error) => {
            diagnostic::message(error);
            return 1;
        }
    };
    if is_interactive {
        let code = interactive(&mut session, &options).await;
        if let Err(error) = session.shutdown(false).await {
            report_error(
                &error,
                session.color(options.color, io::stderr().is_terminal()),
            );
            return 1;
        }
        return code;
    }
    let (name, source) = input.expect("noninteractive source is loaded before signal registration");
    let module = match rill_syntax::parse(&name, &source) {
        Ok(module) => module,
        Err(errors) => {
            for error in errors {
                diagnostic(
                    &name,
                    &source,
                    error.span,
                    "ParseError",
                    &error.message,
                    session.color(options.color, io::stderr().is_terminal()),
                );
            }
            return 2;
        }
    };
    let code = match session
        .evaluate(
            &module,
            options.file.as_deref().and_then(std::path::Path::parent),
            false,
        )
        .await
    {
        Ok(()) => session.exit.unwrap_or(0),
        Err(error) => {
            report_error(
                &error,
                session.color(options.color, io::stderr().is_terminal()),
            );
            if error.is_cancelled() {
                130
            } else {
                error.exit_status.unwrap_or(1)
            }
        }
    };
    if let Err(error) = session.shutdown(true).await {
        report_error(
            &error,
            session.color(options.color, io::stderr().is_terminal()),
        );
        return if code == 0 { 1 } else { code };
    }
    code
}
fn source(options: &Options) -> io::Result<(String, String)> {
    if let Some(source) = &options.command {
        return Ok(("<command>".into(), source.clone()));
    }
    if let Some(path) = &options.file {
        return Ok((path.to_string_lossy().into_owned(), read_source_file(path)?));
    }
    let mut source = String::new();
    io::stdin()
        .take(16 * 1024 * 1024 + 1)
        .read_to_string(&mut source)?;
    if source.len() > 16 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            "script exceeds the source byte limit",
        ));
    }
    Ok(("<stdin>".into(), source))
}
fn read_source_file(path: &std::path::Path) -> io::Result<String> {
    rill_system::source::Directory::open(std::path::Path::new("."))?
        .source(path)?
        .read(16 * 1024 * 1024)
}
#[expect(
    clippy::future_not_send,
    reason = "The traced VM and its coordinator run exclusively on the current-thread runtime"
)]
async fn interactive(session: &mut session::Session, options: &Options) -> u8 {
    if let Err(error) = session.enable_interaction() {
        diagnostic::message(error);
        return 1;
    }
    if !options.no_config
        && let Some(path) = options.config.clone().or_else(rill_editor::startup_file)
    {
        match read_source_file(&path) {
            Ok(source) => {
                submit(
                    session,
                    &path.to_string_lossy(),
                    &source,
                    options.color,
                    false,
                    path.parent(),
                )
                .await;
            }
            Err(error) if options.config.is_none() && error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => diagnostic::message(format_args!("startup file: {error}")),
        }
    }
    let mut input = input::Input::default();
    loop {
        if let Some(code) = session.exit {
            return code;
        }
        let signal = input.read(session, options.color).await;
        match signal {
            Ok(rill_editor::Signal::Success(source)) => {
                submit(session, "<input>", &source, options.color, true, None).await;
            }
            Ok(rill_editor::Signal::CtrlD) => match session.can_exit() {
                Ok(()) => return 0,
                Err(error) => diagnostic::message(error),
            },
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::InvalidData | io::ErrorKind::FileTooLarge
                ) =>
            {
                diagnostic::message(format_args!("input: {error}"));
            }
            Err(error) => {
                diagnostic::message(format_args!("input: {error}"));
                return 1;
            }
        }
    }
}
#[expect(
    clippy::future_not_send,
    reason = "The traced VM and its coordinator run exclusively on the current-thread runtime"
)]
async fn submit(
    session: &mut session::Session,
    name: &str,
    source: &str,
    color: Color,
    display: bool,
    directory: Option<&std::path::Path>,
) {
    let module = match rill_syntax::parse(name, source) {
        Ok(module) => module,
        Err(errors) => {
            for error in errors {
                diagnostic(
                    name,
                    source,
                    error.span,
                    "ParseError",
                    &error.message,
                    session.color(color, io::stderr().is_terminal()),
                );
            }
            return;
        }
    };
    match session.evaluate(&module, directory, display).await {
        Ok(()) if display => {
            if let Some(value) = session.engine.inspect(|value| {
                rill_runtime::presentation::render(value, session.terminal().columns())
            }) {
                eprintln!("{value}");
            }
        }
        Ok(()) => {}
        Err(error) if error.is_cancelled() && error.notes.is_empty() => {}
        Err(error) => {
            report_error(&error, session.color(color, io::stderr().is_terminal()));
        }
    }
}
