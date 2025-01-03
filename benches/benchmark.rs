use std::{fs, time::Duration};

use criterion::{criterion_group, criterion_main, Criterion};
use ring_log::{Logger, LoggerFileOptions};

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

fn criterion_benchmark(c: &mut Criterion) {
    setup();
    let o = LoggerFileOptions {
        path: "log.txt",
        append_mode: false,
    };
    let logger = Logger::builder(Some(o));
    let mut group = c.benchmark_group("print");
    group.measurement_time(Duration::from_micros(25));
    group.sample_size(10);
    group.warm_up_time(Duration::from_micros(10));
    group.bench_function("print", |b| b.iter(|| logger.debug(|| "ff")));
    group.finish();

    logger.shutdown();
    teardown();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
