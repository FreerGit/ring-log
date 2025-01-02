[![build & test](https://github.com/freergit/ring-log/actions/workflows/ci.yml/badge.svg)](https://github.com/freergit/ring-log/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/ring-log?style=flat-square)](https://crates.io/crates/ring-log/versions)
[![license](https://img.shields.io/github/license/freergit/ring-log)](https://github.com/freergit/ring-log/blob/main/LICENSE.txt)

# ring-log
High-performance logger with lock-free ring buffer, use this library when you want to log in the hotpath and performance is critical.

## Example
Submitting a log to either stdout or a file is very simple, you just give a closure which evaluates to a string. This is extremely fast, usually less than 100 nanos. A simple example:

```rust
let o = LoggerFileOptions {
    path: "log.txt",
    append_mode: false, // should the logger just append to what's already there or overwrite?
};

// The size is bounded, issuing a new log when the ringbuffer is full will block.
// When passing a LoggerFileOptions, .with_log_type(LogTo::File) is set implicitly.
let logger = Logger::builder(1024 * 8, Some(o)).with_time(true);

// Log to file
logger.info(String::new);
logger.info(|| String::from("hello"));
logger.debug(|| "foo");

// Log to stdout, without date/time
let logger = logger.with_log_type(LogTo::Ephemeral).with_time(false);

// Will now log to stdout
logger.info(String::new);
logger.info(|| String::from("hello"));
logger.debug(|| "foo");

// Set it back to file 
let logger = logger.with_log_type(LogTo::File);

// Blocks until all logs are handled. Natural race condition if this is not called.
logger.shutdown();
```

        let logger = Logger::builder(1024, None).with_time(true);
        logger.info(String::new);
        logger.info(|| String::from("hello"));
        logger.debug(|| "foo");
        let logger = logger.with_time(false);
        logger.error(|| "bar");
        logger.warning(|| "world");
        logger.shutdown();