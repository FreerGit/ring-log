use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self};

use crossbeam::queue::ArrayQueue;

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
    queue: Arc<ArrayQueue<LogEntry>>,
    file: Option<File>,
    log_to: LogTo,
    with_time: bool,
    shutdown: Arc<AtomicBool>,
}

#[derive(Clone, Copy)]
pub struct LoggerFileOptions {
    path: &'static str,
    append_mode: bool,
}

impl Logger {
    pub fn builder(size: usize, log_op: Option<LoggerFileOptions>) -> Self {
        let queue = Arc::new(ArrayQueue::<LogEntry>::new(size));
        let queue_clone = queue.clone();
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let shutdown_flag_clone = shutdown_flag.clone();
        thread::spawn(move || {
            let mut file = None;
            if let Some(op) = log_op {
                file = Some(Logger::open_log_file(op));
            }

            loop {
                if let Some(entry) = queue_clone.pop() {
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
                } else if queue_clone.is_empty() && shutdown_flag_clone.load(Ordering::Acquire) {
                    break;
                } else {
                    thread::yield_now();
                }
            }
        });

        let file = log_op.map(Logger::open_log_file);

        Logger {
            queue,
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

        // let value = ;
        let value = self.queue.push(entry);
        while value.is_err() {
            thread::yield_now(); // Wait if the queue is full
        }
    }

    pub fn with_time(mut self, time: bool) -> Self {
        self.with_time = time;
        self
    }

    /// Waits until all messages are logged
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        while !self.queue.is_empty() {
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
    use timing_rdtsc::timing;

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
        test_thread_safety();
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
            "src/lib.rs:221 \u{1b}[32m[INFO]\u{1b}[0m to file\n".to_owned()
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
                format!("src/lib.rs:239 \u{1b}[36m[DEBUG]\u{1b}[0m {}", i)
            );
        }
        teardown();
    }

    fn test_thread_safety() {
        use std::fs;
        use std::sync::{Arc, Barrier};

        const NUM_THREADS: usize = 1;
        const ENTRIES_PER_THREAD: usize = 1000;
        const EXPECTED_TOTAL_ENTRIES: usize = NUM_THREADS * ENTRIES_PER_THREAD;
        let log_file_path = "thread_safety_test.log";

        // Configure the logger to write to a file
        let logger = Arc::new(Logger::builder(
            1024 * 100,
            Some(LoggerFileOptions {
                path: log_file_path,
                append_mode: false,
            }),
        ));
        let barrier = Arc::new(Barrier::new(NUM_THREADS + 1));

        let mut handles = vec![];
        for thread_id in 0..NUM_THREADS {
            let logger = logger.clone();
            let barrier = barrier.clone();
            let handle = std::thread::spawn(move || {
                barrier.wait(); // Ensure all threads start logging simultaneously
                let time = timing(|| {
                    for i in 0..ENTRIES_PER_THREAD {
                        logger.info(move || format!("Thread {} - Entry {}", thread_id, i));
                    }
                });

                println!("{:#?}", time);
            });
            handles.push(handle);
        }

        barrier.wait(); // Start all threads

        for handle in handles {
            handle.join().unwrap();
        }

        logger.shutdown();

        // Validate the log file
        let content = fs::read_to_string(log_file_path).expect("Failed to read log file.");
        let log_entries: Vec<&str> = content.lines().collect();

        assert_eq!(
            log_entries.len(),
            EXPECTED_TOTAL_ENTRIES,
            "The total number of log entries is incorrect."
        );

        let mut seen_entries = std::collections::HashSet::new();
        for line in log_entries {
            assert!(seen_entries.insert(line.to_string()));
        }
        assert_eq!(
            seen_entries.len(),
            EXPECTED_TOTAL_ENTRIES,
            "Duplicate or missing log entries detected."
        );

        fs::remove_file(log_file_path).expect("Failed to remove log file.");
    }
}
