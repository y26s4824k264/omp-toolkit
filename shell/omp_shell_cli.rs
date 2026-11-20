use std::{
    env,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, Write},
    path::{self, PathBuf},
    process::{Command, ExitCode, ExitStatus},
};

use anyhow::{Context, Result, bail, ensure};
use omp_shell::{
    minimizer::{self, MinimizerConfig},
    process::{self, ProcessOptions},
};

mod log_preview;

const USAGE: &str = "\
Usage: omp-shell bash|zsh|sh [shell arguments]\n\
       omp-shell run-log [--log PATH] -- PROGRAM [ARGS...]\n\
       omp-shell minimize LOG EXIT_CODE COMMAND\n\
       omp-shell --version";

const PREVIEW_BYTES: usize = 12_000;

fn main() -> ExitCode {
    match run() {
        Ok(status) => ExitCode::from(status),
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<u8> {
    let mut args = env::args_os().skip(1);
    let Some(dialect) = args.next() else {
        bail!("{USAGE}")
    };
    match dialect.to_str() {
        Some("bash" | "zsh" | "sh") => {
            let argv = std::iter::once(dialect)
                .chain(args)
                .map(OsString::into_encoded_bytes);
            let status =
                std::panic::catch_unwind(|| process::main(argv, ProcessOptions { embedded: true }))
                    .unwrap_or(101);
            Ok(u8::try_from(status).unwrap_or(1))
        }
        Some("run-log") => run_log(args.collect()),
        Some("minimize") => minimize(args.collect()),
        Some("--help" | "-h") => {
            println!("{USAGE}");
            Ok(0)
        }
        Some("--version") => {
            println!("omp-shell {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        _ => bail!("{USAGE}"),
    }
}

fn run_log(args: Vec<OsString>) -> Result<u8> {
    let (destination, argv) = match args.as_slice() {
        [separator, argv @ ..] if separator == "--" => (None, argv),
        [option, path, separator, argv @ ..] if option == "--log" && separator == "--" => {
            (Some(path), argv)
        }
        _ => bail!("{USAGE}"),
    };
    let Some((program, arguments)) = argv.split_first() else {
        bail!("{USAGE}");
    };
    let (writer, log) = create_log(destination)?;
    let reader = File::open(&log).with_context(|| format!("open {}", log.display()))?;
    eprintln!("raw log: {}", log.display());

    // The duplicates refer to the same open file, so both streams append at
    // the same offset. The independent reader does not move that offset.
    let mut command = Command::new(program);
    command
        .args(arguments)
        .stdout(writer.try_clone()?)
        .stderr(writer);
    let status = match wait_for_command(command) {
        Ok(status) => exit_code(status),
        Err(error) => {
            eprintln!("run {}: {error}", program.to_string_lossy());
            if error.kind() == io::ErrorKind::NotFound {
                127
            } else {
                126
            }
        }
    };

    if let Err(error) = print_preview(reader, status, argv) {
        eprintln!("exit {status}; preview unavailable: {error:#}");
    }
    Ok(status)
}

#[cfg(unix)]
fn wait_for_command(command: Command) -> io::Result<ExitStatus> {
    use omp_shell::proctree::Process;
    use tokio::signal::unix::{SignalKind, signal};

    // Keep the child in the foreground process group so the terminal still
    // delivers Ctrl+C and other terminal signals to the whole job.
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            let mut terminate = signal(SignalKind::terminate())?;
            let mut child = tokio::process::Command::from(command).spawn()?;
            loop {
                tokio::select! {
                    biased;
                    result = child.wait() => return result,
                    _ = terminate.recv() => {
                        if let Some(process) = child.id().and_then(|id| Process::from_pid(id as i32)) {
                            let _ = process.kill_tree(Some(libc::SIGTERM));
                        }
                    }
                }
            }
        })
}

#[cfg(not(unix))]
fn wait_for_command(mut command: Command) -> io::Result<ExitStatus> {
    command.status()
}

fn create_log(destination: Option<&OsString>) -> Result<(File, PathBuf)> {
    if let Some(destination) = destination {
        let log = path::absolute(destination)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(&log)
            .with_context(|| format!("create {}", log.display()))?;
        return Ok((file, log));
    }
    tempfile::Builder::new()
        .prefix("omp-run-")
        .suffix(".log")
        .tempfile()?
        .keep()
        .context("keep the raw command log")
}

fn exit_code(status: ExitStatus) -> u8 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return u8::try_from(128 + signal).unwrap_or(1);
        }
    }
    status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1)
}

fn print_preview(mut log: File, status: u8, argv: &[OsString]) -> Result<()> {
    let size = log.metadata()?.len();
    let config = minimizer_config();
    if status == 0
        && (PREVIEW_BYTES as u64 + 1..=u64::from(config.max_capture_bytes)).contains(&size)
    {
        let command = argv
            .iter()
            .map(|arg| {
                arg.to_str()
                    .map(|text| format!("'{}'", text.replace('\'', "'\\''")))
            })
            .collect::<Option<Vec<_>>>()
            .map(|parts| parts.join(" "));
        if let Some(command) =
            command.filter(|command| minimizer::engine::should_minimize(command, &config))
        {
            let mut captured = Vec::new();
            (&mut log).take(size).read_to_end(&mut captured)?;
            if let Ok(captured) = std::str::from_utf8(&captured) {
                let output = minimizer::apply(&command, captured, i32::from(status), &config);
                if output.changed && output.text.len() <= PREVIEW_BYTES {
                    eprintln!(
                        "exit {status}; {}: {size} → {} bytes",
                        output.filter, output.output_bytes
                    );
                    io::stdout().lock().write_all(output.text.as_bytes())?;
                    return Ok(());
                }
            }
            log.rewind()?;
        }
    }
    let preview = log_preview::log_excerpt(log, PREVIEW_BYTES)?;
    eprintln!(
        "exit {status}; raw log: {size} bytes; preview: {} bytes",
        preview.len()
    );
    io::stdout().lock().write_all(preview.as_bytes())?;
    Ok(())
}

fn minimizer_config() -> MinimizerConfig {
    MinimizerConfig {
        enabled: true,
        ..MinimizerConfig::default()
    }
}

fn minimize(args: Vec<OsString>) -> Result<u8> {
    let [log, status, command] = args.as_slice() else {
        bail!("{USAGE}")
    };
    let log = PathBuf::from(log);
    let status: u8 = status.to_str().context("expected an exit code")?.parse()?;
    let command = command
        .to_str()
        .context("expected a UTF-8 command description")?;
    let config = minimizer_config();
    let file = fs::File::open(&log).with_context(|| format!("open {}", log.display()))?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file(),
        "expected a regular saved log: {}",
        log.display()
    );
    ensure!(
        metadata.len() <= u64::from(config.max_capture_bytes),
        "log exceeds the OMP minimizer input limit; inspect selected ranges of {}",
        log.display()
    );
    let limit = u64::from(config.max_capture_bytes);
    let mut captured = String::new();
    file.take(limit + 1)
        .read_to_string(&mut captured)
        .with_context(|| format!("read {}", log.display()))?;
    ensure!(
        captured.len() as u64 <= limit,
        "log grew while reading; inspect selected ranges of {}",
        log.display()
    );
    let result = minimizer::apply(command, &captured, i32::from(status), &config);
    eprintln!(
        "exit {status}; {}: {} → {} bytes; raw log: {}",
        result.filter,
        result.input_bytes,
        result.output_bytes,
        log.display()
    );
    io::stdout().lock().write_all(result.text.as_bytes())?;
    Ok(status)
}
