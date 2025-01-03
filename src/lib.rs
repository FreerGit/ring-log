use lockfree::channel::spsc::{create, Sender};
use std::cell::RefCell;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self};

#[derive(Clone)]
pub enum LogTo {
    Ephemeral,
    File,
}

struct LogEntry {
    closure: Box<dyn FnOnce() -> String + Send>,
    log_to: LogTo,
}

pub struct Logger {
    sx: RefCell<Sender<LogEntry>>,
    file: Option<File>,
    log_to: LogTo,
    with_time: bool,
    shutdown: Arc<AtomicBool>,
}

#[derive(Clone, Copy)]
pub struct LoggerFileOptions {
    pub path: &'static str,
    pub append_mode: bool,
}

impl Logger {
    pub fn builder(log_op: Option<LoggerFileOptions>) -> Self {
        let (sx, mut rx) = create::<LogEntry>();

        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let shutdown_flag_clone = shutdown_flag.clone();
        thread::spawn(move || {
            let mut file = None;
            if let Some(op) = log_op {
                file = Some(Logger::open_log_file(op));
            }

            loop {
                match rx.recv() {
                    Err(_) => {
                        if shutdown_flag_clone.load(Ordering::Acquire) {
                            break;
                        }
                    }
                    Ok(entry) => {
                        let mut message = (entry.closure)();

                        match entry.log_to {
                            LogTo::File => {
                                message.push('\n');
                                let f = file.as_mut().unwrap();
                                f.write_all(message.as_bytes()).unwrap();
                                f.flush().unwrap();
                            }
                            LogTo::Ephemeral => println!("{}", message),
                        };
                    }
                }
            }
        });

        let file = log_op.map(Logger::open_log_file);

        Logger {
            sx: RefCell::new(sx),
            file,
            log_to: log_op.map_or(LogTo::Ephemeral, |_| LogTo::File),
            with_time: false,
            shutdown: shutdown_flag,
        }
    }

    fn open_log_file(op: LoggerFileOptions) -> File {
        File::options()
            .write(true)
            .append(op.append_mode)
            .create(true)
            .open(op.path)
            .unwrap()
    }

    #[track_caller]
    fn log<F, T>(&self, level: &'static str, f: F)
    where
        F: FnOnce() -> T + Send + 'static,
        T: AsRef<str>,
    {
        let tt = self.with_time;
        let location = std::panic::Location::caller();
        let entry = LogEntry {
            closure: Box::new(move || {
                let file_line = format!("{}:{}", location.file(), location.line());
                let time = match tt {
                    true => format!(
                        "{}",
                        chrono::offset::Local::now().format("%Y-%m-%d %H:%M:%S ")
                    ),
                    false => String::new(),
                };
                let message = f();
                format!("{}{} {} {}", time, file_line, level, message.as_ref())
            }),
            log_to: self.log_to.clone(),
        };

        match self.sx.borrow_mut().send(entry) {
            Ok(_) => (),
            Err(_) => panic!("Logger thread died :("),
        }
    }

    pub fn with_time(mut self, time: bool) -> Self {
        self.with_time = time;
        self
    }

    /// Waits until all messages are logged
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        while self.sx.borrow().is_connected() {
            thread::yield_now();
        }

        if let Some(ref file) = self.file {
            file.sync_all().unwrap();
        }
    }

    #[track_caller]
    pub fn info<F, T>(&self, f: F)
    where
        F: FnOnce() -> T + Send + 'static,
        T: AsRef<str>,
    {
        self.log("\x1b[32m[INFO]\x1b[0m", f);
    }

    #[track_caller]
    pub fn error<F, T>(&self, f: F)
    where
        F: FnOnce() -> T + Send + 'static,
        T: AsRef<str>,
    {
        self.log("\x1b[31m[ERROR]\x1b[0m", f);
    }

    #[track_caller]
    pub fn debug<F, T>(&self, f: F)
    where
        F: FnOnce() -> T + Send + 'static,
        T: AsRef<str>,
    {
        self.log("\x1b[36m[DEBUG]\x1b[0m", f);
    }

    #[track_caller]
    pub fn warning<F, T>(&self, f: F)
    where
        F: FnOnce() -> T + Send + 'static,
        T: AsRef<str>,
    {
        self.log("\x1b[33m[WARNING]\x1b[0m", f);
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use std::fs;

    fn setup() {
        fs::File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open("log.txt")
            .unwrap();
    }

    fn teardown() {
        fs::remove_file("log.txt").unwrap();
    }

    #[test]
    fn run_test_sequentially() {
        simple_to_file();
        correct_ord();
        tt();
    }

    fn tt() {
        setup();
        let logger = Logger::builder(None).with_time(true);
        logger.info(String::new);
        logger.info(|| String::from("hello"));
        logger.debug(|| "foo");
        let logger = logger.with_time(false);
        logger.error(|| "bar");
        logger.warning(|| "world");

        logger.shutdown();
        teardown();
    }

    fn simple_to_file() {
        setup();
        let o = LoggerFileOptions {
            path: "log.txt",
            append_mode: false,
        };
        let logger = Logger::builder(Some(o)).with_time(false);
        logger.info(|| "to file".to_owned());
        logger.shutdown();
        let bytes = fs::read(o.path).unwrap();

        teardown();

        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "src/lib.rs:222 \u{1b}[32m[INFO]\u{1b}[0m to file\n".to_owned()
        );
    }

    fn correct_ord() {
        setup();
        let o = LoggerFileOptions {
            path: "log.txt",
            append_mode: false,
        };
        let logger = Logger::builder(Some(o));
        for i in 0..1000 {
            logger.debug(move || format!("{}", i));
        }

        logger.shutdown();

        for (i, line) in fs::read_to_string("log.txt").unwrap().lines().enumerate() {
            assert_eq!(
                line,
                format!("src/lib.rs:242 \u{1b}[36m[DEBUG]\u{1b}[0m {}", i)
            );
        }
        teardown();
    }
}
