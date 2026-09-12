pub(crate) fn builder() -> env_logger::Builder {
    let mut builder = env_logger::Builder::new();
    builder.filter_level(log::LevelFilter::Info);
    builder.filter_module("mainline", log::LevelFilter::Warn);
    builder
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn mainline_tracing_reaches_default_daemon_logger() {
        let capture = Capture::default();
        super::builder()
            .target(env_logger::Target::Pipe(Box::new(capture.clone())))
            .try_init()
            .unwrap();

        // The transport runs on separate threads without a tracing subscriber.
        std::thread::spawn(|| {
            tracing::warn!(
                target: "mainline::rpc::transport", peer = "127.0.0.1:9000", stage = "handshake",
                error = "test timeout", timeout_ms = 100, "diagnostic bridge probe"
            );
            tracing::debug!(target: "mainline::rpc::transport", "hidden gate retirement probe");
        })
        .join()
        .unwrap();

        let output = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
        for expected in [
            "diagnostic bridge probe",
            "127.0.0.1:9000",
            "handshake",
            "test timeout",
            "timeout_ms=100",
        ] {
            assert!(output.contains(expected), "missing {expected}: {output}");
        }
        assert!(!output.contains("hidden gate retirement probe"));

        // RUST_LOG uses the same filter parser, applied after the defaults.
        let logger = super::builder()
            .parse_filters("info,mainline=warn,mainline::rpc::transport=debug")
            .build();
        assert!(log::Log::enabled(
            &logger,
            &log::Metadata::builder()
                .target("mainline::rpc::transport")
                .level(log::Level::Debug)
                .build()
        ));
    }
}
