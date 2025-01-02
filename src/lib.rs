use std::cell::UnsafeCell;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

struct RingBuffer {
    buffer: Vec<AtomicUsize>,
    entries: Vec<UnsafeCell<Option<LogEntry>>>,
    head: AtomicUsize,
    tail: AtomicUsize,
    size: usize,
    is_empty: AtomicBool,
    shutdown: AtomicBool,
}

unsafe impl Sync for RingBuffer {}

impl RingBuffer {
    fn new(size: usize) -> Self {
        RingBuffer {
            size,
            buffer: (0..size).map(|_| AtomicUsize::new(0)).collect(),
            entries: (0..size).map(|_| UnsafeCell::new(None)).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            is_empty: AtomicBool::new(true),
            shutdown: AtomicBool::new(false),
        }
    }

    fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
    }

    fn should_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }

    fn is_empty(&self) -> bool {
        self.is_empty.load(Ordering::Acquire)
    }

    fn push(&self, entry: &mut LogEntry) -> bool {
        let mut head = self.head.load(Ordering::Relaxed);
        loop {
            let next = (head + 1) % self.size;
            if next == self.tail.load(Ordering::Relaxed) {
                return false; // Buffer is full
            }
            match self
                .head
                .compare_exchange(head, next, Ordering::Release, Ordering::Relaxed)
            {
                Ok(_) => {
                    unsafe {
                        // TODO
                        *self.entries[head].get() = Some(std::mem::replace(
                            entry,
                            LogEntry {
                                closure: Box::new(|| "Error: LogEntry moved".to_string()),
                                log_to: entry.log_to.clone(),
                            },
                        ));
                    }
                    self.buffer[head].store(1, Ordering::Release);
                    self.is_empty.store(false, Ordering::Release);
                    return true;
                }
                Err(x) => head = x,
            }
        }
    }

    fn pop(&self) -> Option<LogEntry> {
        let mut tail = self.tail.load(Ordering::Relaxed);
        loop {
            if self.buffer[tail].load(Ordering::Acquire) == 0 {
                return None; // Buffer is empty
            }
            let next = (tail + 1) % self.size;
            match self
                .tail
                .compare_exchange_weak(tail, next, Ordering::Release, Ordering::Relaxed)
            {
                Ok(_) => {
                    let entry = unsafe { (*self.entries[tail].get()).take() };
                    self.buffer[tail].store(0, Ordering::Release);
                    if next == self.head.load(Ordering::Relaxed) {
                        self.is_empty.store(true, Ordering::Release);
                    }
                    return entry;
                }
                Err(x) => tail = x,
            }
        }
    }

    fn wait_for_new_entries(&self) {
        while self.is_empty() && !self.should_shutdown() {
            thread::yield_now();
        }
    }
}

pub struct Logger {
    buffer: Arc<RingBuffer>,
    file: Option<File>,
    log_to: LogTo,
    with_time: bool,
}

#[derive(Clone, Copy)]
pub struct LoggerFileOptions {
    path: &'static str,
    append_mode: bool,
}

impl Logger {
    pub fn builder(size: usize, log_op: Option<LoggerFileOptions>) -> Self {
        let buffer = Arc::new(RingBuffer::new(size));
        let buffer_clone = buffer.clone();
        thread::spawn(move || {
            let mut file = None;
            if let Some(op) = log_op {
                file = Some(Logger::open_log_file(op));
            }
            loop {
                if let Some(entry) = buffer_clone.pop() {
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
                } else if buffer_clone.should_shutdown() && buffer_clone.is_empty() {
                    break;
                } else {
                    buffer_clone.wait_for_new_entries();
                }
            }
        });

        let file = log_op.map(Logger::open_log_file);

        Logger {
            buffer,
            file,
            log_to: log_op.map_or(LogTo::Ephemeral, |_| LogTo::File),
            with_time: false,
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
        let location = std::panic::Location::caller();
        let file_line = format!("{}:{}", location.file(), location.line());
        let tt = self.with_time;
        let mut entry = LogEntry {
            closure: Box::new(move || {
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

        while !self.buffer.push(&mut entry) {
            thread::yield_now(); // Wait if the buffer is full
        }
    }

    pub fn with_log_type(mut self, t: LogTo) -> Self {
        self.log_to = t;
        self
    }

    pub fn with_time(mut self, time: bool) -> Self {
        self.with_time = time;
        self
    }

    /// Waits until all messages are logged
    pub fn shutdown(&self) {
        self.buffer.shutdown();
        while !self.buffer.is_empty() {
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
        let logger = Logger::builder(1024, None).with_time(true);
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
        let logger = Logger::builder(1024, Some(o)).with_time(false);
        logger.info(|| "to file".to_owned());
        logger.shutdown();
        let bytes = fs::read(o.path).unwrap();
        teardown();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "src/lib.rs:314 \u{1b}[32m[INFO]\u{1b}[0m to file\n".to_owned()
        );
    }

    fn correct_ord() {
        setup();
        let o = LoggerFileOptions {
            path: "log.txt",
            append_mode: false,
        };
        let logger = Logger::builder(1024 * 1024, Some(o));
        for i in 0..100_000 {
            logger.debug(move || format!("{}", i));
        }

        logger.shutdown();

        for (i, line) in fs::read_to_string("log.txt").unwrap().lines().enumerate() {
            assert_eq!(
                line,
                format!("src/lib.rs:332 \u{1b}[36m[DEBUG]\u{1b}[0m {}", i)
            );
        }
        teardown();
    }
}
